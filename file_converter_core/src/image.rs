//! Pure-Rust High Performance Image & PDF Rasterization Engine.
//!
//! Replaces legacy ImageMagick bindings with zero-copy memory-mapped file I/O (`memmap2`),
//! native HEIC/HEIF decoding (`heic`), SIMD-accelerated resizing (`fast_image_resize`),
//! and multi-core PDF page rendering (`hayro` + `rayon`).

use image::{DynamicImage, GenericImageView, ImageFormat};
use std::path::Path;
use std::sync::Arc;

use crate::settings::ConversionPreset;
use crate::types::OutputType;

use hayro::hayro_syntax::Pdf;
use hayro::{RenderCache, RenderSettings, render};
use hayro_interpret::InterpreterSettings;
use hayro_interpret::font::FontQuery;

use fast_image_resize::{PixelType, Resizer, images::Image};
use heic::{DecoderConfig, PixelLayout};
use memmap2::Mmap;

/// Returns total page count of a PDF document using memory-mapped parsing.
pub fn get_pdf_page_count(input_path: &str) -> Result<usize, String> {
    let file =
        std::fs::File::open(input_path).map_err(|e| format!("Failed to open PDF file: {:?}", e))?;

    let mmap =
        unsafe { Mmap::map(&file) }.map_err(|e| format!("Failed to memory map PDF: {:?}", e))?;

    let pdf =
        Pdf::new(Arc::new(mmap)).map_err(|e| format!("Failed to parse PDF document: {:?}", e))?;

    Ok(pdf.pages().len())
}

