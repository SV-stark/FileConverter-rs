use ahash::AHashMap;
use quick_xml::de::from_str;
use quick_xml::se::to_string_with_root;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use crate::error::Result;
use crate::types::{HardwareAccelerationMode, InputPostConversionAction, OutputType};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "PascalCase")]
pub struct Settings {
    #[serde(rename = "@SerializationVersion", default)]
    pub serialization_version: i32,

    /// `0` means "automatic" (half the logical CPUs), as in the C# original.
    /// Defaulted so a hand-edited or older file without it still loads
    /// instead of failing outright.
    #[serde(default)]
    pub maximum_number_of_simultaneous_conversions: usize,

    #[serde(default)]
    pub exit_application_when_conversions_finished: bool,

    /// Seconds, clamped to 0..=10 on load (C# parity). Defaults to 3.
    #[serde(default = "default_exit_delay")]
    pub duration_between_end_of_conversions_and_application_exit: f32,

    /// On by default, like the C# original.
    #[serde(default = "default_true")]
    pub check_upgrade_at_startup: bool,

    #[serde(default)]
    pub application_language_name: String,

    #[serde(default)]
    pub copy_files_in_clipboard_after_conversion: bool,

    #[serde(default)]
    pub hardware_acceleration_mode: HardwareAccelerationMode,

    #[serde(default)]
    pub auto_start_on_file_drop: bool,

    /// `None` follows the Windows app theme; `Some(true)` forces dark.
    ///
    /// Optional so that "not chosen yet" stays distinguishable from an explicit
    /// choice, which matters because the in-app toggle used to be discarded on
    /// exit and re-derived from the registry every launch.
    ///
    /// `skip_serializing_if` is essential, not cosmetic: quick-xml writes `None`
    /// as an empty `<DarkMode />` element, and reading that back fails with
    /// `InvalidBoolean("")`, which would make the settings file unloadable after
    /// the very first save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dark_mode: Option<bool>,

    #[serde(rename = "ConversionPreset", default)]
    pub conversion_presets: Vec<ConversionPreset>,
}

pub use compact_str::CompactString as CompactStr;

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
    pub input_types: Vec<CompactStr>,

    #[serde(default)]
    pub input_post_conversion_action: InputPostConversionAction,

    #[serde(rename = "Settings", default)]
    pub settings: Vec<PresetSetting>,

    #[serde(default = "default_output_template")]
    pub output_file_name_template: String,
}

fn default_true() -> bool {
    true
}

fn default_exit_delay() -> f32 {
    3.0
}

fn default_output_template() -> String {
    "(p)(f)".to_string()
}

/// A standalone preset file, as written by the C# "Export preset" command:
/// `<Presets><ConversionPreset .../>...</Presets>`.
///
/// Reading also accepts a full `<Settings>` file (unknown elements are
/// ignored), so either kind of file can be imported.
#[derive(Debug, Serialize, Deserialize, Default)]
struct PresetsFile {
    #[serde(rename = "ConversionPreset", default)]
    presets: Vec<ConversionPreset>,
}

/// Writes `bytes` to `path` atomically: a crash or full disk mid-write can
/// no longer leave a truncated, unloadable settings file behind.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// Reads the presets from a preset file or a full settings file.
pub fn load_presets_file<P: AsRef<Path>>(path: P) -> Result<Vec<ConversionPreset>> {
    let content = std::fs::read_to_string(path)?;
    let content = content.trim_start_matches('\u{feff}');
    let file: PresetsFile = from_str(content)?;
    let mut presets = file.presets;
    for preset in &mut presets {
        preset.normalize();
    }
    Ok(presets)
}

