//! Pure-Rust High Performance Image & PDF Rasterization Engine.
//!
//! Replaces legacy ImageMagick bindings with buffered file I/O, native
//! HEIC/HEIF decoding (`heic`), SIMD-accelerated resizing (`fast_image_resize`),
//! and multi-core PDF page rendering (`hayro` + `rayon`).
//!
//! This module intentionally contains **no `unsafe` code**: every decoder is fed
//! from an owned `Vec<u8>` rather than a memory map.

use crate::error::{FileConverterError, Result};
use crate::settings::ConversionPreset;
use crate::types::{OutputType, is_native_image_extension};
use image::{DynamicImage, GenericImageView, ImageDecoder, ImageReader};
use rayon::prelude::*;
use std::io::{Cursor, Write};
use std::path::Path;
use std::sync::{Arc, LazyLock};

static SHARED_FONTDB: LazyLock<Arc<resvg::usvg::fontdb::Database>> = LazyLock::new(|| {
    let mut fontdb = resvg::usvg::fontdb::Database::new();
    fontdb.load_system_fonts();
    Arc::new(fontdb)
});

use hayro::hayro_syntax::Pdf;
use hayro::{RenderCache, RenderSettings, render};
use hayro_interpret::InterpreterSettings;
use hayro_interpret::font::FontQuery;

use fast_image_resize::{
    PixelType, Resizer,
    images::{Image, ImageRef},
};
use heic::{DecoderConfig, PixelLayout};

/// Reads a whole file into memory. Kept in one place so every decoder is fed
/// from an owned buffer (no `unsafe` memory-mapping required).
fn read_file_bytes(path: &str, what: &str) -> Result<Vec<u8>> {
    std::fs::read(path)
        .map_err(|e| FileConverterError::Image(format!("Failed to open {} file: {}", what, e)))
}

fn unsupported_image_kind(ext: &str) -> FileConverterError {
    FileConverterError::Image(format!(
        "'{}' images require a dedicated decoder that is not bundled. \
         Convert the file to PNG/JPEG/TIFF/WebP first (for example with FFmpeg or a raw converter).",
        ext
    ))
}

/// Returns total page count of a PDF document.
pub fn get_pdf_page_count(input_path: &str) -> Result<usize> {
    let bytes = read_file_bytes(input_path, "PDF")?;
    let pdf = Pdf::new(Arc::new(bytes))
        .map_err(|e| FileConverterError::Image(format!("Failed to parse PDF document: {:?}", e)))?;

    Ok(pdf.pages().len())
}

