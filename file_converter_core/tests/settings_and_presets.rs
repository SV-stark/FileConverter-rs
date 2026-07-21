use file_converter_core::settings::Settings;
use file_converter_core::types::{
    OutputType, get_extension_category, is_output_type_compatible_with_category,
};

const DEFAULT_SETTINGS_XML: &str = include_str!("../../Settings.default.xml");

#[test]
fn test_default_settings_xml_parsing_integration() {
    let temp_dir = std::env::temp_dir();
    let xml_path = temp_dir.join("test_settings_default_integration.xml");
    std::fs::write(&xml_path, DEFAULT_SETTINGS_XML).expect("Failed to write default XML");

    let settings = Settings::load_from_file(&xml_path).expect("Failed to load settings XML");
    assert!(
        !settings.conversion_presets.is_empty(),
        "Default presets should not be empty"
    );
    assert_eq!(settings.serialization_version, 4);

    let _ = std::fs::remove_file(xml_path);
}

#[test]
fn test_preset_compatibility_logic_integration() {
    assert_eq!(get_extension_category("mp3"), "Audio");
    assert_eq!(get_extension_category("mp4"), "Video");
    assert_eq!(get_extension_category("png"), "Image");
    assert_eq!(get_extension_category("gif"), "Animated Image");
    assert_eq!(get_extension_category("docx"), "Document");

    assert!(is_output_type_compatible_with_category(
        OutputType::Mp3,
        "Audio"
    ));
    assert!(is_output_type_compatible_with_category(
        OutputType::Mp3,
        "Video"
    ));
    assert!(!is_output_type_compatible_with_category(
        OutputType::Mp3,
        "Image"
    ));

    assert!(is_output_type_compatible_with_category(
        OutputType::Png,
        "Image"
    ));
    assert!(is_output_type_compatible_with_category(
        OutputType::Png,
        "Document"
    ));
}
