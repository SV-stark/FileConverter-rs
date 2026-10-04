use file_converter_core::settings::{ConversionPreset, Settings};
use file_converter_core::types::{HardwareAccelerationMode, InputPostConversionAction, OutputType};

fn preset(name: &str, output_type: OutputType, inputs: &[&str]) -> ConversionPreset {
    ConversionPreset {
        name: name.to_string(),
        output_type,
        output_file_name_template: "(p)(f)".to_string(),
        is_default_settings: true,
        input_types: inputs.iter().map(|s| (*s).into()).collect(),
        input_post_conversion_action: InputPostConversionAction::None,
        settings: vec![],
    }
}

/// Creates a unique temp path inside the OS temp directory.
fn temp_path(name: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    std::env::temp_dir().join(format!(
        "fc_test_{}_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed),
        name
    ))
}

/// A real file so `ConversionJob::validate` (which requires an existing input)
/// accepts it.
fn touch_temp(name: &str, contents: &[u8]) -> String {
    let path = temp_path(name);
    std::fs::write(&path, contents).expect("write temp fixture");
    path.to_string_lossy().to_string()
}

#[test]
fn test_default_settings_xml_parses_and_covers_every_output_type() {
    let xml = include_str!("../../Settings.default.xml");
    let settings = Settings::load_from_str(xml).expect("default settings must parse");
    assert!(!settings.conversion_presets.is_empty());

    // Every preset must round-trip through the new enum variants.
    for p in &settings.conversion_presets {
        assert!(
            !p.name.is_empty(),
            "preset with empty name: {:?}",
            p.output_type
        );
    }
}

#[test]
fn test_settings_save_and_reload_roundtrip() {
    let xml_path = temp_path("settings_roundtrip.xml");
    let mut settings = Settings::load_from_str(include_str!("../../Settings.default.xml")).unwrap();
    settings.maximum_number_of_simultaneous_conversions = 8;
    settings.auto_start_on_file_drop = false;
    settings.copy_files_in_clipboard_after_conversion = true;
    // Unset: must not emit an empty element, which quick-xml would then fail to
    // read back as a boolean and make the whole file unloadable.
    settings.dark_mode = None;
    settings.save_to_file(&xml_path).expect("save settings");

    let reloaded = Settings::load_from_file(&xml_path).expect("reload settings");
    assert_eq!(reloaded.maximum_number_of_simultaneous_conversions, 8);
    assert!(!reloaded.auto_start_on_file_drop);
    assert!(reloaded.copy_files_in_clipboard_after_conversion);
    assert_eq!(reloaded.dark_mode, None);

    // An explicit choice must survive the round-trip in both directions.
    for choice in [Some(true), Some(false)] {
        settings.dark_mode = choice;
        settings.save_to_file(&xml_path).expect("save settings");
        let reloaded = Settings::load_from_file(&xml_path).expect("reload settings");
        assert_eq!(
            reloaded.dark_mode, choice,
            "theme preference {choice:?} did not round-trip"
        );
    }

    let _ = std::fs::remove_file(xml_path);
}

#[test]
fn test_preset_setting_lookup_and_mutation() {
    let mut p = preset("Test Preset", OutputType::Mp3, &["wav", "flac"]);
    assert_eq!(p.get_setting_value("AudioBitRate"), None);
    p.set_setting_value("AudioBitRate", "320k");
    assert_eq!(p.get_setting_value("AudioBitRate"), Some("320k"));
    p.set_setting_value("AudioBitRate", "192k");
    assert_eq!(p.get_setting_value("AudioBitRate"), Some("192k"));
}

#[test]
fn test_path_template_replacements() {
    use file_converter_core::path_helpers::generate_file_path_from_template;
    let input = "C:\\Music\\Album\\track1.flac";
    assert_eq!(
        generate_file_path_from_template(input, "mp3", "(p)(f)", 1, 1),
        "C:\\Music\\Album\\track1.mp3"
    );
    assert_eq!(
        generate_file_path_from_template(input, "mp3", "(p)(d0) - (f)", 1, 1),
        "C:\\Music\\Album\\Album - track1.mp3"
    );
    assert_eq!(
        generate_file_path_from_template(input, "mp3", "(p)(F)_(O)", 1, 1),
        "C:\\Music\\Album\\TRACK1_MP3.mp3"
    );
    assert_eq!(
        generate_file_path_from_template(
            "C:\\Music\\Rock\\Track01.flac",
            "mp3",
            "(p)(f)_(n:i)of(n:c)",
            3,
            10
        ),
        "C:\\Music\\Rock\\Track01_3of10.mp3"
    );
}