/// Retrieves image dimensions (width, height) without full image decoding.
pub fn get_image_dimensions(input_path: &str) -> Result<(u32, u32)> {
    let ext = Path::new(input_path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    if ext == "pdf" {
        let bytes = read_file_bytes(input_path, "PDF")?;
        let pdf = Pdf::new(Arc::new(bytes)).map_err(|e| {
            FileConverterError::Image(format!("Failed to parse PDF document: {:?}", e))
        })?;
        let page = pdf.pages().first().ok_or_else(|| {
            FileConverterError::Image("PDF document does not contain any page".to_string())
        })?;
        let (w, h) = page.render_dimensions();
        return Ok((w.ceil().max(1.0) as u32, h.ceil().max(1.0) as u32));
    }

    let data = read_file_bytes(input_path, "image")?;

    if ext == "svg" {
        let opt = resvg::usvg::Options {
            fontdb: SHARED_FONTDB.clone(),
            ..Default::default()
        };
        let tree = resvg::usvg::Tree::from_data(&data, &opt)
            .map_err(|e| FileConverterError::Image(format!("Failed to parse SVG: {:?}", e)))?;
        let size = tree.size().to_int_size();
        return Ok((size.width().max(1), size.height().max(1)));
    }

    if ext == "jxl" {
        let jxl = jxl_oxide::JxlImage::builder()
            .read(Cursor::new(&data))
            .map_err(|e| FileConverterError::Image(format!("Failed to parse JXL: {:?}", e)))?;
        return Ok((jxl.width(), jxl.height()));
    }

    if (ext == "heic" || ext == "heif") && is_native_image_extension(&ext) {
        let output = DecoderConfig::new()
            .decode(&data, PixelLayout::Rgba8)
            .map_err(|e| {
                FileConverterError::Image(format!("Failed to decode HEIC file: {:?}", e))
            })?;
        return Ok((output.width, output.height));
    }

    if !is_native_image_extension(&ext) {
        return Err(unsupported_image_kind(&ext));
    }

    let reader = ImageReader::new(Cursor::new(&data))
        .with_guessed_format()
        .map_err(|e| {
            FileConverterError::Image(format!("Failed to detect image format: {:?}", e))
        })?;
    let reader = reader.into_decoder().map_err(|e| {
        FileConverterError::Image(format!("Failed to load image from memory: {:?}", e))
    })?;

    Ok(reader.dimensions())
}

fn decode_jxl_from_memory(data: &[u8]) -> Result<DynamicImage> {
    let decoder = jxl_oxide::integration::JxlDecoder::new(Cursor::new(data)).map_err(|e| {
        FileConverterError::Image(format!("Failed to initialize JXL decoder: {:?}", e))
    })?;
    DynamicImage::from_decoder(decoder)
        .map_err(|e| FileConverterError::Image(format!("Failed to decode JXL image: {:?}", e)))
}

/// Resizes images using CPU SIMD vectors (AVX2/NEON/SSE4.1) via `fast_image_resize`.
fn resize_simd(img: &DynamicImage, target_w: u32, target_h: u32) -> Result<DynamicImage> {
    let (src_w, src_h) = img.dimensions();
    let target_w = target_w.max(1);
    let target_h = target_h.max(1);
    if (src_w, src_h) == (target_w, target_h) {
        return Ok(img.clone());
    }

    let mut resizer = Resizer::new();
    let resize_options = fast_image_resize::ResizeOptions {
        algorithm: fast_image_resize::ResizeAlg::Convolution(
            fast_image_resize::FilterType::Lanczos3,
        ),
        ..Default::default()
    };

    match img {
        DynamicImage::ImageRgb8(buf) => {
            let src_image = ImageRef::new(buf.width(), buf.height(), buf.as_raw(), PixelType::U8x3)
                .map_err(|e| {
                    FileConverterError::Image(format!("Failed to create SIMD RGB image: {:?}", e))
                })?;
            let mut dst_image = Image::new(target_w, target_h, PixelType::U8x3);
            resizer
                .resize(&src_image, &mut dst_image, Some(&resize_options))
                .map_err(|e| FileConverterError::Image(format!("SIMD resize failed: {:?}", e)))?;
            let buffer = dst_image.into_vec();
            let rgb_buf =
                image::ImageBuffer::from_raw(target_w, target_h, buffer).ok_or_else(|| {
                    FileConverterError::Image(
                        "Failed to create ImageBuffer from resized RGB data".to_string(),
                    )
                })?;
            Ok(DynamicImage::ImageRgb8(rgb_buf))
        }
        DynamicImage::ImageLuma8(buf) => {
            let src_image = ImageRef::new(buf.width(), buf.height(), buf.as_raw(), PixelType::U8)
                .map_err(|e| {
                FileConverterError::Image(format!("Failed to create SIMD Luma image: {:?}", e))
            })?;
            let mut dst_image = Image::new(target_w, target_h, PixelType::U8);
            resizer
                .resize(&src_image, &mut dst_image, Some(&resize_options))
                .map_err(|e| FileConverterError::Image(format!("SIMD resize failed: {:?}", e)))?;
            let buffer = dst_image.into_vec();
            let luma_buf =
                image::ImageBuffer::from_raw(target_w, target_h, buffer).ok_or_else(|| {
                    FileConverterError::Image(
                        "Failed to create ImageBuffer from resized Luma data".to_string(),
                    )
                })?;
            Ok(DynamicImage::ImageLuma8(luma_buf))
        }
        _ => {
            let rgba_img = img.to_rgba8();
            let src_image = Image::from_vec_u8(
                img.width(),
                img.height(),
                rgba_img.into_raw(),
                PixelType::U8x4,
            )
            .map_err(|e| {
                FileConverterError::Image(format!("Failed to create SIMD source image: {:?}", e))
            })?;

            let mut dst_image = Image::new(target_w, target_h, PixelType::U8x4);

            resizer
                .resize(&src_image, &mut dst_image, Some(&resize_options))
                .map_err(|e| FileConverterError::Image(format!("SIMD resize failed: {:?}", e)))?;

            let buffer = dst_image.into_vec();
            let rgba_buf =
                image::ImageBuffer::from_raw(target_w, target_h, buffer).ok_or_else(|| {
                    FileConverterError::Image(
                        "Failed to create ImageBuffer from resized data".to_string(),
                    )
                })?;

            Ok(DynamicImage::ImageRgba8(rgba_buf))
        }
    }
}

/// Largest power of two that is `<= value` (and at least 1).
fn floor_power_of_two(value: u32) -> u32 {
    if value < 2 {
        return 1;
    }
    1u32 << (31 - value.leading_zeros())
}

/// Rasterizes SVG markup at the requested pixel size.
pub fn rasterize_svg(svg: &str, width: u32, height: u32) -> Result<DynamicImage> {
    let opt = resvg::usvg::Options {
        fontdb: SHARED_FONTDB.clone(),
        ..Default::default()
    };
    let tree = resvg::usvg::Tree::from_str(svg, &opt).map_err(|e| {
        FileConverterError::Image(format!("Failed to parse generated SVG: {:?}", e))
    })?;

    let target_w = width.max(1);
    let target_h = height.max(1);

    let mut pixmap = resvg::tiny_skia::Pixmap::new(target_w, target_h)
        .ok_or_else(|| FileConverterError::Image("Failed to create SVG pixmap canvas".into()))?;

    let scale_x = target_w as f32 / tree.size().width().max(f32::MIN_POSITIVE);
    let scale_y = target_h as f32 / tree.size().height().max(f32::MIN_POSITIVE);
    let transform = resvg::tiny_skia::Transform::from_row(scale_x, 0.0, 0.0, scale_y, 0.0, 0.0);

    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let buffer = image::ImageBuffer::from_raw(target_w, target_h, pixmap.data().to_vec())
        .ok_or_else(|| {
            FileConverterError::Image("Failed to create ImageBuffer from SVG pixmap".into())
        })?;

    Ok(DynamicImage::ImageRgba8(buffer))
}

/// Loads any raster/vector image into memory using the pure-Rust decoders.
fn decode_image_bytes(ext: &str, data: &[u8], preset: &ConversionPreset) -> Result<DynamicImage> {
    match ext {
        "svg" => {
            let scale_factor = parsed_scale(preset);
            let opt = resvg::usvg::Options {
                fontdb: SHARED_FONTDB.clone(),
                ..Default::default()
            };
            let tree = resvg::usvg::Tree::from_data(data, &opt)
                .map_err(|e| FileConverterError::Image(format!("Failed to parse SVG: {:?}", e)))?;

            let size = tree.size().to_int_size();
            let orig_w = size.width().max(1);
            let orig_h = size.height().max(1);
            let target_w = ((orig_w as f32 * scale_factor).round() as u32).max(1);
            let target_h = ((orig_h as f32 * scale_factor).round() as u32).max(1);

            let source = std::str::from_utf8(data).map_err(|_| {
                FileConverterError::Image("SVG source is not valid UTF-8".to_string())
            })?;
            rasterize_svg(source, target_w, target_h)
        }
        "jxl" => decode_jxl_from_memory(data),
        "heic" | "heif" => {
            let output = DecoderConfig::new()
                .decode(data, PixelLayout::Rgba8)
                .map_err(|e| {
                    FileConverterError::Image(format!("Failed to decode HEIC: {:?}", e))
                })?;
            let buffer = image::ImageBuffer::from_raw(output.width, output.height, output.data)
                .ok_or_else(|| {
                    FileConverterError::Image("Failed to parse HEIC buffer".to_string())
                })?;
            Ok(DynamicImage::ImageRgba8(buffer))
        }
        _ if !is_native_image_extension(ext) => Err(unsupported_image_kind(ext)),
        _ => image::load_from_memory(data).map_err(|e| {
            FileConverterError::Image(format!("Failed to decode image data: {:?}", e))
        }),
    }
}

fn parsed_scale(preset: &ConversionPreset) -> f32 {
    let raw = preset
        .get_setting_value("ImageScale")
        .and_then(|v| v.trim().replace(',', ".").parse::<f32>().ok())
        .unwrap_or(1.0);
    if raw.is_finite() && raw > 0.0 {
        raw
    } else {
        1.0
    }
}

fn parsed_rotation(preset: &ConversionPreset) -> f32 {
    let raw = preset
        .get_setting_value("ImageRotation")
        .and_then(|v| v.trim().replace(',', ".").parse::<f32>().ok())
        .unwrap_or(0.0);
    if raw.is_finite() { raw } else { 0.0 }
}

/// Applies the shared `ImageScale` / `ImageRotation` / clamp pipeline.
///
/// `already_scaled` must be true when the decoder produced the bitmap at the
/// requested scale (SVG rendering and PDF rasterization both do this natively).
fn apply_image_transforms(
    img: DynamicImage,
    preset: &ConversionPreset,
    already_scaled: bool,
) -> Result<DynamicImage> {
    let mut img = img;

    if !already_scaled {
        let scale_factor = parsed_scale(preset);
        if (scale_factor - 1.0).abs() >= 0.005 {
            let (w, h) = img.dimensions();
            let nw = ((w as f32 * scale_factor).round() as u32).max(1);
            let nh = ((h as f32 * scale_factor).round() as u32).max(1);
            img = resize_simd(&img, nw, nh)?;
        }
    }

    let normalized = parsed_rotation(preset).rem_euclid(360.0);
    if normalized >= 45.0 {
        let quarter_turns = ((normalized / 90.0).round() as u32) % 4;
        for _ in 0..quarter_turns {
            img = img.rotate90();
        }
    }

    let clamp_power_2 = preset
        .get_setting_value("ImageClampSizePowerOf2")
        .map(|v| {
            let v = v.trim();
            v.eq_ignore_ascii_case("true") || v == "1"
        })
        .unwrap_or(false);
    let max_size = preset
        .get_setting_value("ImageMaximumSize")
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(0);

    if clamp_power_2 || max_size > 0 {
        let (w, h) = img.dimensions();
        let mut target_w = w;
        let mut target_h = h;

        // Preserve the aspect ratio: each axis is independently rounded down to
        // the closest power of two instead of being forced into a square.
        if clamp_power_2 {
            target_w = floor_power_of_two(w);
            target_h = floor_power_of_two(h);
        }

        if max_size > 0 {
            target_w = std::cmp::min(target_w, max_size);
            target_h = std::cmp::min(target_h, max_size);
        }

        target_w = target_w.max(1);
        target_h = target_h.max(1);

        if (target_w, target_h) != (w, h) {
            img = resize_simd(&img, target_w, target_h)?;
        }
    }

    Ok(img)
}

/// Executes image and PDF page conversion operations.
pub fn run_image_conversion(
    preset: &ConversionPreset,
    input_path: &str,
    output_file_paths: &[String],
    progress_callback: &(dyn Fn(f32, &str) + Sync),
) -> Result<()> {
    let ext = Path::new(input_path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    if output_file_paths.is_empty() {
        return Err(FileConverterError::Invalid(
            "No output path specified for image conversion".to_string(),
        ));
    }

    if ext == "pdf" {
        run_pdf_page_conversion(preset, input_path, output_file_paths, progress_callback)
    } else {
        run_still_image_conversion(
            preset,
            input_path,
            ext,
            output_file_paths,
            progress_callback,
        )
    }
}

fn run_pdf_page_conversion(
    preset: &ConversionPreset,
    input_path: &str,
    output_file_paths: &[String],
    progress_callback: &(dyn Fn(f32, &str) + Sync),
) -> Result<()> {
    let bytes = read_file_bytes(input_path, "PDF")?;
    let pdf = Pdf::new(Arc::new(bytes))
        .map_err(|e| FileConverterError::Image(format!("Failed to parse PDF: {:?}", e)))?;

    let pages = pdf.pages();
    let page_count = output_file_paths.len().min(pages.len()).max(1);

    let scale_factor = parsed_scale(preset);

    let interp_settings = InterpreterSettings {
        font_resolver: Arc::new(|query| match query {
            FontQuery::Standard(s) => Some(s.get_font_data()),
            FontQuery::Fallback(f) => Some(f.pick_standard_font().get_font_data()),
        }),
        ..Default::default()
    };

    let render_settings = RenderSettings {
        x_scale: scale_factor,
        y_scale: scale_factor,
        ..Default::default()
    };

    use std::sync::atomic::{AtomicUsize, Ordering};
    let pages_done = AtomicUsize::new(0);

    let results: Result<()> = (0..page_count)
        .into_par_iter()
        .map(|index| {
            let page = &pages[index];

            let done = pages_done.fetch_add(1, Ordering::Relaxed) + 1;
            progress_callback(done as f32 / page_count as f32, "Rendering PDF page");

            let local_cache = RenderCache::default();
            let pixmap = render(page, &local_cache, &interp_settings, &render_settings);

            let width = pixmap.width() as u32;
            let height = pixmap.height() as u32;

            // hayro/vello stores premultiplied alpha; `image` expects straight RGBA.
            let straight_rgba = pixmap.take_unpremultiplied();
            let buffer = image::ImageBuffer::from_raw(
                width,
                height,
                bytemuck::cast_slice(&straight_rgba).to_vec(),
            )
            .ok_or_else(|| {
                FileConverterError::Image("Failed to create ImageBuffer from PDF page".to_string())
            })?;

            // Rendered pages must honour the same rotation/clamp settings as
            // still images, otherwise presets such as "To Ico" or "Scale x%"
            // silently ignore them for PDF input.
            let img = apply_image_transforms(DynamicImage::ImageRgba8(buffer), preset, true)?;

            save_image(&img, preset, &output_file_paths[index])
        })
        .collect();

    results?;
    progress_callback(1.0, "Done");
    Ok(())
}

fn run_still_image_conversion(
    preset: &ConversionPreset,
    input_path: &str,
    ext: String,
    output_file_paths: &[String],
    progress_callback: &(dyn Fn(f32, &str) + Sync),
) -> Result<()> {
    progress_callback(0.0, "Loading Image");

    if !is_native_image_extension(&ext) {
        return Err(unsupported_image_kind(&ext));
    }

    let data = read_file_bytes(input_path, "image")?;
    let img = decode_image_bytes(&ext, &data, preset)?;

    progress_callback(0.4, "Processing transforms");

    // SVGs are already rasterised at the requested scale factor.
    let img = apply_image_transforms(img, preset, ext == "svg")?;

    progress_callback(0.7, "Saving Image");
    save_image(&img, preset, &output_file_paths[0])?;

    progress_callback(1.0, "Done");
    Ok(())
}

fn save_image(img: &DynamicImage, preset: &ConversionPreset, output_file: &str) -> Result<()> {
    if preset.output_type == OutputType::Pdf {
        return create_pdf_from_image(img, output_file);
    }

    if preset.output_type == OutputType::Ico {
        return save_ico(img, output_file);
    }

    if preset.output_type == OutputType::Gif {
        return save_still_gif(img, output_file);
    }

    let file = std::fs::File::create(output_file).map_err(|e| {
        FileConverterError::Image(format!(
            "Failed to create output file {}: {:?}",
            output_file, e
        ))
    })?;
    let mut writer = std::io::BufWriter::with_capacity(128 * 1024, file);

    match preset.output_type {
        OutputType::Jpg => {
            let quality = preset
                .get_setting_value("JpegQuality")
                .and_then(|v| v.trim().parse::<u8>().ok())
                .unwrap_or(85)
                .clamp(1, 100);
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, quality);
            img.write_with_encoder(encoder).map_err(|e| {
                FileConverterError::Image(format!("Failed to save JPEG image: {:?}", e))
            })?;
        }
        OutputType::Png => {
            let encoder = image::codecs::png::PngEncoder::new(&mut writer);
            img.write_with_encoder(encoder).map_err(|e| {
                FileConverterError::Image(format!("Failed to save PNG image: {:?}", e))
            })?;
        }
        OutputType::Webp => {
            let encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut writer);
            img.write_with_encoder(encoder).map_err(|e| {
                FileConverterError::Image(format!("Failed to save WebP image: {:?}", e))
            })?;
        }
        OutputType::Avif => {
            let encoder = image::codecs::avif::AvifEncoder::new(&mut writer);
            img.write_with_encoder(encoder).map_err(|e| {
                FileConverterError::Image(format!("Failed to save AVIF image: {:?}", e))
            })?;
        }
        other => {
            return Err(FileConverterError::Image(format!(
                "The image engine cannot write '{}' output",
                other.extension()
            )));
        }
    }

    writer
        .flush()
        .map_err(|e| FileConverterError::Image(format!("Failed to flush output file: {:?}", e)))?;

    Ok(())
}

