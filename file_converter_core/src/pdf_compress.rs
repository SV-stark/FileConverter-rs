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

pub fn compress_pdf<P: AsRef<Path>>(
    input_path: P,
    output_path: P,
    options: &PdfCompressOptions,
) -> Result<()> {
    let mut doc = Document::load(input_path.as_ref()).map_err(|e| {
        FileConverterError::Invalid(format!("Failed to load PDF document: {:?}", e))
    })?;

    // Target max dimensions for a standard 8.5x11 inch page at target DPI
    let max_dimension = ((11.0 * options.target_dpi as f32) as u32).max(600);

    let mut resizer = Resizer::new();
    let resize_options = ResizeOptions::default();

    // Iterate through objects and process image XObject streams
    let object_ids: Vec<_> = doc.objects.keys().copied().collect();
    for id in object_ids {
        if let Ok(Object::Stream(stream)) = doc.get_object_mut(id) {
            let is_image = stream
                .dict
                .get(b"Subtype")
                .and_then(|obj| obj.as_name())
                .map(|name| name == b"Image")
                .unwrap_or(false);

            if !is_image {
                // Compress content stream if uncompressed
                if !stream.dict.has(b"Filter") {
                    let _ = stream.compress();
                }
                continue;
            }

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

            if width <= max_dimension && height <= max_dimension {
                continue;
            }

            // Attempt to decode and resize embedded image stream
            if let Ok(decompressed) = stream.decompressed_content()
                && let Ok(img) = image::load_from_memory(&decompressed)
            {
                let rgb_img = img.to_rgb8();
                let orig_w = rgb_img.width();
                let orig_h = rgb_img.height();

                if orig_w == 0 || orig_h == 0 {
                    continue;
                }

                let scale = (max_dimension as f32 / orig_w as f32)
                    .min(max_dimension as f32 / orig_h as f32);
                if scale >= 1.0 {
                    continue;
                }

                let new_w = ((orig_w as f32 * scale) as u32).max(1);
                let new_h = ((orig_h as f32 * scale) as u32).max(1);

                let src_image =
                    match Image::from_vec_u8(orig_w, orig_h, rgb_img.into_raw(), PixelType::U8x3) {
                        Ok(img) => img,
                        Err(_) => continue,
                    };

                let mut dst_image = Image::new(new_w, new_h, src_image.pixel_type());

                if resizer
                    .resize(&src_image, &mut dst_image, Some(&resize_options))
                    .is_err()
                {
                    continue;
                }

                // Re-encode resized image as JPEG
                let mut jpeg_buf = Vec::new();
                let mut cursor = Cursor::new(&mut jpeg_buf);
                let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
                    &mut cursor,
                    options.jpeg_quality,
                );

                if encoder
                    .write_image(
                        dst_image.buffer(),
                        new_w,
                        new_h,
                        image::ExtendedColorType::Rgb8,
                    )
                    .is_err()
                {
                    continue;
                }

                // Update PDF image stream properties
                stream.content = jpeg_buf;
                stream.dict.set("Width", new_w as i64);
                stream.dict.set("Height", new_h as i64);
                stream
                    .dict
                    .set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
                stream.dict.set("BitsPerComponent", 8);
                stream
                    .dict
                    .set("Filter", Object::Name(b"DCTDecode".to_vec()));
            }
        }
    }

    doc.prune_objects();
    doc.save(output_path.as_ref()).map_err(|e| {
        FileConverterError::Io(std::io::Error::other(format!(
            "Failed to save PDF: {:?}",
            e
        )))
    })?;

    Ok(())
}
