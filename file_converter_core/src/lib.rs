#![allow(clippy::all, warnings)]
pub mod cda;
pub mod ffmpeg;
pub mod imagemagick;
pub mod office;
pub mod path_helpers;
pub mod scheduler;
pub mod settings;
pub mod types;

#[cfg(test)]
mod tests {
    use super::settings::Settings;

    #[test]
    fn test_load_default_settings() {
        // Path to C# Settings.default.xml
        let default_xml_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../C#/Application/FileConverter/Settings.default.xml"
        );
        let load_result = Settings::load_from_file(default_xml_path);

        match &load_result {
            Ok(settings) => {
                println!("Loaded settings successfully!");
                println!("Serialization version: {}", settings.serialization_version);
                println!("Presets count: {}", settings.conversion_presets.len());
                assert!(settings.conversion_presets.len() > 0);
            }
            Err(e) => {
                panic!("Failed to load default settings: {:?}", e);
            }
        }
    }

    #[test]
    fn test_path_templates() {
        use super::path_helpers::generate_file_path_from_template;

        let input = "C:\\FolderA\\FolderB\\music_track.flac";
        let template = "(p)(f)";
        let output = generate_file_path_from_template(input, "mp3", template, 1, 1);
        assert_eq!(output, "C:\\FolderA\\FolderB\\music_track.mp3");

        let template_d0 = "(p)(d0) - (f)";
        let output_d0 = generate_file_path_from_template(input, "mp3", template_d0, 1, 1);
        assert_eq!(output_d0, "C:\\FolderA\\FolderB\\FolderB - music_track.mp3");

        let template_d1 = "(p)(d1) - (f)";
        let output_d1 = generate_file_path_from_template(input, "mp3", template_d1, 1, 1);
        assert_eq!(output_d1, "C:\\FolderA\\FolderB\\FolderA - music_track.mp3");
    }
}