/// Writes a still image as a single-frame GIF using the pure-Rust encoder.
///
/// Routing still images through FFmpeg's `palettegen`/`paletteuse` pair is
/// unreliable (a single-frame source can produce an empty palette file, which
/// then breaks the second pass), so stills are encoded directly here.
fn save_still_gif(img: &DynamicImage, output_file: &str) -> Result<()> {
    let file = std::fs::File::create(output_file).map_err(|e| {
        FileConverterError::Image(format!(
            "Failed to create output file {}: {:?}",
            output_file, e
        ))
    })?;
    let mut writer = std::io::BufWriter::with_capacity(128 * 1024, file);

    let rgba = img.to_rgba8();
    let frame = image::Frame::new(rgba);

    let mut encoder = image::codecs::gif::GifEncoder::new(&mut writer);
    encoder
        .encode_frame(frame)
        .map_err(|e| FileConverterError::Image(format!("Failed to save GIF image: {:?}", e)))?;
    drop(encoder);

    writer
        .flush()
        .map_err(|e| FileConverterError::Image(format!("Failed to flush output file: {:?}", e)))?;

    Ok(())
}

/// Writes a multi-resolution ICO container (16/24/32/48/64/128/256 where the
/// source artwork allows it) so the icon looks sharp at every shell size and the
/// aspect ratio is preserved instead of being forced into a square.
fn save_ico(img: &DynamicImage, output_file: &str) -> Result<()> {
    const ICO_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

    let (src_w, src_h) = img.dimensions();
    if src_w == 0 || src_h == 0 {
        return Err(FileConverterError::Image(
            "Cannot build an icon from an empty image".to_string(),
        ));
    }

    let longest = src_w.max(src_h);
    let mut frames = Vec::new();

    for size in ICO_SIZES {
        // Never upscale beyond the source artwork.
        if size > longest {
            continue;
        }
        let scale = size as f32 / longest as f32;
        let w = ((src_w as f32 * scale).round() as u32).clamp(1, size);
        let h = ((src_h as f32 * scale).round() as u32).clamp(1, size);

        let resized = resize_simd(img, w, h)?;
        let rgba = resized.to_rgba8();
        let frame = image::codecs::ico::IcoFrame::as_png(
            rgba.as_raw(),
            rgba.width(),
            rgba.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| FileConverterError::Image(format!("Failed to build ICO frame: {:?}", e)))?;
        frames.push(frame);
    }

    if frames.is_empty() {
        return Err(FileConverterError::Image(
            "Cannot build an icon from an empty image".to_string(),
        ));
    }

    let file = std::fs::File::create(output_file).map_err(|e| {
        FileConverterError::Image(format!(
            "Failed to create output file {}: {:?}",
            output_file, e
        ))
    })?;
    let mut writer = std::io::BufWriter::with_capacity(128 * 1024, file);
    image::codecs::ico::IcoEncoder::new(&mut writer)
        .encode_images(&frames)
        .map_err(|e| FileConverterError::Image(format!("Failed to save ICO image: {:?}", e)))?;
    writer
        .flush()
        .map_err(|e| FileConverterError::Image(format!("Failed to flush output file: {:?}", e)))?;

    Ok(())
}