/// Retrieves image dimensions (width, height) without full image decoding.
pub fn get_image_dimensions(input_path: &str) -> Result<(u32, u32), String> {
    let ext = Path::new(input_path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    let file = std::fs::File::open(input_path)
        .map_err(|e| format!("Failed to open image file: {:?}", e))?;

    let mmap =
        unsafe { Mmap::map(&file) }.map_err(|e| format!("Failed to memory map image: {:?}", e))?;

    if ext == "heic" || ext == "heif" {
        let output = DecoderConfig::new()
            .decode(&mmap, PixelLayout::Rgba8)
            .map_err(|e| format!("Failed to decode HEIC file: {:?}", e))?;
        return Ok((output.width, output.height));
    }

    let img = image::load_from_memory(&mmap)
        .map_err(|e| format!("Failed to load image from memory map: {:?}", e))?;

    Ok(img.dimensions())
}

/// Resizes images using CPU SIMD vectors (AVX2/NEON/SSE4.1) via `fast_image_resize`.
fn resize_simd(img: &DynamicImage, target_w: u32, target_h: u32) -> Result<DynamicImage, String> {
    let rgba_img = img.to_rgba8();
    let src_image = Image::from_vec_u8(
        img.width(),
        img.height(),
        rgba_img.into_raw(),
        PixelType::U8x4,
    )
    .map_err(|e| format!("Failed to create SIMD source image: {:?}", e))?;

    let mut dst_image = Image::new(target_w, target_h, PixelType::U8x4);

    let mut resizer = Resizer::new();
    resizer
        .resize(&src_image, &mut dst_image, None)
        .map_err(|e| format!("SIMD resize failed: {:?}", e))?;

    let buffer = dst_image.buffer().to_vec();
    let rgba_buf = image::ImageBuffer::from_raw(target_w, target_h, buffer)
        .ok_or_else(|| "Failed to create ImageBuffer from resized data".to_string())?;

    Ok(DynamicImage::ImageRgba8(rgba_buf))
}

/// Executes image and PDF page conversion operations.
pub fn run_image_conversion(
    preset: &ConversionPreset,
    input_path: &str,
    output_file_paths: &[String],
    progress_callback: &(dyn Fn(f32, &str) + Sync),
) -> Result<(), String> {
    let ext = Path::new(input_path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    let is_pdf = ext == "pdf";

    if is_pdf {
        let pdf_file = std::fs::File::open(input_path)
            .map_err(|e| format!("Failed to open PDF file: {:?}", e))?;

        let mmap = unsafe { Mmap::map(&pdf_file) }
            .map_err(|e| format!("Failed to memory map PDF file: {:?}", e))?;

        let pdf = Pdf::new(Arc::new(mmap)).map_err(|e| format!("Failed to parse PDF: {:?}", e))?;

        let page_count = output_file_paths.len();

        let scale_factor = preset
            .get_setting_value("ImageScale")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(1.0);

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

        use rayon::prelude::*;

        let pages = pdf.pages();
        let results: Result<(), String> = (0..page_count)
            .into_par_iter()
            .map(|index| {
                if index >= pages.len() {
                    return Ok(());
                }

                progress_callback(index as f32 / page_count as f32, "Rendering PDF page");

                let page = &pages[index];
                let mut local_cache = RenderCache::default();
                let pixmap = render(page, &mut local_cache, &interp_settings, &render_settings);

                let width = pixmap.width() as u32;
                let height = pixmap.height() as u32;

                let raw_bytes: &[u8] = bytemuck::cast_slice(pixmap.data());
                let buffer = image::ImageBuffer::from_raw(width, height, raw_bytes.to_vec())
                    .ok_or_else(|| "Failed to create ImageBuffer from PDF page".to_string())?;

                let img = DynamicImage::ImageRgba8(buffer);
                let output_file = &output_file_paths[index];

                save_image(&img, preset, output_file)?;
                Ok(())
            })
            .collect();

        results?;
        progress_callback(1.0, "Done");
    } else {
        progress_callback(0.0, "Loading Image");

        let file = std::fs::File::open(input_path)
            .map_err(|e| format!("Failed to open image file: {:?}", e))?;

        let mmap = unsafe { Mmap::map(&file) }
            .map_err(|e| format!("Failed to memory map input: {:?}", e))?;

        let mut img = if ext == "heic" || ext == "heif" {
            let output = DecoderConfig::new()
                .decode(&mmap, PixelLayout::Rgba8)
                .map_err(|e| format!("Failed to decode HEIC: {:?}", e))?;
            let buffer = image::ImageBuffer::from_raw(output.width, output.height, output.data)
                .ok_or_else(|| "Failed to parse HEIC buffer".to_string())?;
            DynamicImage::ImageRgba8(buffer)
        } else {
            image::load_from_memory(&mmap)
                .map_err(|e| format!("Failed to load image from memory map: {:?}", e))?
        };

        progress_callback(0.4, "Processing transforms");

        // Scale
        let scale_factor = preset
            .get_setting_value("ImageScale")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(1.0);
        if (scale_factor - 1.0).abs() >= 0.005 {
            let (w, h) = img.dimensions();
            let nw = (w as f32 * scale_factor) as u32;
            let nh = (h as f32 * scale_factor) as u32;
            img = resize_simd(&img, nw, nh)?;
        }

        // Rotation
        let rotation = preset
            .get_setting_value("ImageRotation")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.0);
        if (rotation - 0.0).abs() >= 0.05 {
            if (rotation - 90.0).abs() <= 0.05 {
                img = img.rotate90();
            } else if (rotation - 180.0).abs() <= 0.05 {
                img = img.rotate180();
            } else if (rotation - 270.0).abs() <= 0.05 {
                img = img.rotate270();
            }
        }

        // Clamps
        let clamp_power_2 = preset
            .get_setting_value("ImageClampSizePowerOf2")
            .and_then(|v| v.parse::<bool>().ok())
            .unwrap_or(false);
        let max_size = preset
            .get_setting_value("ImageMaximumSize")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0);

        if clamp_power_2 || max_size > 0 {
            let (w, h) = img.dimensions();
            let mut target_w = w;
            let mut target_h = h;

            if clamp_power_2 {
                let ref_size = std::cmp::min(w, h);
                let mut size = 2;
                while size * 2 <= ref_size {
                    size *= 2;
                }
                target_w = size;
                target_h = size;
            }

            if max_size > 0 {
                target_w = std::cmp::min(target_w, max_size);
                target_h = std::cmp::min(target_h, max_size);
            }

            img = resize_simd(&img, target_w, target_h)?;
        }

        progress_callback(0.7, "Saving Image");
        let output_file = &output_file_paths[0];
        save_image(&img, preset, output_file)?;

        progress_callback(1.0, "Done");
    }

    Ok(())
}

fn save_image(
    img: &DynamicImage,
    preset: &ConversionPreset,
    output_file: &str,
) -> Result<(), String> {
    let format = match preset.output_type {
        OutputType::Png => ImageFormat::Png,
        OutputType::Jpg => ImageFormat::Jpeg,
        OutputType::Gif => ImageFormat::Gif,
        OutputType::Webp => ImageFormat::WebP,
        OutputType::Avif => ImageFormat::Avif,
        OutputType::Ico => ImageFormat::Ico,
        OutputType::Pdf => ImageFormat::Png,
        _ => ImageFormat::Png,
    };

    let file = std::fs::File::create(output_file)
        .map_err(|e| format!("Failed to create output file {}: {:?}", output_file, e))?;
    let mut writer = std::io::BufWriter::with_capacity(128 * 1024, file);
    img.write_to(&mut writer, format)
        .map_err(|e| format!("Failed to save output image: {:?}", e))?;

    Ok(())
}
