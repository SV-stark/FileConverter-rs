pub mod doc_convert;
pub mod error;
pub mod ffmpeg;
pub mod ffmpeg_download;
pub mod image;
pub mod office;
pub mod path_helpers;
pub mod pdf_compress;
pub mod scheduler;
pub mod settings;
pub mod types;
pub mod update_check;

#[cfg(test)]
mod tests {
    use super::path_helpers::*;
    use super::scheduler::*;
    use super::settings::*;
    use super::types::*;

    const DEFAULT_SETTINGS_XML: &str = include_str!("../../Settings.default.xml");

    #[test]
    fn test_load_default_settings_xml() {
        let temp_dir = std::env::temp_dir();
        let unique_name = format!(
            "test_settings_default_{}_{}.xml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let xml_path = temp_dir.join(unique_name);
        std::fs::write(&xml_path, DEFAULT_SETTINGS_XML).expect("Failed to write test XML");

        let settings = Settings::load_from_file(&xml_path).expect("Failed to parse settings XML");
        assert!(
            !settings.conversion_presets.is_empty(),
            "Default presets should not be empty"
        );
        assert_eq!(settings.serialization_version, 4);

        let _ = std::fs::remove_file(xml_path);
    }

    #[test]
    fn test_settings_save_and_reload_roundtrip() {
        let temp_dir = std::env::temp_dir();
        let unique_name = format!(
            "test_settings_roundtrip_{}_{}.xml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let xml_path = temp_dir.join(unique_name);
        std::fs::write(&xml_path, DEFAULT_SETTINGS_XML).expect("Failed to write temp XML");

        let mut settings = Settings::load_from_file(&xml_path).unwrap_or_else(|_| Settings {
            serialization_version: 4,
            maximum_number_of_simultaneous_conversions: 4,
            exit_application_when_conversions_finished: true,
            duration_between_end_of_conversions_and_application_exit: 2.0,
            check_upgrade_at_startup: false,
            application_language_name: "en".to_string(),
            copy_files_in_clipboard_after_conversion: true,
            hardware_acceleration_mode: HardwareAccelerationMode::Off,
            auto_start_on_file_drop: true,
            conversion_presets: vec![],
        });

        settings.maximum_number_of_simultaneous_conversions = 8;
        settings
            .save_to_file(&xml_path)
            .expect("Failed to save settings roundtrip");

        let reloaded =
            Settings::load_from_file(&xml_path).expect("Failed to reload saved settings");
        assert_eq!(reloaded.maximum_number_of_simultaneous_conversions, 8);

        let _ = std::fs::remove_file(xml_path);
    }

    #[test]
    fn test_preset_setting_lookup_and_mutation() {
        let mut preset = ConversionPreset {
            name: "Test Preset".to_string(),
            output_type: OutputType::Mp3,
            output_file_name_template: "(p)\\(f)".to_string(),
            is_default_settings: false,
            input_types: vec!["wav".into(), "flac".into()],
            input_post_conversion_action: InputPostConversionAction::None,
            settings: vec![],
        };

        assert_eq!(preset.get_setting_value("AudioBitRate"), None);

        preset.set_setting_value("AudioBitRate", "320k");
        assert_eq!(preset.get_setting_value("AudioBitRate"), Some("320k"));

        preset.set_setting_value("AudioBitRate", "192k");
        assert_eq!(preset.get_setting_value("AudioBitRate"), Some("192k"));
    }

    #[test]
    fn test_path_template_replacements() {
        let input = "C:\\Music\\Album\\track1.flac";
        let template = "(p)(f)";
        let output = generate_file_path_from_template(input, "mp3", template, 1, 1);
        assert_eq!(output, "C:\\Music\\Album\\track1.mp3");

        let template_d0 = "(p)(d0) - (f)";
        let output_d0 = generate_file_path_from_template(input, "mp3", template_d0, 1, 1);
        assert_eq!(output_d0, "C:\\Music\\Album\\Album - track1.mp3");

        let template_d1 = "(p)(d1) - (f)";
        let output_d1 = generate_file_path_from_template(input, "mp3", template_d1, 1, 1);
        assert_eq!(output_d1, "C:\\Music\\Album\\Music - track1.mp3");

        let template_case = "(p)(F)_(O)";
        let output_case = generate_file_path_from_template(input, "mp3", template_case, 1, 1);
        assert_eq!(output_case, "C:\\Music\\Album\\TRACK1_MP3.mp3");
    }

    #[test]
    fn test_path_validation_and_drive_helpers() {
        assert!(is_path_drive_letter_valid("C:\\Program Files"));
        assert!(is_path_drive_letter_valid("D:\\"));
        assert!(!is_path_drive_letter_valid("Program Files"));

        assert_eq!(
            get_path_drive_letter("C:\\Users\\Desktop"),
            Some("C:\\".to_string())
        );
        assert_eq!(get_path_drive_letter("relative/path/file.txt"), None);

        assert!(is_path_valid("C:\\Users\\file.txt"));
        assert!(is_path_valid("\\\\Server\\Share\\file.txt"));
    }

    #[test]
    fn test_unique_path_generator() {
        let temp_dir = std::env::temp_dir();
        let file_path = temp_dir.join("test_unique_file.tmp");
        std::fs::write(&file_path, "dummy").expect("Failed to write temp file");

        let blacklist = vec![];
        let unique = generate_unique_path(&file_path, &blacklist);
        assert_ne!(unique, file_path);
        assert!(unique.to_string_lossy().contains("(2)"));

        let _ = std::fs::remove_file(file_path);
    }

    #[test]
    fn test_job_preparation_and_engine_category() {
        let preset = ConversionPreset {
            name: "To Mp3".to_string(),
            output_type: OutputType::Mp3,
            output_file_name_template: "(p)(f)".to_string(),
            is_default_settings: true,
            input_types: vec!["wav".into()],
            input_post_conversion_action: InputPostConversionAction::None,
            settings: vec![],
        };

        let mut job = ConversionJob::new(1, preset.clone(), "C:\\Audio\\sample.wav".to_string());
        assert!(job.prepare(0, 1).is_ok());
        assert_eq!(job.output_file_paths.len(), 1);
        assert_eq!(job.output_file_paths[0], "C:\\Audio\\sample.mp3");

        let engine = determine_job_engine(&preset, "C:\\Audio\\sample.wav");
        assert!(matches!(engine, JobEngine::Ffmpeg));
    }

    #[test]
    fn test_image_engine_category() {
        let preset = ConversionPreset {
            name: "To Png".to_string(),
            output_type: OutputType::Png,
            output_file_name_template: "(p)\\(f)".to_string(),
            is_default_settings: true,
            input_types: vec!["jpg".into(), "bmp".into()],
            input_post_conversion_action: InputPostConversionAction::None,
            settings: vec![],
        };

        let engine = determine_job_engine(&preset, "C:\\Pictures\\photo.jpg");
        assert!(matches!(engine, JobEngine::Image));
    }

    #[test]
    fn test_scheduler_bounded_execution() {
        let preset = ConversionPreset {
            name: "To Png".to_string(),
            output_type: OutputType::Png,
            output_file_name_template: "(p)\\(f)".to_string(),
            is_default_settings: true,
            input_types: vec!["jpg".into()],
            input_post_conversion_action: InputPostConversionAction::None,
            settings: vec![],
        };

        let jobs = vec![
            ConversionJob::new(1, preset.clone(), "C:\\Test\\file1.jpg".to_string()),
            ConversionJob::new(2, preset.clone(), "C:\\Test\\file2.jpg".to_string()),
        ];

        let scheduler = ConversionScheduler::new(jobs, 2, HardwareAccelerationMode::Off, false);
        assert_eq!(scheduler.jobs.len(), 2);
    }

    #[test]
    fn test_pdf_compress_options() {
        let opts = super::pdf_compress::PdfCompressOptions::default();
        assert_eq!(opts.target_dpi, 150);
        assert_eq!(opts.jpeg_quality, 75);
    }

    #[test]
    fn test_document_extensions_and_markdown_conversion() {
        assert_eq!(get_extension_category("epub"), FileCategory::Document);
        assert_eq!(get_extension_category("mobi"), FileCategory::Document);
        assert_eq!(get_extension_category("azw3"), FileCategory::Document);
        assert_eq!(get_extension_category("kfx"), FileCategory::Document);
        assert_eq!(get_extension_category("fb2"), FileCategory::Document);
        assert_eq!(get_extension_category("cbz"), FileCategory::Document);
        assert_eq!(get_extension_category("lit"), FileCategory::Document);
        assert_eq!(get_extension_category("md"), FileCategory::Document);
        assert_eq!(get_extension_category("typ"), FileCategory::Document);

        let temp_dir = std::env::temp_dir();
        let unique_suffix = format!(
            "{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let md_path = temp_dir.join(format!("test_doc_{}.md", unique_suffix));
        let html_out = temp_dir.join(format!("test_doc_{}.html", unique_suffix));

        std::fs::write(&md_path, "# Title\n\nThis is **Markdown** text.").unwrap();
        // 1. Test HTML generation
        let res_html = super::doc_convert::run_markdown_conversion(
            md_path.to_str().unwrap(),
            html_out.to_str().unwrap(),
            OutputType::None,
            &|_p, _m| {},
        );
        assert!(res_html.is_ok());
        assert!(html_out.exists());

        let html_content = std::fs::read_to_string(&html_out).unwrap();
        assert!(html_content.contains("Title"));
        assert!(html_content.contains("<strong>Markdown</strong>"));

        // 2. Test PDF vector generation
        let pdf_out = temp_dir.join(format!("test_doc_{}.pdf", unique_suffix));
        let res_pdf = super::doc_convert::run_markdown_conversion(
            md_path.to_str().unwrap(),
            pdf_out.to_str().unwrap(),
            OutputType::Pdf,
            &|_p, _m| {},
        );
        assert!(res_pdf.is_ok());
        assert!(pdf_out.exists());
        let pdf_bytes = std::fs::read(&pdf_out).unwrap();
        assert!(pdf_bytes.starts_with(b"%PDF-"));

        let _ = std::fs::remove_file(md_path);
        let _ = std::fs::remove_file(html_out);
        let _ = std::fs::remove_file(pdf_out);
    }

    #[test]
    fn test_svg_resvg_rendering() {
        let temp_dir = std::env::temp_dir();
        let svg_path = temp_dir.join("test_resvg.svg");
        let png_path = temp_dir.join("test_resvg.png");

        let svg_content = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
            <rect width="100" height="100" fill="red" />
        </svg>"#;

        std::fs::write(&svg_path, svg_content).unwrap();

        let dims = super::image::get_image_dimensions(svg_path.to_str().unwrap());
        assert!(dims.is_ok());
        let (w, h) = dims.unwrap();
        assert_eq!(w, 100);
        assert_eq!(h, 100);

        let preset = ConversionPreset {
            name: "To Png".to_string(),
            output_type: OutputType::Png,
            output_file_name_template: "(p)\\(f)".to_string(),
            is_default_settings: true,
            input_types: vec!["svg".into()],
            input_post_conversion_action: InputPostConversionAction::None,
            settings: vec![],
        };

        let res = super::image::run_image_conversion(
            &preset,
            svg_path.to_str().unwrap(),
            &[png_path.to_str().unwrap().to_string()],
            &|_p, _m| {},
        );
        assert!(res.is_ok());
        assert!(png_path.exists());

        let _ = std::fs::remove_file(svg_path);
        let _ = std::fs::remove_file(png_path);
    }

    #[test]
    fn test_image_to_pdf_generation_via_pdf_writer() {
        let temp_dir = std::env::temp_dir();
        let jpg_path = temp_dir.join("test_img.jpg");
        let pdf_path = temp_dir.join("test_output.pdf");

        // Create a small test RGB image
        let img = image::DynamicImage::new_rgb8(120, 80);
        img.save(&jpg_path).unwrap();

        let preset = ConversionPreset {
            name: "To Pdf".to_string(),
            output_type: OutputType::Pdf,
            output_file_name_template: "(p)\\(f)".to_string(),
            is_default_settings: true,
            input_types: vec!["jpg".into()],
            input_post_conversion_action: InputPostConversionAction::None,
            settings: vec![],
        };

        let res = super::image::run_image_conversion(
            &preset,
            jpg_path.to_str().unwrap(),
            &[pdf_path.to_str().unwrap().to_string()],
            &|_p, _m| {},
        );
        assert!(res.is_ok());
        assert!(pdf_path.exists());
        let pdf_bytes = std::fs::read(&pdf_path).unwrap();
        assert!(pdf_bytes.starts_with(b"%PDF-"));

        let _ = std::fs::remove_file(jpg_path);
        let _ = std::fs::remove_file(pdf_path);
    }

    #[test]
    fn test_audio_loudnorm_filter_computation() {
        let preset_normal = ConversionPreset {
            name: "To MP3".to_string(),
            output_type: OutputType::Mp3,
            output_file_name_template: "(p)\\(f)".to_string(),
            is_default_settings: true,
            input_types: vec!["wav".into()],
            input_post_conversion_action: InputPostConversionAction::None,
            settings: vec![],
        };
        assert!(super::ffmpeg::compute_audio_filter_args(&preset_normal).is_none());

        let preset_loudnorm = ConversionPreset {
            name: "To MP3 (Normalize)".to_string(),
            output_type: OutputType::Mp3,
            output_file_name_template: "(p)\\(f)".to_string(),
            is_default_settings: true,
            input_types: vec!["wav".into()],
            input_post_conversion_action: InputPostConversionAction::None,
            settings: vec![],
        };
        assert_eq!(
            super::ffmpeg::compute_audio_filter_args(&preset_loudnorm),
            Some("loudnorm=I=-16:TP=-1.5:LRA=11".to_string())
        );
    }

    #[test]
    fn test_jxl_and_epub_output_types_and_categories() {
        assert_eq!(get_extension_category("jxl"), FileCategory::Image);
        assert!(is_output_type_compatible_with_category(
            OutputType::Jxl,
            FileCategory::Image
        ));
        assert!(is_output_type_compatible_with_category(
            OutputType::Epub,
            FileCategory::Document
        ));
        assert_eq!(OutputType::Jxl.extension(), "jxl");
        assert_eq!(OutputType::Epub.extension(), "epub");
        assert!(matches!(
            HardwareAccelerationMode::default(),
            HardwareAccelerationMode::Auto
        ));
    }

    #[test]
    fn test_strip_html_tags_simd_accuracy() {
        let input = "<p>Welcome to <b>File Converter</b>! <a href=\"https://example.com\">Click here</a> for more.</p>";
        let stripped = super::doc_convert::strip_html_tags(input);
        assert_eq!(stripped, "Welcome to File Converter! Click here for more.");

        let unclosed = "Plain text with <unclosed tag and <tag>valid</tag>";
        let stripped_unclosed = super::doc_convert::strip_html_tags(unclosed);
        assert_eq!(stripped_unclosed, "Plain text with valid");

        let empty = "";
        assert_eq!(super::doc_convert::strip_html_tags(empty), "");

        let no_tags = "Just plain text without any HTML tags.";
        assert_eq!(super::doc_convert::strip_html_tags(no_tags), no_tags);
    }

    #[test]
    fn test_path_template_date_and_index_placeholders() {
        let path = "C:\\Music\\Rock\\Track01.flac";
        let res = super::path_helpers::generate_file_path_from_template(
            path,
            "mp3",
            "(p)(f)_(n:i)of(n:c)",
            3,
            10,
        );
        assert_eq!(res, "C:\\Music\\Rock\\Track01_3of10.mp3");

        let res_nesting = super::path_helpers::generate_file_path_from_template(
            path,
            "mp3",
            "(p)(d0)_(d1)_(f)",
            0,
            1,
        );
        assert_eq!(res_nesting, "C:\\Music\\Rock\\Rock_Music_Track01.mp3");
    }

    #[test]
    fn test_ffmpeg_h264_encoding_speed_mappings() {
        use super::ffmpeg::{
            h264_encoding_speed_to_amf_quality, h264_encoding_speed_to_nvenc_preset,
            h264_encoding_speed_to_preset,
        };

        assert_eq!(
            h264_encoding_speed_to_preset(VideoEncodingSpeed::UltraFast),
            "ultrafast"
        );
        assert_eq!(
            h264_encoding_speed_to_preset(VideoEncodingSpeed::Medium),
            "medium"
        );
        assert_eq!(
            h264_encoding_speed_to_preset(VideoEncodingSpeed::VerySlow),
            "veryslow"
        );

        assert_eq!(
            h264_encoding_speed_to_nvenc_preset(VideoEncodingSpeed::UltraFast),
            "p1"
        );
        assert_eq!(
            h264_encoding_speed_to_nvenc_preset(VideoEncodingSpeed::Medium),
            "p4"
        );
        assert_eq!(
            h264_encoding_speed_to_nvenc_preset(VideoEncodingSpeed::VerySlow),
            "p7"
        );

        assert_eq!(
            h264_encoding_speed_to_amf_quality(VideoEncodingSpeed::UltraFast),
            "speed"
        );
        assert_eq!(
            h264_encoding_speed_to_amf_quality(VideoEncodingSpeed::Medium),
            "balanced"
        );
        assert_eq!(
            h264_encoding_speed_to_amf_quality(VideoEncodingSpeed::VerySlow),
            "quality"
        );
    }

    #[test]
    fn test_ffmpeg_audio_quality_conversions() {
        use super::ffmpeg::{
            aac_bitrate_to_quality_index, mp3_vbr_bitrate_to_quality_index,
            ogg_vbr_bitrate_to_quality_index,
        };

        assert_eq!(aac_bitrate_to_quality_index(340), "3");
        assert_eq!(aac_bitrate_to_quality_index(128), "1");
        assert_eq!(aac_bitrate_to_quality_index(48), "0.3");

        assert_eq!(mp3_vbr_bitrate_to_quality_index(245).unwrap(), 0);
        assert_eq!(mp3_vbr_bitrate_to_quality_index(190).unwrap(), 2);
        assert_eq!(mp3_vbr_bitrate_to_quality_index(130).unwrap(), 5);
        assert_eq!(mp3_vbr_bitrate_to_quality_index(65).unwrap(), 9);

        assert_eq!(ogg_vbr_bitrate_to_quality_index(500).unwrap(), 10);
        assert_eq!(ogg_vbr_bitrate_to_quality_index(160).unwrap(), 5);
        assert_eq!(ogg_vbr_bitrate_to_quality_index(64).unwrap(), 0);
        assert_eq!(ogg_vbr_bitrate_to_quality_index(48).unwrap(), -1);
    }

    #[test]
    fn test_ffmpeg_custom_command_pass_generation() {
        let mut preset = ConversionPreset {
            name: "Custom MP4".to_string(),
            output_type: OutputType::Mp4,
            output_file_name_template: "(p)\\(f)".to_string(),
            is_default_settings: false,
            input_types: vec!["mkv".into()],
            input_post_conversion_action: InputPostConversionAction::None,
            settings: vec![],
        };
        preset.set_setting_value("EnableFFMPEGCustomCommand", "true");
        preset.set_setting_value("FFMPEGCustomCommand", "-c:v copy -c:a copy");

        let passes = super::ffmpeg::get_ffmpeg_passes(
            &preset,
            "C:\\input.mkv",
            "C:\\output.mp4",
            HardwareAccelerationMode::Off,
        );
        assert!(passes.is_ok());
        let pass_list = passes.unwrap();
        assert_eq!(pass_list.len(), 1);
        assert!(pass_list[0].arguments.contains(&"-c:v".to_string()));
        assert!(pass_list[0].arguments.contains(&"copy".to_string()));
    }

    #[test]
    fn test_all_output_type_extensions_and_category_compatibilities() {
        let all_types = [
            (OutputType::Aac, "aac", FileCategory::Audio),
            (OutputType::Avi, "avi", FileCategory::Video),
            (OutputType::Avif, "avif", FileCategory::Image),
            (OutputType::Epub, "epub", FileCategory::Document),
            (OutputType::Flac, "flac", FileCategory::Audio),
            (OutputType::Gif, "gif", FileCategory::AnimatedImage),
            (OutputType::Ico, "ico", FileCategory::Image),
            (OutputType::Jpg, "jpg", FileCategory::Image),
            (OutputType::Jxl, "jxl", FileCategory::Image),
            (OutputType::Mkv, "mkv", FileCategory::Video),
            (OutputType::Mp3, "mp3", FileCategory::Audio),
            (OutputType::Mp4, "mp4", FileCategory::Video),
            (OutputType::Ogg, "ogg", FileCategory::Audio),
            (OutputType::Ogv, "ogv", FileCategory::Video),
            (OutputType::Pdf, "pdf", FileCategory::Document),
            (OutputType::Png, "png", FileCategory::Image),
            (OutputType::Wav, "wav", FileCategory::Audio),
            (OutputType::Webm, "webm", FileCategory::Video),
            (OutputType::Webp, "webp", FileCategory::Image),
        ];

        for (ot, ext, cat) in all_types {
            assert_eq!(ot.extension(), ext);
            assert!(is_output_type_compatible_with_category(ot, cat));
        }
        // Test GIF compatibility across AnimatedImage, Image, and Video
        assert!(is_output_type_compatible_with_category(
            OutputType::Gif,
            FileCategory::AnimatedImage
        ));
        assert!(is_output_type_compatible_with_category(
            OutputType::Gif,
            FileCategory::Image
        ));
        assert!(is_output_type_compatible_with_category(
            OutputType::Gif,
            FileCategory::Video
        ));
        assert!(!is_output_type_compatible_with_category(
            OutputType::Gif,
            FileCategory::Audio
        ));

        assert_eq!(OutputType::None.extension(), "");
        assert!(!is_output_type_compatible_with_category(
            OutputType::None,
            FileCategory::Audio
        ));
    }

    #[test]
    fn test_document_and_office_conversion_routing() {
        let doc_preset = ConversionPreset {
            name: "To PDF".to_string(),
            output_type: OutputType::Pdf,
            output_file_name_template: "(p)\\(f)".to_string(),
            is_default_settings: true,
            input_types: vec!["docx".into(), "xlsx".into(), "md".into(), "epub".into()],
            input_post_conversion_action: InputPostConversionAction::None,
            settings: vec![],
        };

        assert!(matches!(
            determine_job_engine(&doc_preset, "C:\\doc.docx"),
            JobEngine::Word
        ));
        assert!(matches!(
            determine_job_engine(&doc_preset, "C:\\sheet.xlsx"),
            JobEngine::Excel
        ));
        assert!(matches!(
            determine_job_engine(&doc_preset, "C:\\slides.pptx"),
            JobEngine::PowerPoint
        ));
        assert!(matches!(
            determine_job_engine(&doc_preset, "C:\\readme.md"),
            JobEngine::Markdown
        ));
        assert!(matches!(
            determine_job_engine(&doc_preset, "C:\\book.epub"),
            JobEngine::Epub
        ));
        assert!(matches!(
            determine_job_engine(&doc_preset, "C:\\novel.mobi"),
            JobEngine::Epub
        ));
    }
}