fn create_pdf_from_image(img: &DynamicImage, output_path: &str) -> Result<()> {
    use image::ImageEncoder;
    use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref};

    // Treat the bitmap as 96 DPI so the PDF page gets a sane physical size
    // instead of one PDF point per pixel (which produced 50+ inch pages).
    const ASSUMED_DPI: f32 = 96.0;
    let points_per_pixel = 72.0 / ASSUMED_DPI;

    let w = (img.width() as f32 * points_per_pixel).max(1.0);
    let h = (img.height() as f32 * points_per_pixel).max(1.0);

    let mut pdf = Pdf::new();
    let catalog_id = Ref::new(1);
    let pages_id = Ref::new(2);
    let page_id = Ref::new(3);
    let image_id = Ref::new(4);
    let content_id = Ref::new(5);

    let rgb = img.to_rgb8();
    let mut jpeg_data = Vec::new();
    let mut cursor = Cursor::new(&mut jpeg_data);
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, 85);
    encoder
        .write_image(
            rgb.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|e| FileConverterError::Image(e.to_string()))?;

    let image_name = Name(b"Im1");

    let mut image_obj = pdf.image_xobject(image_id, &jpeg_data);
    image_obj.filter(Filter::DctDecode);
    image_obj.width(img.width() as i32);
    image_obj.height(img.height() as i32);
    image_obj.color_space().device_rgb();
    image_obj.bits_per_component(8);
    image_obj.finish();

    let mut content = Content::new();
    content.save_state();
    content.transform([w, 0.0, 0.0, h, 0.0, 0.0]);
    content.x_object(image_name);
    content.restore_state();
    pdf.stream(content_id, &content.finish());

    let mut page = pdf.page(page_id);
    page.media_box(Rect::new(0.0, 0.0, w, h));
    page.parent(pages_id);
    page.contents(content_id);
    let mut resources = page.resources();
    resources.x_objects().pair(image_name, image_id);
    resources.finish();
    page.finish();

    let mut pages = pdf.pages(pages_id);
    pages.kids([page_id]);
    pages.count(1);
    pages.finish();

    pdf.catalog(catalog_id).pages(pages_id);

    std::fs::write(output_path, pdf.finish()).map_err(FileConverterError::Io)?;
    Ok(())
}

