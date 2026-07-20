use quick_xml::de::from_str;
use quick_xml::se::to_string_with_root;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use crate::types::{HardwareAccelerationMode, InputPostConversionAction, OutputType};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "PascalCase")]
pub struct Settings {
    #[serde(rename = "@SerializationVersion")]
    pub serialization_version: i32,

    pub maximum_number_of_simultaneous_conversions: usize,

    #[serde(default)]
    pub exit_application_when_conversions_finished: bool,

    #[serde(default)]
    pub duration_between_end_of_conversions_and_application_exit: f32,

    #[serde(default)]
    pub check_upgrade_at_startup: bool,

    #[serde(default)]
    pub application_language_name: String,

    #[serde(default)]
    pub copy_files_in_clipboard_after_conversion: bool,

    #[serde(default)]
    pub hardware_acceleration_mode: HardwareAccelerationMode,

    #[serde(rename = "ConversionPreset", default)]
    pub conversion_presets: Vec<ConversionPreset>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "PascalCase")]
pub struct ConversionPreset {
    #[serde(rename = "@Name")]
    pub name: String,

    #[serde(rename = "@OutputType")]
    pub output_type: OutputType,

    #[serde(rename = "@IsDefaultSettings", default)]
    pub is_default_settings: bool,

    #[serde(rename = "InputTypes", default)]
    pub input_types: Vec<String>,

    pub input_post_conversion_action: InputPostConversionAction,

    #[serde(rename = "Settings", default)]
    pub settings: Vec<PresetSetting>,

    pub output_file_name_template: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PresetSetting {
    #[serde(rename = "@Key")]
    pub key: String,
    #[serde(rename = "@Value")]
    pub value: String,
}

impl Settings {
    pub const CURRENT_VERSION: i32 = 4;

    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let mut file = File::open(path)?;
        let mut content = String::new();
        file.read_to_string(&mut content)?;

        let mut settings: Settings = from_str(&content)?;
        settings.migrate();
        Ok(settings)
    }

    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<(), Box<dyn std::error::Error>> {
        let xml_str = to_string_with_root("Settings", self)?;
        // Add XML header
        let mut file = File::create(path)?;
        file.write_all(b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\n")?;
        file.write_all(xml_str.as_bytes())?;
        Ok(())
    }

    pub fn merge(&mut self, default: Settings) {
        for default_preset in default.conversion_presets {
            if !self
                .conversion_presets
                .iter()
                .any(|p| p.name == default_preset.name)
            {
                self.conversion_presets.push(default_preset);
            }
        }
    }

    pub fn get_preset_from_name(&self, name: &str) -> Option<&ConversionPreset> {
        self.conversion_presets.iter().find(|p| p.name == name)
    }

    pub fn migrate(&mut self) {
        let version = self.serialization_version;
        if version == Self::CURRENT_VERSION {
            return;
        }

        for preset in &mut self.conversion_presets {
            preset.migrate(version);
        }

        self.serialization_version = Self::CURRENT_VERSION;
    }
}

impl ConversionPreset {
    pub fn get_settings_map(&self) -> HashMap<String, String> {
        self.settings
            .iter()
            .map(|s| (s.key.clone(), s.value.clone()))
            .collect()
    }

    pub fn get_setting_value(&self, key: &str) -> Option<&str> {
        self.settings
            .iter()
            .find(|s| s.key == key)
            .map(|s| s.value.as_str())
    }

    pub fn set_setting_value(&mut self, key: &str, value: &str) {
        if let Some(setting) = self.settings.iter_mut().find(|s| s.key == key) {
            setting.value = value.to_string();
        } else {
            self.settings.push(PresetSetting {
                key: key.to_string(),
                value: value.to_string(),
            });
        }
    }

    pub fn migrate(&mut self, old_version: i32) {
        if old_version <= 2 {
            if let Some(speed) = self.get_setting_value("VideoEncodingSpeed") {
                let mapped = match speed {
                    "Ultra Fast" => Some("UltraFast"),
                    "Super Fast" => Some("SuperFast"),
                    "Very Fast" => Some("VeryFast"),
                    "Very Slow" => Some("VerySlow"),
                    _ => None,
                };
                if let Some(m) = mapped {
                    self.set_setting_value("VideoEncodingSpeed", m);
                }
            }
        }

        if old_version <= 3 {
            for key in &["ImageScale", "VideoScale"] {
                if let Some(scale) = self.get_setting_value(key) {
                    let cleaned = scale.replace(',', ".");
                    self.set_setting_value(key, &cleaned);
                }
            }
        }
    }
}
