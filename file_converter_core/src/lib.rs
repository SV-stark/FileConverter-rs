#![allow(clippy::all, warnings)]
pub mod ffmpeg;
pub mod image;
pub mod office;
pub mod path_helpers;
pub mod scheduler;
pub mod settings;
pub mod types;

#[cfg(test)]
mod tests {
    use super::path_helpers::*;
    use super::scheduler::*;
    use super::settings::*;
    use super::types::*;
    use std::path::PathBuf;

    const DEFAULT_SETTINGS_XML: &str = include_str!("../../Settings.default.xml");

    #[test]
    fn test_load_default_settings_xml() {
        let temp_dir = std::env::temp_dir();
        let xml_path = temp_dir.join("test_settings_default.xml");
        std::fs::write(&xml_path, DEFAULT_SETTINGS_XML).expect("Failed to write test XML");

        let settings = Settings::load_from_file(&xml_path).expect("Failed to parse settings XML");
        assert!(
            settings.conversion_presets.len() > 0,
            "Default presets should not be empty"
        );
        assert_eq!(settings.serialization_version, 4);

        let _ = std::fs::remove_file(xml_path);
    }

    #[test]
    fn test_settings_save_and_reload_roundtrip() {
        let temp_dir = std::env::temp_dir();
        let xml_path = temp_dir.join("test_settings_roundtrip.xml");
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
            input_types: vec!["wav".to_string(), "flac".to_string()],
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
            input_types: vec!["wav".to_string()],
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
            input_types: vec!["jpg".to_string(), "bmp".to_string()],
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
            input_types: vec!["jpg".to_string()],
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
}