/// Detects whether an image file contains more than one frame.
///
/// Only cheap header inspection is performed (the first few kilobytes are enough
/// for both GIF and WebP), so this is safe to call from the scheduler.
pub fn source_is_animated(input_path: &str) -> bool {
    let ext = Path::new(input_path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    if !matches!(ext.as_str(), "gif" | "webp" | "png") {
        return false;
    }

    let Ok(file) = std::fs::File::open(input_path) else {
        return false;
    };

    match ext.as_str() {
        "webp" => {
            use std::io::Read;
            let mut header = [0u8; 64];
            let mut reader = std::io::BufReader::new(file);
            let Ok(read) = reader.read(&mut header) else {
                return false;
            };
            if read < 30 || &header[0..4] != b"RIFF" || &header[8..12] != b"WEBP" {
                return false;
            }
            // `VP8X` extended format carries an ANIM flag in its flags byte.
            matches!(&header[12..16], b"VP8X") && header[20] & 0x02 != 0
        }
        // GIF: more than one Graphic Control Extension block means animation.
        "gif" => {
            use std::io::Read;
            let mut reader = std::io::BufReader::new(file);
            let mut buf = vec![0u8; 32 * 1024];
            let Ok(read) = reader.read(&mut buf) else {
                return false;
            };
            buf[..read]
                .windows(3)
                .filter(|w| w[0] == 0x21 && w[1] == 0xF9)
                .count()
                > 1
        }
        // APNG: an `acTL` chunk means animation.
        _ => {
            use std::io::Read;
            let mut reader = std::io::BufReader::new(file);
            let mut buf = vec![0u8; 1024];
            let Ok(read) = reader.read(&mut buf) else {
                return false;
            };
            if read < 8 {
                return false;
            }
            buf[..read].windows(4).any(|w| w == b"acTL")
        }
    }
}

/// Converts a **still** image into a GIF container using the pure-Rust encoder.
///
/// FFmpeg's `palettegen`/`paletteuse` recipe silently produces an empty palette
/// file for single-frame sources (and therefore fails the second pass), so still
/// images are encoded here instead. Animated sources and video keep using the
/// FFmpeg path in the scheduler, where the two-pass recipe works reliably.
pub fn run_still_gif_conversion(
    preset: &ConversionPreset,
    input_path: &str,
    output_file: &str,
    progress_callback: &(dyn Fn(f32, &str) + Sync),
) -> Result<()> {
    let ext = Path::new(input_path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    if !is_native_image_extension(&ext) {
        return Err(unsupported_image_kind(&ext));
    }

    progress_callback(0.2, "Decoding image");
    let data = read_file_bytes(input_path, "image")?;
    let img = decode_image_bytes(&ext, &data, preset)?;

    progress_callback(0.6, "Processing transforms");
    // SVGs are already rasterised at the requested scale factor.
    let img = apply_image_transforms(img, preset, ext == "svg")?;

    progress_callback(0.85, "Saving GIF");
    save_still_gif(&img, output_file)?;
    progress_callback(1.0, "Done");
    Ok(())
}

/// Rasterizes a laid-out document page (SVG markup) into the requested raster
/// image format. Used by the plain-text / markup document engine.
pub fn rasterize_svg_page(
    svg: &str,
    output_file: &str,
    output_type: OutputType,
    progress_callback: &(dyn Fn(f32, &str) + Sync),
) -> Result<()> {
    progress_callback(0.4, "Rasterizing document page");

    // A4 page rendered at ~150 DPI.
    const BASE_W: u32 = 1240;
    const BASE_H: u32 = 1754;

    let preset = ConversionPreset {
        name: "Document page".to_string(),
        output_type,
        is_default_settings: true,
        input_types: Vec::new(),
        input_post_conversion_action: crate::types::InputPostConversionAction::None,
        settings: Vec::new(),
        output_file_name_template: String::new(),
    };

    let img = rasterize_svg(svg, BASE_W, BASE_H)?;
    let img = apply_image_transforms(img, &preset, true)?;

    progress_callback(0.85, "Saving image");
    save_image(&img, &preset, output_file)?;
    progress_callback(1.0, "Done");
    Ok(())
}

/// Losslessly compresses a PNG image using `oxipng`.
pub fn run_oxipng_compression<F>(
    preset: Option<&crate::settings::ConversionPreset>,
    input_file: &str,
    output_file: &str,
    progress_callback: F,
) -> Result<()>
where
    F: Fn(f32, &str),
{
    progress_callback(0.1, "Reading PNG file");

    let opt_level = preset
        .and_then(|p| p.get_setting_value("OxipngOptimizationLevel"))
        .and_then(|v| v.parse::<u8>().ok())
        .unwrap_or(2)
        .clamp(1, 6);

    let mut options = oxipng::Options::from_preset(opt_level);

    if let Some(strip_val) = preset.and_then(|p| p.get_setting_value("OxipngStrip")) {
        match strip_val.to_lowercase().as_str() {
            "all" => options.strip = oxipng::StripChunks::All,
            "safe" => options.strip = oxipng::StripChunks::Safe,
            "none" => options.strip = oxipng::StripChunks::None,
            _ => {}
        }
    }

    if let Some(interlace_val) = preset.and_then(|p| p.get_setting_value("OxipngInterlace")) {
        if interlace_val.eq_ignore_ascii_case("true") || interlace_val.eq_ignore_ascii_case("adam7")
        {
            options.interlace = Some(oxipng::Interlacing::Adam7);
        } else if interlace_val.eq_ignore_ascii_case("false")
            || interlace_val.eq_ignore_ascii_case("none")
        {
            options.interlace = Some(oxipng::Interlacing::None);
        }
    }

    let in_file = oxipng::InFile::Path(std::path::PathBuf::from(input_file));
    let out_file = oxipng::OutFile::Path {
        path: Some(std::path::PathBuf::from(output_file)),
        preserve_attrs: false,
    };

    progress_callback(0.4, "Optimizing PNG losslessly via OxiPNG");
    oxipng::optimize(&in_file, &out_file, &options)
        .map_err(|e| FileConverterError::Image(format!("Oxipng compression failed: {:?}", e)))?;

    progress_callback(1.0, "Done");
    Ok(())
}
