use crate::error::{FileConverterError, Result};
use fast_image_resize::images::Image;
use fast_image_resize::{PixelType, ResizeOptions, Resizer};
use image::ImageEncoder;
use lopdf::{Document, Object};
use std::io::Cursor;
use std::path::Path;

pub struct PdfCompressOptions {
    pub target_dpi: u32,
    pub jpeg_quality: u8,
}

impl Default for PdfCompressOptions {
    fn default() -> Self {
        Self {
            target_dpi: 150,
            jpeg_quality: 75,
        }
    }
}

use memmap2::Mmap;

pub fn compress_pdf<P: AsRef<Path>>(
    input_path: P,
    output_path: P,
    options: &PdfCompressOptions,
) -> Result<()> {
    let file = std::fs::File::open(input_path.as_ref()).map_err(FileConverterError::Io)?;
    let mmap = unsafe { Mmap::map(&file) }.map_err(FileConverterError::Io)?;

    let mut doc = Document::load_mem(&mmap).map_err(|e| {
        FileConverterError::Invalid(format!("Failed to load PDF document: {:?}", e))
    })?;

    if doc.is_encrypted() {
        return Err(FileConverterError::Invalid(
            "Cannot compress password-protected or encrypted PDF".to_string(),
        ));
    }

    // Target max dimensions for a standard 8.5x11 inch page at target DPI
    let max_w = ((8.5 * options.target_dpi as f32) as u32).max(600);
    let max_h = ((11.0 * options.target_dpi as f32) as u32).max(600);

    use rayon::prelude::*;

    // Collect candidate image streams for parallel processing
    let mut image_tasks = Vec::new();
    let object_ids: Vec<_> = doc.objects.keys().copied().collect();

    for id in &object_ids {
        if let Ok(Object::Stream(stream)) = doc.get_object(*id) {
            let is_image = stream
                .dict
                .get(b"Subtype")
                .and_then(|obj| obj.as_name())
                .map(|name| name == b"Image")
                .unwrap_or(false);

            let has_mask = stream.dict.has(b"SMask") || stream.dict.has(b"Mask");
            let is_supported_colorspace = match stream.dict.get(b"ColorSpace") {
                Ok(Object::Name(name)) => name == b"DeviceRGB",
                _ => false, // Preserve CMYK, Gray, and indexed color spaces without corrupting
            };

            if is_image && !has_mask && is_supported_colorspace {
                let width = stream
                    .dict
                    .get(b"Width")
                    .and_then(|obj| obj.as_i64())
                    .unwrap_or(0) as u32;

                let height = stream
                    .dict
                    .get(b"Height")
                    .and_then(|obj| obj.as_i64())
                    .unwrap_or(0) as u32;

                if (width > max_w || height > max_h)
                    && let Ok(decompressed) = stream.decompressed_content()
                {
                    image_tasks.push((*id, decompressed, width, height));
                }
            }
        }
    }

    struct CompressedImageTask {
        id: lopdf::ObjectId,
        content: Vec<u8>,
        width: u32,
        height: u32,
    }

    // Process and recompress images in parallel using Rayon
    let jpeg_quality = options.jpeg_quality;
    let processed_images: Vec<CompressedImageTask> = image_tasks
        .into_par_iter()
        .filter_map(|(id, decompressed, _w, _h)| {
            if let Ok(img) = image::load_from_memory(&decompressed) {
                let rgb_img = img.to_rgb8();
                let orig_w = rgb_img.width();
                let orig_h = rgb_img.height();

                if orig_w == 0 || orig_h == 0 {
                    return None;
                }

                let scale = (max_w as f32 / orig_w as f32).min(max_h as f32 / orig_h as f32);
                if scale >= 1.0 {
                    return None;
                }

                let new_w = ((orig_w as f32 * scale) as u32).max(1);
                let new_h = ((orig_h as f32 * scale) as u32).max(1);

                let src_image =
                    match Image::from_vec_u8(orig_w, orig_h, rgb_img.into_raw(), PixelType::U8x3) {
                        Ok(img) => img,
                        Err(_) => return None,
                    };

                let mut dst_image = Image::new(new_w, new_h, src_image.pixel_type());
                let mut local_resizer = Resizer::new();
                if local_resizer
                    .resize(&src_image, &mut dst_image, Some(&ResizeOptions::default()))
                    .is_err()
                {
                    return None;
                }

                let mut jpeg_buf = Vec::new();
                let mut cursor = Cursor::new(&mut jpeg_buf);
                let encoder =
                    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, jpeg_quality);

                if encoder
                    .write_image(
                        dst_image.buffer(),
                        new_w,
                        new_h,
                        image::ExtendedColorType::Rgb8,
                    )
                    .is_err()
                {
                    return None;
                }

                Some(CompressedImageTask {
                    id,
                    content: jpeg_buf,
                    width: new_w,
                    height: new_h,
                })
            } else {
                None
            }
        })
        .collect();

    // Apply compressed images back to doc
    for item in processed_images {
        if let Ok(Object::Stream(stream)) = doc.get_object_mut(item.id) {
            stream.content = item.content;
            stream.dict.set("Width", item.width as i64);
            stream.dict.set("Height", item.height as i64);
            stream
                .dict
                .set("Filter", Object::Name(b"DCTDecode".to_vec()));
            stream
                .dict
                .set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
            stream.dict.set("BitsPerComponent", 8i64);
        }
    }

    // Compress non-image content streams if uncompressed
    for id in object_ids {
        if let Ok(Object::Stream(stream)) = doc.get_object_mut(id) {
            let is_image = stream
                .dict
                .get(b"Subtype")
                .and_then(|obj| obj.as_name())
                .map(|name| name == b"Image")
                .unwrap_or(false);

            if !is_image && !stream.dict.has(b"Filter") {
                let _ = stream.compress();
            }
        }
    }

    let is_same_file = input_path.as_ref() == output_path.as_ref();
    let temp_save_path = if is_same_file {
        let parent = output_path
            .as_ref()
            .parent()
            .unwrap_or_else(|| Path::new("."));
        let temp_file = tempfile::Builder::new()
            .prefix("fc_pdf_")
            .suffix(".pdf")
            .tempfile_in(parent)
            .map_err(FileConverterError::Io)?;
        Some(temp_file.into_temp_path())
    } else {
        None
    };

    let save_target = if let Some(ref tp) = temp_save_path {
        tp.as_ref()
    } else {
        output_path.as_ref()
    };

    doc.prune_objects();
    doc.save(save_target).map_err(|e| {
        FileConverterError::Io(std::io::Error::other(format!(
            "Failed to save PDF: {:?}",
            e
        )))
    })?;

    // Drop mmap & file before atomic rename
    drop(doc);
    drop(mmap);
    drop(file);

    if let Some(tp) = temp_save_path {
        tp.persist(output_path.as_ref()).map_err(|e| {
            FileConverterError::Io(std::io::Error::other(format!(
                "Failed to persist compressed PDF: {:?}",
                e
            )))
        })?;
    }

    Ok(())
}