#[test]
fn test_engine_routing_covers_every_category() {
    use file_converter_core::scheduler::{JobEngine, determine_job_engine};

    let wav = touch_temp("in.wav", b"RIFF");
    let txt = touch_temp("in.txt", b"hello");
    let png = touch_temp("in.png", &[0x89, b'P', b'N', b'G']);

    let to_mp3 = preset("To Mp3", OutputType::Mp3, &["wav", "mp3"]);
    assert_eq!(determine_job_engine(&to_mp3, &wav), JobEngine::Ffmpeg);

    // Text documents must NOT be routed to the image engine any more.
    let to_pdf = preset("To Pdf", OutputType::Pdf, &["txt", "html", "png"]);
    assert_eq!(determine_job_engine(&to_pdf, &txt), JobEngine::TextDocument);
    assert_eq!(determine_job_engine(&to_pdf, &png), JobEngine::Image);

    let to_gif = preset("To Gif", OutputType::Gif, &["png", "gif"]);
    assert_eq!(determine_job_engine(&to_gif, &png), JobEngine::Gif);

    let to_ico = preset("To Ico", OutputType::Ico, &["png"]);
    assert_eq!(determine_job_engine(&to_ico, &png), JobEngine::Ico);

    // OxiPNG must only be used for real PNG containers.
    let mut compress = preset("Compress Png (lossless)", OutputType::Png, &["png"]);
    compress.set_setting_value("OxipngLossless", "True");
    assert_eq!(determine_job_engine(&compress, &png), JobEngine::Oxipng);
    let jpg = touch_temp("in.jpg", &[0xFF, 0xD8]);
    assert_eq!(determine_job_engine(&compress, &jpg), JobEngine::Image);

    for path in [wav, txt, png, jpg] {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn test_job_prepare_rejects_preset_input_mismatch() {
    use file_converter_core::scheduler::{ConversionJob, JobStatus};
    use file_converter_core::types::HardwareAccelerationMode;

    // "Compress Png (lossless)" only declares PNG inputs.
    let mut compress = preset("Compress Png (lossless)", OutputType::Png, &["png"]);
    compress.set_setting_value("OxipngLossless", "True");

    let jpg = touch_temp("mismatch.jpg", &[0xFF, 0xD8, 0xFF]);
    let mut job = ConversionJob::new(1, compress.clone(), jpg.clone());
    let err = job.prepare(0, 1).expect_err("jpg must be rejected");
    assert!(err.to_string().contains("does not support"), "{err}");

    // Running the job must surface the preparation error verbatim.
    job.run(HardwareAccelerationMode::Off);
    let status = job.status.lock().clone();
    assert!(matches!(status, JobStatus::Failed(ref m) if m.contains("does not support")));
    let _ = std::fs::remove_file(jpg);

    // The matching PNG input is accepted.
    let png = touch_temp("match.png", &[0x89, b'P', b'N', b'G']);
    let mut job = ConversionJob::new(1, compress, png.clone());
    job.prepare(0, 1).expect("png must be accepted");
    assert_eq!(job.output_file_paths.len(), 1);
    let _ = std::fs::remove_file(png);
}

#[test]
fn test_job_run_reports_missing_outputs_as_failure() {
    use file_converter_core::scheduler::{ConversionJob, JobStatus};
    use file_converter_core::types::HardwareAccelerationMode;

    let png = touch_temp(
        "ghost.png",
        &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
    );
    let job = ConversionJob::new(1, preset("To Png", OutputType::Png, &["png"]), png.clone());
    job.run(HardwareAccelerationMode::Off);
    let status = job.status.lock().clone();
    assert!(matches!(status, JobStatus::Failed(_)), "{status:?}");
    let _ = std::fs::remove_file(png);
}

#[test]
fn test_image_clamp_preserves_aspect_ratio() {
    use file_converter_core::image::{get_image_dimensions, run_image_conversion};

    let src = temp_path("clamp_src.png");
    let out = temp_path("clamp_out.png");
    image::DynamicImage::new_rgba8(300, 150).save(&src).unwrap();

    let mut p = preset("To Png (clamped)", OutputType::Png, &["png"]);
    p.set_setting_value("ImageClampSizePowerOf2", "True");

    run_image_conversion(
        &p,
        src.to_str().unwrap(),
        &[out.to_string_lossy().to_string()],
        &|_p, _m| {},
    )
    .unwrap();

    // 300x150 must round down to 256x128 - not be forced into a 128x128 square.
    assert_eq!(
        get_image_dimensions(out.to_str().unwrap()).unwrap(),
        (256, 128)
    );

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn test_ico_is_multiresolution_and_keeps_aspect_ratio() {
    use file_converter_core::image::run_image_conversion;

    let src = temp_path("ico_src.png");
    let out = temp_path("ico_out.ico");
    image::DynamicImage::new_rgba8(512, 256).save(&src).unwrap();

    let p = preset("To Ico", OutputType::Ico, &["png"]);
    run_image_conversion(
        &p,
        src.to_str().unwrap(),
        &[out.to_string_lossy().to_string()],
        &|_p, _m| {},
    )
    .unwrap();

    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(&bytes[0..4], &[0, 0, 1, 0], "ICONDIR magic");
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    assert!(count >= 4, "expected several icon resolutions, got {count}");

    let mut last: Option<(u32, u32)> = None;
    for i in 0..count {
        let off = 6 + i * 16;
        // 0 is stored as 256.
        let w = if bytes[off] == 0 {
            256
        } else {
            bytes[off] as u32
        };
        let h = if bytes[off + 1] == 0 {
            256
        } else {
            bytes[off + 1] as u32
        };
        if let Some((prev_w, prev_h)) = last {
            assert!(w >= prev_w && h >= prev_h, "sizes must be ascending");
            assert_eq!(w as f64 / h as f64, 2.0, "aspect ratio must be 2:1");
        }
        last = Some((w, h));
    }
    assert_eq!(
        last,
        Some((256, 128)),
        "largest ICO entry is capped at 256px"
    );

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn test_still_image_to_gif_without_ffmpeg() {
    use file_converter_core::image::run_still_gif_conversion;

    let src = temp_path("gif_src.png");
    let out = temp_path("gif_out.gif");
    image::DynamicImage::new_rgb8(64, 48).save(&src).unwrap();

    let mut p = preset("To Gif", OutputType::Gif, &["png"]);
    p.set_setting_value("ImageScale", "1");

    run_still_gif_conversion(
        &p,
        src.to_str().unwrap(),
        out.to_str().unwrap(),
        &|_p, _m| {},
    )
    .unwrap();

    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(&bytes[0..6], b"GIF89a");
    assert_eq!(
        u16::from_le_bytes([bytes[6], bytes[7]]) as u32,
        64,
        "GIF width"
    );
    assert_eq!(
        u16::from_le_bytes([bytes[8], bytes[9]]) as u32,
        48,
        "GIF height"
    );

    let _ = std::fs::remove_file(src);
    let _ = std::fs::remove_file(out);
}

#[test]
fn test_animated_detection() {
    use file_converter_core::image::source_is_animated;

    let still = temp_path("still.webp");
    std::fs::write(&still, b"RIFF____WEBPVP8 \x00\x00\x00\x00").unwrap();
    assert!(!source_is_animated(still.to_str().unwrap()));

    let anim = temp_path("anim.webp");
    // RIFF | size | WEBP | VP8X | chunk-size (4) | flags (ANIM bit at +20)
    let mut header: Vec<u8> = Vec::new();
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&[0u8; 4]);
    header.extend_from_slice(b"WEBP");
    header.extend_from_slice(b"VP8X");
    header.extend_from_slice(&[10u8, 0, 0, 0]);
    header.push(0x02);
    header.extend_from_slice(&[0u8; 20]);
    std::fs::write(&anim, header).unwrap();
    assert!(source_is_animated(anim.to_str().unwrap()));

    let _ = std::fs::remove_file(still);
    let _ = std::fs::remove_file(anim);
}

#[test]
fn test_text_document_conversions() {
    use file_converter_core::doc_convert::run_text_document_conversion;

    let src = touch_temp(
        "doc.txt",
        b"Hello File Converter\nSecond line of body text.\n",
    );

    // txt -> txt
    let txt_out = temp_path("doc_out.txt");
    run_text_document_conversion(
        &src,
        txt_out.to_str().unwrap(),
        OutputType::Txt,
        &|_p, _m| {},
    )
    .unwrap();
    assert!(
        std::fs::read_to_string(&txt_out)
            .unwrap()
            .contains("Hello File Converter")
    );

    // txt -> pdf
    let pdf_out = temp_path("doc_out.pdf");
    run_text_document_conversion(
        &src,
        pdf_out.to_str().unwrap(),
        OutputType::Pdf,
        &|_p, _m| {},
    )
    .unwrap();
    assert!(std::fs::read(&pdf_out).unwrap().starts_with(b"%PDF-"));

    // txt -> png (rasterised document page, previously an "unknown format" error)
    let png_out = temp_path("doc_out.png");
    run_text_document_conversion(
        &src,
        png_out.to_str().unwrap(),
        OutputType::Png,
        &|_p, _m| {},
    )
    .unwrap();
    assert_eq!(
        file_converter_core::image::get_image_dimensions(png_out.to_str().unwrap()).unwrap(),
        (1240, 1754)
    );

    for p in [txt_out, pdf_out, png_out] {
        let _ = std::fs::remove_file(p);
    }
    let _ = std::fs::remove_file(src);
}

#[test]
fn test_html_document_is_stripped_and_entity_decoded() {
    use file_converter_core::doc_convert::run_text_document_conversion;

    let src = touch_temp(
        "doc.html",
        b"<html><body><p>Caf&eacute; &amp; bar</p><p>Line two</p></body></html>",
    );
    let out = temp_path("doc_out2.txt");
    run_text_document_conversion(&src, out.to_str().unwrap(), OutputType::Txt, &|_p, _m| {})
        .unwrap();

    let text = std::fs::read_to_string(&out).unwrap();
    assert!(text.contains("Café & bar"), "{text}");
    assert!(text.contains("Line two"), "{text}");
    assert!(!text.contains('<'), "{text}");

    let _ = std::fs::remove_file(out);
    let _ = std::fs::remove_file(src);
}

#[test]
fn test_markdown_output_types_are_honoured() {
    use file_converter_core::doc_convert::run_markdown_conversion;

    let src = touch_temp("readme.md", b"# Title\n\nSome **bold** text.\n");

    let png_out = temp_path("readme_out.png");
    run_markdown_conversion(
        &src,
        png_out.to_str().unwrap(),
        OutputType::Png,
        &|_p, _m| {},
    )
    .unwrap();
    // Must be a PNG, not an HTML document with a .png extension.
    assert!(std::fs::read(&png_out).unwrap().starts_with(b"\x89PNG"));

    let txt_out = temp_path("readme_out.txt");
    run_markdown_conversion(
        &src,
        txt_out.to_str().unwrap(),
        OutputType::Txt,
        &|_p, _m| {},
    )
    .unwrap();
    let text = std::fs::read_to_string(&txt_out).unwrap();
    assert!(text.contains("Title") && text.contains("bold"), "{text}");

    let _ = std::fs::remove_file(png_out);
    let _ = std::fs::remove_file(txt_out);
    let _ = std::fs::remove_file(src);
}

#[test]
fn test_unsupported_raw_image_reports_actionable_error() {
    use file_converter_core::image::run_image_conversion;

    let src = touch_temp("photo.cr2", &[0u8; 64]);
    let out = temp_path("photo_out.png");
    let err = run_image_conversion(
        &preset("To Png", OutputType::Png, &["cr2"]),
        &src,
        &[out.to_string_lossy().to_string()],
        &|_p, _m| {},
    )
    .expect_err("raw formats must be rejected clearly");
    assert!(err.to_string().contains("dedicated decoder"), "{err}");

    let _ = std::fs::remove_file(src);
}

#[test]
fn test_preset_applicability_matches_ui_filter() {
    use file_converter_core::types::is_preset_applicable_to_file;

    // Declared input list is honoured.
    assert!(is_preset_applicable_to_file(
        OutputType::Mp3,
        &["wav".into(), "flac".into()],
        "C:\\Music\\song.wav"
    ));
    assert!(!is_preset_applicable_to_file(
        OutputType::Mp3,
        &["wav".into()],
        "C:\\Music\\song.png"
    ));
    // Wildcard and empty lists accept anything compatible with the category.
    assert!(is_preset_applicable_to_file(
        OutputType::Mp3,
        &["*".into()],
        "C:\\Music\\song.flac"
    ));
    // Category guard still rejects impossible combinations.
    assert!(!is_preset_applicable_to_file(
        OutputType::Mp3,
        &[],
        "C:\\x\\file.png"
    ));
    assert!(is_preset_applicable_to_file(
        OutputType::Pdf,
        &[],
        "C:\\x\\file.png"
    ));
}

#[test]
fn test_output_type_classification() {
    use file_converter_core::types::get_extension_category;

    assert!(OutputType::Txt.is_textual_document());
    assert!(OutputType::Html.is_textual_document());
    assert!(!OutputType::Pdf.is_textual_document());
    assert!(OutputType::Png.is_raster_image());
    assert!(OutputType::Jxl.is_raster_image());
    assert!(!OutputType::Mp3.is_raster_image());
    assert_eq!(OutputType::Txt.extension(), "txt");
    assert_eq!(OutputType::Html.extension(), "html");

    assert_eq!(get_extension_category("log"), get_extension_category("txt"));
    assert_eq!(
        get_extension_category("csv"),
        file_converter_core::types::FileCategory::Document
    );
    assert!(file_converter_core::types::is_text_document_extension(
        "html"
    ));
    assert!(!file_converter_core::types::is_text_document_extension(
        "png"
    ));
    assert!(file_converter_core::types::is_ebook_extension("epub"));
    assert!(file_converter_core::types::is_ebook_extension("cbz"));
}

#[test]
fn test_ffmpeg_gif_palette_pass_does_not_use_fps() {
    use file_converter_core::ffmpeg::get_ffmpeg_passes;

    let mut p = preset("To Gif (low quality)", OutputType::Gif, &["png", "mp4"]);
    p.set_setting_value("VideoScale", "0.75");
    p.set_setting_value("VideoFramesPerSecond", "10");

    let passes = get_ffmpeg_passes(
        &p,
        "C:\\in.mp4",
        "C:\\out.gif",
        HardwareAccelerationMode::Off,
    )
    .unwrap();
    assert_eq!(
        passes.len(),
        2,
        "GIF needs a palette pass and a conversion pass"
    );

    // Pass 1: single palette frame, no fps filter (fps+scale on a single-frame
    // source can emit an empty palette file).
    assert!(
        !passes[0].arguments.iter().any(|a| a.contains("fps")),
        "palette pass must not use fps: {:?}",
        passes[0].arguments
    );
    assert!(passes[0].arguments.iter().any(|a| a.contains("palettegen")));
    assert!(passes[0].arguments.iter().any(|a| a == "-frames:v"));

    // Pass 2: explicit filter_complex so the palette frame is not rescaled.
    assert!(passes[1].arguments.iter().any(|a| a == "-filter_complex"));
    assert!(passes[1].arguments.iter().any(|a| a.contains("paletteuse")));
    assert!(
        passes[1].file_to_delete.is_some(),
        "palette must be cleaned up"
    );
}

#[test]
fn test_ffmpeg_video_outputs_force_even_dimensions_and_pix_fmt() {
    use file_converter_core::ffmpeg::{ffmpeg_has_encoder, get_ffmpeg_passes};

    for output in [OutputType::Webm, OutputType::Ogv, OutputType::Avi] {
        let p = preset("To X", output, &["mp4"]);
        let passes = get_ffmpeg_passes(&p, "C:\\in.mp4", "C:\\out", HardwareAccelerationMode::Off);

        if output == OutputType::Ogv && !ffmpeg_has_encoder("libtheora") {
            // Reported separately by `test_ffmpeg_missing_encoder_reports_clear_error`.
            assert!(passes.is_err());
            continue;
        }

        let passes = passes.unwrap();
        let args = &passes[0].arguments;
        assert!(
            args.iter().any(|a| a.contains("trunc(iw*")),
            "{output:?} must force even dimensions: {args:?}"
        );
        assert!(
            args.iter().any(|a| a == "yuv420p") && args.iter().any(|a| a == "-pix_fmt"),
            "{output:?} must request yuv420p: {args:?}"
        );
    }
}

#[test]
fn test_ffmpeg_missing_encoder_reports_clear_error() {
    use file_converter_core::error::FileConverterError;
    use file_converter_core::ffmpeg::{ffmpeg_has_encoder, get_ffmpeg_passes};

    let p = preset("To Ogv", OutputType::Ogv, &["mp4"]);
    let passes = get_ffmpeg_passes(
        &p,
        "C:\\in.mp4",
        "C:\\out.ogv",
        HardwareAccelerationMode::Off,
    );

    if ffmpeg_has_encoder("libtheora") {
        assert!(passes.is_ok());
    } else {
        let err = passes.expect_err("must fail without libtheora");
        assert!(matches!(err, FileConverterError::Ffmpeg(_)));
        assert!(err.to_string().contains("libtheora"), "{err}");
    }
}

#[test]
fn test_ffmpeg_custom_command_and_tokenizer() {
    use file_converter_core::ffmpeg::{get_ffmpeg_passes, tokenize_command};

    let cmd = r#"-vf "scale=1920:1080,fps=30" -c:v libx264 -preset veryfast"#;
    assert_eq!(
        tokenize_command(cmd),
        vec![
            "-vf",
            "scale=1920:1080,fps=30",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast"
        ]
    );

    let mut p = preset("Custom MP4", OutputType::Mp4, &["mkv"]);
    p.set_setting_value("EnableFFMPEGCustomCommand", "true");
    p.set_setting_value("FFMPEGCustomCommand", "-c:v copy -c:a copy");

    let passes = get_ffmpeg_passes(
        &p,
        "C:\\in.mkv",
        "C:\\out.mp4",
        HardwareAccelerationMode::Off,
    )
    .unwrap();
    assert_eq!(passes.len(), 1);
    assert!(passes[0].arguments.contains(&"-c:v".to_string()));
    assert!(passes[0].arguments.contains(&"copy".to_string()));
}

#[test]
fn test_ffmpeg_quality_mappings() {
    use file_converter_core::ffmpeg::{
        h264_encoding_speed_to_amf_quality, h264_encoding_speed_to_nvenc_preset,
        h264_encoding_speed_to_preset, mp3_vbr_bitrate_to_quality_index,
        ogg_vbr_bitrate_to_quality_index,
    };
    use file_converter_core::types::VideoEncodingSpeed;

    assert_eq!(
        h264_encoding_speed_to_preset(VideoEncodingSpeed::UltraFast),
        "ultrafast"
    );
    assert_eq!(
        h264_encoding_speed_to_nvenc_preset(VideoEncodingSpeed::VerySlow),
        "p7"
    );
    assert_eq!(
        h264_encoding_speed_to_amf_quality(VideoEncodingSpeed::VerySlow),
        "quality"
    );

    assert_eq!(mp3_vbr_bitrate_to_quality_index(320).unwrap(), 0);
    assert_eq!(mp3_vbr_bitrate_to_quality_index(190).unwrap(), 2);
    assert_eq!(mp3_vbr_bitrate_to_quality_index(64).unwrap(), 9);

    assert_eq!(ogg_vbr_bitrate_to_quality_index(500).unwrap(), 10);
    assert_eq!(ogg_vbr_bitrate_to_quality_index(48).unwrap(), -1);
}

#[test]
fn test_pdf_compress_options_defaults() {
    let opts = file_converter_core::pdf_compress::PdfCompressOptions::default();
    assert_eq!(opts.target_dpi, 150);
    assert_eq!(opts.jpeg_quality, 75);
}

#[test]
fn test_strip_html_tags() {
    use file_converter_core::doc_convert::strip_html_tags;

    assert_eq!(
        strip_html_tags(
            r#"<p>Welcome to <b>File Converter</b>! <a href="x">Click</a> for more.</p>"#
        ),
        "Welcome to File Converter! Click for more."
    );
    assert_eq!(strip_html_tags(""), "");
    assert_eq!(strip_html_tags("Just plain text."), "Just plain text.");
}

#[test]
fn test_paged_output_reports_the_real_total() {
    use file_converter_core::doc_convert::create_pdf_from_text;
    use file_converter_core::scheduler::ConversionJob;

    // A multi-page PDF converted with a `(n:i) of (n:c)` template must number
    // the pages "1 of N" ... "N of N" (previously every page said "of 1").
    let pdf = temp_path("paged.pdf");
    let text = (1..=200)
        .map(|i| format!("Line {i} of the generated PDF body."))
        .collect::<Vec<_>>()
        .join("\n");
    create_pdf_from_text("Paged", &text, pdf.to_str().unwrap()).unwrap();
    let pages = file_converter_core::image::get_pdf_page_count(pdf.to_str().unwrap()).unwrap();
    assert!(pages >= 2, "fixture must span several pages, got {pages}");

    let mut p = preset("To Png (paged)", OutputType::Png, &["pdf"]);
    p.output_file_name_template = "(p)(f) (n:i) of (n:c)".to_string();

    let mut job = ConversionJob::new(1, p, pdf.to_string_lossy().to_string());
    job.prepare(0, 1).expect("prepare must succeed");

    assert_eq!(job.output_file_paths.len(), pages);
    for (i, path) in job.output_file_paths.iter().enumerate() {
        let name = std::path::Path::new(path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert!(
            name.ends_with(&format!("{} of {}.png", i + 1, pages)),
            "unexpected page name: {name}"
        );
    }

    let _ = std::fs::remove_file(pdf);
}

#[test]
fn test_embedded_default_presets_match_the_packaged_file() {
    // Both binaries must embed the *repository* copy. A crate-local duplicate
    // used to silently ship a stale preset set (76 instead of 79 presets).
    let packaged = include_str!("../../Settings.default.xml");
    let settings = Settings::load_from_str(packaged).expect("packaged defaults must parse");
    assert!(
        settings
            .conversion_presets
            .iter()
            .any(|p| p.name == "To Png (from Text/Markdown/EPUB)"),
        "packaged defaults must include the text-document presets"
    );
    assert!(
        settings
            .conversion_presets
            .iter()
            .any(|p| p.name == "To Jpg (from Text/Markdown/EPUB)"),
    );
    assert!(
        settings
            .conversion_presets
            .iter()
            .any(|p| p.name == "To Html (from EPUB/Markdown/Text)"),
    );
    // The text preset must no longer claim to produce a PDF.
    let to_text = settings
        .conversion_presets
        .iter()
        .find(|p| p.name == "To Text (from EPUB/Markdown)")
        .expect("text preset");
    assert_eq!(to_text.output_type, OutputType::Txt);
}

#[test]
fn test_settings_merge_adopts_only_new_presets() {
    let mut user = Settings::load_from_str(include_str!("../../Settings.default.xml")).unwrap();
    let total = user.conversion_presets.len();

    // A preset the user customised must not be replaced by the shipped default.
    if let Some(p) = user
        .conversion_presets
        .iter_mut()
        .find(|p| p.name == "To Pdf")
    {
        p.output_file_name_template = "(p)(f) MINE".to_string();
    }

    let mut defaults = Settings::load_from_str(include_str!("../../Settings.default.xml")).unwrap();
    defaults
        .conversion_presets
        .push(preset("Brand New Preset", OutputType::Jpg, &["png"]));

    user.merge(defaults);
    assert_eq!(user.conversion_presets.len(), total + 1);
    assert!(
        user.conversion_presets
            .iter()
            .any(|p| p.name == "Brand New Preset")
    );
    let to_pdf = user
        .conversion_presets
        .iter()
        .find(|p| p.name == "To Pdf")
        .unwrap();
    assert_eq!(to_pdf.output_file_name_template, "(p)(f) MINE");
}

#[test]
fn test_cancelled_job_is_not_reported_as_failed() {
    use file_converter_core::scheduler::{ConversionJob, JobStatus};
    use file_converter_core::types::HardwareAccelerationMode;

    // A job cancelled before it runs must end as `Canceled`, and must not leave
    // output files behind.
    let png = touch_temp(
        "cancelled.png",
        &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
    );
    let mut job = ConversionJob::new(1, preset("To Png", OutputType::Png, &["png"]), png.clone());
    job.prepare(0, 1).expect("prepare");
    job.cancel();
    assert!(job.is_cancelled());

    job.run(HardwareAccelerationMode::Off);
    assert_eq!(*job.status.lock(), JobStatus::Canceled);

    for path in &job.output_file_paths {
        assert!(
            !std::path::Path::new(path).exists(),
            "cancelled job must not leave {path}"
        );
    }
    let _ = std::fs::remove_file(png);
}

#[test]
fn test_preset_input_types_are_enforced_end_to_end() {
    use file_converter_core::scheduler::{ConversionJob, JobStatus};
    use file_converter_core::types::HardwareAccelerationMode;

    // The shell extension filters the menu with exactly this predicate, so a
    // mismatch here means the menu offers a preset the scheduler rejects.
    let mut png_compressor = preset("Compress Png (lossless)", OutputType::Png, &["png"]);
    png_compressor.set_setting_value("OxipngLossless", "True");

    let declared: Vec<String> = png_compressor
        .input_types
        .iter()
        .map(|s| s.to_string())
        .collect();
    for file in [
        "C:\\a\\x.wav",
        "C:\\a\\x.png",
        "C:\\a\\x.pdf",
        "C:\\a\\x.docx",
    ] {
        let applicable = file_converter_core::types::is_preset_applicable_to_file(
            png_compressor.output_type,
            &declared,
            file,
        );
        // Category compatibility is checked before the declared list, so an
        // audio file is rejected on category grounds and a pdf on the declared list.
        assert_eq!(
            applicable,
            file.ends_with("png"),
            "unexpected applicability for {file}"
        );
    }

    // A job whose prepare failed must surface the reason, not a generic error.
    let wav = touch_temp("declared.wav", b"RIFF");
    let mut job = ConversionJob::new(1, png_compressor, wav.clone());
    assert!(job.prepare(0, 1).is_err());
    assert!(job.preparation_error.is_some());
    job.run(HardwareAccelerationMode::Off);
    let status = job.status.lock().clone();
    assert!(matches!(status, JobStatus::Failed(_)), "{status:?}");
    let _ = std::fs::remove_file(wav);
}

#[test]
fn test_preset_setting_descriptors_cover_every_output_type() {
    use file_converter_core::types::{OutputType, preset_setting_keys};

    // Every output type must offer at least the Advanced group, otherwise the
    // editor renders an empty card and the user cannot set anything at all.
    for ot in [
        OutputType::Aac,
        OutputType::Avi,
        OutputType::Flac,
        OutputType::Jpg,
        OutputType::Mp3,
        OutputType::Mp4,
        OutputType::Pdf,
        OutputType::Png,
        OutputType::Wav,
        OutputType::None,
    ] {
        let keys = preset_setting_keys(ot);
        assert!(!keys.is_empty(), "{ot:?} has no setting descriptors");
        assert!(
            keys.iter().any(|k| k.key == "FFMPEGCustomCommand"),
            "{ot:?} is missing the Advanced escape hatch"
        );
        assert!(
            keys.iter().all(|k| !k.hint.is_empty()),
            "{ot:?} has a descriptor without a hint"
        );
        // Keys must be unique or the editor would render duplicate rows that
        // fight over the same underlying setting.
        let mut seen: Vec<&str> = keys.iter().map(|k| k.key).collect();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(before, seen.len(), "{ot:?} has duplicate descriptor keys");
    }

    // Format-specific keys must not leak into unrelated formats: a stale
    // `JpegQuality` on an audio preset would be silently ignored by the engine.
    let mp3: Vec<&str> = preset_setting_keys(OutputType::Mp3)
        .iter()
        .map(|k| k.key)
        .collect();
    assert!(mp3.contains(&"AudioEncodingMode"));
    assert!(!mp3.contains(&"JpegQuality"));

    let jpg: Vec<&str> = preset_setting_keys(OutputType::Jpg)
        .iter()
        .map(|k| k.key)
        .collect();
    assert!(jpg.contains(&"JpegQuality"));
    assert!(!jpg.contains(&"AudioEncodingMode"));

    // Slint 1.9 has no wrapping layout, so long choice lists would overflow the
    // pane; `preset_setting_keys` demotes them to free text.
    for ot in [
        OutputType::Mp4,
        OutputType::Png,
        OutputType::Wav,
        OutputType::Jpg,
    ] {
        for k in preset_setting_keys(ot) {
            assert!(
                k.choices.len() <= 4,
                "{ot:?}/{} has {} choices and would overflow the editor",
                k.key,
                k.choices.len()
            );
        }
    }
}

#[test]
fn test_ui_post_action_strings_are_all_parseable() {
    // Regression guard: the UI used to send "Recycle", which is not an
    // `InputPostConversionAction` variant, so the button silently did nothing.
    // Every string the .slint can emit must parse.
    for s in ["None", "MoveInArchiveFolder", "Delete"] {
        assert!(
            s.parse::<file_converter_core::types::InputPostConversionAction>()
                .is_ok(),
            "UI sends {s:?} but it does not parse"
        );
    }
    // And the raw variant names must round-trip for the enum-name comparison the
    // UI highlight relies on.
    for action in [
        file_converter_core::types::InputPostConversionAction::None,
        file_converter_core::types::InputPostConversionAction::MoveInArchiveFolder,
        file_converter_core::types::InputPostConversionAction::Delete,
    ] {
        let debug = format!("{action:?}");
        assert_eq!(
            debug.parse::<file_converter_core::types::InputPostConversionAction>(),
            Ok(action),
            "{debug:?} does not round-trip"
        );
    }
}

#[test]
fn test_remove_setting_distinguishes_absent_from_empty() {
    let mut p = preset("To Mp3", OutputType::Mp3, &["wav"]);
    p.set_setting_value("AudioBitrate", "192");
    assert_eq!(p.get_setting_value("AudioBitrate"), Some("192"));

    assert!(p.remove_setting("AudioBitrate"));
    assert_eq!(p.get_setting_value("AudioBitrate"), None);
    assert!(!p.settings.iter().any(|s| s.key == "AudioBitrate"));

    // Removing something absent reports false so the UI can say so.
    assert!(!p.remove_setting("AudioBitrate"));

    // An empty value is *not* the same as removal: it means "use the default"
    // but must still round-trip through the XML.
    p.set_setting_value("AudioBitrate", "");
    assert_eq!(p.get_setting_value("AudioBitrate"), Some(""));
    assert!(p.remove_setting("AudioBitrate"));
}

#[test]
fn test_preset_setting_descriptors_match_engine_read_keys() {
    use file_converter_core::types::{OutputType, preset_setting_keys};

    // Guards against a descriptor being added for a key no engine reads: the UI
    // would offer a control that has no effect.
    let known: Vec<&str> = vec![
        "AudioBitrate",
        "AudioChannelCount",
        "AudioEncodingMode",
        "AudioNormalize",
        "AudioLoudnorm",
        "EnableAudio",
        "EnableFFMPEGCustomCommand",
        "FFMPEGCustomCommand",
        "ImageClampSizePowerOf2",
        "ImageMaximumSize",
        "ImageRotation",
        "ImageScale",
        "JpegQuality",
        "OxipngOptimizationLevel",
        "PdfJpegQuality",
        "PdfTargetDpi",
        "VideoEncodingSpeed",
        "VideoFramesPerSecond",
        "VideoQuality",
        "VideoRotation",
        "VideoScale",
    ];
    for ot in [
        OutputType::Aac,
        OutputType::Avi,
        OutputType::Flac,
        OutputType::Gif,
        OutputType::Jpg,
        OutputType::Mkv,
        OutputType::Mp3,
        OutputType::Mp4,
        OutputType::Ogv,
        OutputType::Pdf,
        OutputType::Png,
        OutputType::Wav,
        OutputType::Webm,
    ] {
        for k in preset_setting_keys(ot) {
            assert!(
                known.contains(&k.key),
                "{ot:?} exposes unknown setting key {:?}",
                k.key
            );
        }
    }
}

#[test]
fn test_version_comparison() {
    use file_converter_core::update_check::is_version_newer;
    assert!(is_version_newer("0.9.1", "v0.9.2"));
    assert!(!is_version_newer("0.9.2", "0.9.2"));
    assert!(!is_version_newer("0.9.4", "0.9.3"));
}