/// Writes presets in the C#-compatible `<Presets>` export format.
pub fn save_presets_file<P: AsRef<Path>>(path: P, presets: &[ConversionPreset]) -> Result<()> {
    let file = PresetsFile {
        presets: presets.to_vec(),
    };
    let xml = to_string_with_root("Presets", &file)?;
    let mut bytes = b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\n".to_vec();
    bytes.extend_from_slice(xml.as_bytes());
    write_atomically(path.as_ref(), &bytes)
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PresetSetting {
    #[serde(rename = "@Key")]
    pub key: CompactStr,
    #[serde(rename = "@Value")]
    pub value: CompactStr,
}

impl Settings {
    pub const CURRENT_VERSION: i32 = 4;

    pub fn load_from_str(content: &str) -> Result<Self> {
        // Files written by some editors start with a UTF-8 byte-order mark.
        let content = content.trim_start_matches('\u{feff}');
        let mut settings: Settings = from_str(content)?;
        settings.migrate();
        settings.normalize();
        Ok(settings)
    }

    /// Applies the load-time fix-ups the C# original performs in
    /// `OnDeserializationComplete`: clamp the exit delay, lower-case input
    /// types and translate legacy setting keys.
    pub fn normalize(&mut self) {
        let delay = self.duration_between_end_of_conversions_and_application_exit;
        self.duration_between_end_of_conversions_and_application_exit = if delay.is_finite() {
            delay.clamp(0.0, 10.0)
        } else {
            3.0
        };
        for preset in &mut self.conversion_presets {
            preset.normalize();
        }
    }

    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mut file = File::open(path)?;
        let mut content = String::new();
        file.read_to_string(&mut content)?;
        Self::load_from_str(&content)
    }

    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let xml_str = to_string_with_root("Settings", self)?;
        let mut bytes = b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\n".to_vec();
        bytes.extend_from_slice(xml_str.as_bytes());
        write_atomically(path.as_ref(), &bytes)
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

    pub fn build_preset_map(&self) -> AHashMap<&str, &ConversionPreset> {
        let mut map = AHashMap::with_capacity(self.conversion_presets.len());
        for preset in &self.conversion_presets {
            map.insert(preset.name.as_str(), preset);
        }
        map
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

/// Splits a preset name into its `/`-separated parts, ignoring slashes inside
/// brackets: `"Scale 75%/To Pdf (from Markdown/EPUB)"` ->
/// `["Scale 75%", "To Pdf (from Markdown/EPUB)"]`.
///
/// A plain `split('/')` turned the shipped "To Pdf (from Markdown/Typst/EPUB)"
/// preset into the folders "To Pdf (from Markdown" > "Typst" and the item
/// "EPUB)" in the right-click menu and the settings list.
pub fn split_name_parts(name: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, c) in name.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            '/' if depth == 0 => {
                parts.push(&name[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&name[start..]);
    parts
}

impl ConversionPreset {
    /// The preset name without its folders: `"Scale 75%/To Mp4"` -> `"To Mp4"`.
    pub fn short_name(&self) -> &str {
        split_name_parts(&self.name)
            .pop()
            .unwrap_or(self.name.as_str())
    }

    /// The folders a preset is filed under (`"Scale 75%/To Mp4"` ->
    /// `["Scale 75%"]`). The C# original shows them as sub-menus.
    pub fn folders(&self) -> Vec<&str> {
        let mut parts = split_name_parts(&self.name);
        parts.pop();
        parts.retain(|p| !p.trim().is_empty());
        parts
    }

    /// Load-time fix-ups (C# parity): input types are trimmed, lower-cased and
    /// de-duplicated, and the legacy `Bitrate` / `Encoding` keys written by old
    /// versions are renamed to `AudioBitrate` / `AudioEncodingMode`.
    pub fn normalize(&mut self) {
        let mut seen: Vec<CompactStr> = Vec::with_capacity(self.input_types.len());
        for t in &self.input_types {
            let t = t.trim().trim_start_matches('.').to_lowercase();
            if !t.is_empty() && !seen.iter().any(|s| s == t.as_str()) {
                seen.push(CompactStr::new(t));
            }
        }
        self.input_types = seen;

        for (legacy, current) in [
            ("Bitrate", "AudioBitrate"),
            ("Encoding", "AudioEncodingMode"),
        ] {
            if let Some(pos) = self.settings.iter().position(|s| s.key == legacy) {
                let value = self.settings.remove(pos).value;
                if self.get_setting_value(current).is_none() {
                    self.set_setting_value(current, &value);
                }
            }
        }
    }

    pub fn get_settings_map(&self) -> AHashMap<CompactStr, CompactStr> {
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
            setting.value = CompactStr::new(value);
        } else {
            self.settings.push(PresetSetting {
                key: CompactStr::new(key),
                value: CompactStr::new(value),
            });
        }
    }

    /// Drops a setting entirely, returning whether it was present.
    ///
    /// Not equivalent to `set_setting_value(key, "")`: several engines read an
    /// empty value as "use the default", but the key would still be written back
    /// out to `Settings.user.xml` and keep showing up in the editor.
    pub fn remove_setting(&mut self, key: &str) -> bool {
        let before = self.settings.len();
        self.settings.retain(|s| s.key != key);
        self.settings.len() != before
    }

    pub fn migrate(&mut self, old_version: i32) {
        if old_version <= 2
            && let Some(speed) = self.get_setting_value("VideoEncodingSpeed")
        {
            let mapped = match speed {
                "Ultra Fast" => "UltraFast",
                "Super Fast" => "SuperFast",
                "Very Fast" => "VeryFast",
                "Very Slow" => "VerySlow",
                _ => "",
            };
            if !mapped.is_empty() {
                self.set_setting_value("VideoEncodingSpeed", mapped);
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
