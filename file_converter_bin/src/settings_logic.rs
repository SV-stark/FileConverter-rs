//! GUI-free rules behind the settings window: preset names and folders,
//! validation messages, input-type shortcuts and history filtering.
//!
//! Kept apart from the Slint wiring so every rule is unit-tested.

use file_converter_core::settings::{CompactStr, ConversionPreset, Settings, split_name_parts};
use file_converter_core::types::{
    FileCategory, OutputType, get_extension_category, is_output_type_compatible_with_category,
};

/// Splits `"Scale 75%/To Mp4"` into `("Scale 75%", "To Mp4")`.
pub fn split_preset_name(name: &str) -> (String, String) {
    let mut parts = split_name_parts(name);
    let short = parts.pop().unwrap_or("").to_string();
    let folder = parts
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    (folder, short)
}

/// Inverse of [`split_preset_name`]: empty folder segments are dropped and
/// every part is trimmed, so `" A / /B"` + `" x "` becomes `"A/B/x"`.
pub fn join_preset_name(folder: &str, short: &str) -> String {
    let mut parts: Vec<&str> = split_name_parts(folder)
        .into_iter()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    parts.push(short.trim());
    parts.join("/")
}

/// The problem with preset `idx` that must be fixed before saving, or an
/// empty string. These break the right-click menu or the command line, which
/// both address presets by name.
pub fn preset_warning(settings: &Settings, idx: usize) -> String {
    let Some(p) = settings.conversion_presets.get(idx) else {
        return String::new();
    };
    if p.short_name().trim().is_empty() {
        return "Give this preset a name: it is how you find it in the right-click menu.".into();
    }
    if p.name.contains('"') {
        return "Preset names can't contain double quotes (\").".into();
    }
    if let Some(other) = settings
        .conversion_presets
        .iter()
        .enumerate()
        .find(|(i, o)| *i != idx && o.name.eq_ignore_ascii_case(&p.name))
    {
        return format!(
            "Another preset is already called \"{}\". Names must be unique so the right-click menu can tell them apart.",
            other.1.name
        );
    }
    if p.output_file_name_template.trim().is_empty() {
        return "The output file name is empty. Pick one of the suggestions below.".into();
    }
    String::new()
}

/// First preset that can't be saved, with the reason.
pub fn validate_for_save(settings: &Settings) -> Result<(), (usize, String)> {
    for idx in 0..settings.conversion_presets.len() {
        let warning = preset_warning(settings, idx);
        if !warning.is_empty() {
            return Err((idx, warning));
        }
    }
    Ok(())
}

/// Explains which listed input types the output format can't be made from
/// (they are silently never offered in the menu otherwise).
pub fn input_warning(p: &ConversionPreset) -> String {
    let bad: Vec<&str> = p
        .input_types
        .iter()
        .map(|t| t.as_str())
        .filter(|t| {
            let cat = get_extension_category(t);
            cat != FileCategory::Misc
                && !is_output_type_compatible_with_category(p.output_type, cat)
        })
        .collect();
    if bad.is_empty() {
        return String::new();
    }
    format!(
        "{} files can't be made from {}, so the preset won't be offered for them.",
        p.output_type.extension().to_uppercase(),
        bad.join(", ")
    )
}

const AUDIO_INPUTS: &[&str] = &[
    "aac", "aiff", "ape", "flac", "m4a", "mp3", "oga", "ogg", "opus", "wav", "wma",
];
const VIDEO_INPUTS: &[&str] = &[
    "3gp", "avi", "flv", "m4v", "mkv", "mov", "mp4", "mpeg", "mpg", "ogv", "ts", "webm", "wmv",
];
const IMAGE_INPUTS: &[&str] = &[
    "avif", "bmp", "gif", "heic", "ico", "jfif", "jpeg", "jpg", "jxl", "png", "svg", "tga", "tif",
    "tiff", "webp",
];
const DOCUMENT_INPUTS: &[&str] = &[
    "doc", "docx", "epub", "htm", "html", "md", "odp", "ods", "odt", "pdf", "ppt", "pptx", "rtf",
    "txt", "xls", "xlsx",
];

/// Common file extensions of a kind ("Audio", "Video", "Image", "Document").
pub fn extensions_for(kind: &str) -> &'static [&'static str] {
    match kind {
        "Audio" => AUDIO_INPUTS,
        "Video" => VIDEO_INPUTS,
        "Image" => IMAGE_INPUTS,
        "Document" => DOCUMENT_INPUTS,
        _ => &[],
    }
}

/// Adds the common extensions of `kind` that `output` can be made from.
/// Returns how many were new.
pub fn add_input_kind(inputs: &mut Vec<CompactStr>, kind: &str, output: OutputType) -> usize {
    let mut added = 0;
    for ext in extensions_for(kind) {
        let cat = get_extension_category(ext);
        if !is_output_type_compatible_with_category(output, cat) {
            continue;
        }
        if !inputs.iter().any(|i| i.eq_ignore_ascii_case(ext)) {
            inputs.push(CompactStr::new(ext));
            added += 1;
        }
    }
    added
}

/// Parses the "Extensions" box: commas, semicolons or spaces separate
/// entries; dots and case are ignored; duplicates are dropped.
pub fn parse_input_types(text: &str) -> Vec<CompactStr> {
    let mut out: Vec<CompactStr> = Vec::new();
    for part in text.split([',', ';', ' ']) {
        let ext = part.trim().trim_start_matches('.').to_lowercase();
        if !ext.is_empty() && !out.iter().any(|e| e == ext.as_str()) {
            out.push(CompactStr::new(ext));
        }
    }
    out
}

/// Does preset `p` belong in the list for this search text and category chip?
pub fn preset_matches(p: &ConversionPreset, query_lower: &str, category: &str) -> bool {
    let kind = p.output_type.kind();
    let cat_ok = match category {
        "" | "All" => true,
        "Doc" => kind == "Document",
        other => kind == other,
    };
    if !cat_ok {
        return false;
    }
    query_lower.is_empty()
        || p.name.to_lowercase().contains(query_lower)
        || p.output_type.extension().contains(query_lower)
        || p.input_types
            .iter()
            .any(|t| t.to_lowercase().contains(query_lower))
}

/// Short status, detail message and success flag of a history entry. The
/// stored status is "Done", "Failed (reason)", "Canceled" or "Finished".
pub fn history_status(status: &str) -> (String, String, bool) {
    if status == "Done" || status == "Finished" {
        return ("Converted".into(), String::new(), true);
    }
    if let Some(rest) = status.strip_prefix("Failed") {
        let reason = rest
            .trim()
            .strip_prefix('(')
            .and_then(|r| r.strip_suffix(')'))
            .unwrap_or(rest.trim());
        return ("Failed".into(), reason.to_string(), false);
    }
    (status.to_string(), String::new(), false)
}

/// Does a history entry match the search text and the All/Done/Failed chip?
pub fn history_matches(
    preset_name: &str,
    input_path: &str,
    output_path: &str,
    status: &str,
    query_lower: &str,
    filter: &str,
) -> bool {
    let (_, _, ok) = history_status(status);
    let filter_ok = match filter {
        "Done" => ok,
        "Failed" => !ok,
        _ => true,
    };
    filter_ok
        && (query_lower.is_empty()
            || preset_name.to_lowercase().contains(query_lower)
            || input_path.to_lowercase().contains(query_lower)
            || output_path.to_lowercase().contains(query_lower))
}

/// A sample input file of the preset's kind, for the output-path preview.
pub fn sample_input(output: OutputType) -> &'static str {
    match output.kind() {
        "Video" => "C:\\Videos\\Holiday\\clip.mov",
        "Image" => "C:\\Pictures\\Trip\\photo.heic",
        "Document" => "C:\\Documents\\Work\\report.docx",
        _ => "C:\\Music\\Album\\sample_track.flac",
    }
}

/// Label of an output format in the editor's drop-down: "MP3 · Audio".
pub fn output_type_label(t: OutputType) -> String {
    format!("{} · {}", t.extension().to_uppercase(), t.kind())
}

#[cfg(test)]
mod tests {
    use super::*;
    use file_converter_core::types::InputPostConversionAction;

    fn preset(name: &str, output: OutputType, inputs: &[&str]) -> ConversionPreset {
        ConversionPreset {
            name: name.to_string(),
            output_type: output,
            is_default_settings: false,
            input_types: inputs.iter().map(|i| CompactStr::new(i)).collect(),
            input_post_conversion_action: InputPostConversionAction::None,
            settings: vec![],
            output_file_name_template: "(p)(f)".to_string(),
        }
    }

    fn settings(presets: Vec<ConversionPreset>) -> Settings {
        let mut s = Settings::load_from_str(crate::DEFAULT_SETTINGS_XML).expect("defaults load");
        s.conversion_presets = presets;
        s
    }

    #[test]
    fn names_split_and_join_round_trip() {
        assert_eq!(
            split_preset_name("Scale 75%/To Mp4"),
            ("Scale 75%".to_string(), "To Mp4".to_string())
        );
        assert_eq!(
            split_preset_name("To Mp3"),
            (String::new(), "To Mp3".to_string())
        );
        assert_eq!(
            split_preset_name("A/B/To Png"),
            ("A/B".to_string(), "To Png".to_string())
        );
        assert_eq!(join_preset_name("", "To Mp3"), "To Mp3");
        assert_eq!(join_preset_name(" A / /B ", " To Png "), "A/B/To Png");
        let (folder, short) = split_preset_name("A/B/To Png");
        assert_eq!(join_preset_name(&folder, &short), "A/B/To Png");

        // Slashes inside brackets belong to the name.
        assert_eq!(
            split_preset_name("Docs/To Pdf (from Markdown/EPUB)"),
            (
                "Docs".to_string(),
                "To Pdf (from Markdown/EPUB)".to_string()
            )
        );
        assert_eq!(
            join_preset_name("Docs", "To Pdf (from Markdown/EPUB)"),
            "Docs/To Pdf (from Markdown/EPUB)"
        );
    }

    #[test]
    fn warnings_catch_names_that_break_the_menu() {
        let s = settings(vec![
            preset("To Mp3", OutputType::Mp3, &["wav"]),
            preset("to mp3", OutputType::Mp3, &["flac"]),
            preset("Folder/", OutputType::Png, &[]),
            preset("Say \"hi\"", OutputType::Png, &[]),
        ]);
        assert!(preset_warning(&s, 0).contains("already called"));
        assert!(preset_warning(&s, 2).contains("name"));
        assert!(preset_warning(&s, 3).contains("quotes"));
        assert_eq!(validate_for_save(&s).unwrap_err().0, 0);

        let ok = settings(vec![preset("To Mp3", OutputType::Mp3, &["wav"])]);
        assert!(preset_warning(&ok, 0).is_empty());
        assert!(validate_for_save(&ok).is_ok());

        let mut empty_template = preset("To Mp3", OutputType::Mp3, &["wav"]);
        empty_template.output_file_name_template = "  ".into();
        let s = settings(vec![empty_template]);
        assert!(preset_warning(&s, 0).contains("file name"));
    }

    #[test]
    fn input_warning_lists_impossible_inputs() {
        let p = preset("To Mp3", OutputType::Mp3, &["wav", "png", "mp4", "xyz"]);
        let w = input_warning(&p);
        assert!(w.contains("png"), "{w}");
        assert!(
            !w.contains("wav") && !w.contains("mp4") && !w.contains("xyz"),
            "{w}"
        );
        assert!(input_warning(&preset("To Mp3", OutputType::Mp3, &["wav"])).is_empty());
    }

    #[test]
    fn input_kinds_only_add_compatible_extensions() {
        let mut inputs = vec![CompactStr::new("wav")];
        let added = add_input_kind(&mut inputs, "Audio", OutputType::Mp3);
        assert_eq!(added, AUDIO_INPUTS.len() - 1, "wav was already there");
        assert_eq!(add_input_kind(&mut inputs, "Image", OutputType::Mp3), 0);
        let mut pdf = Vec::new();
        assert!(add_input_kind(&mut pdf, "Image", OutputType::Pdf) > 0);
        assert!(add_input_kind(&mut pdf, "Document", OutputType::Pdf) > 0);
    }

    #[test]
    fn input_box_parsing_is_forgiving() {
        let parsed = parse_input_types(" .WAV, flac;mp3  ogg,,wav ");
        let parsed: Vec<&str> = parsed.iter().map(|c| c.as_str()).collect();
        assert_eq!(parsed, vec!["wav", "flac", "mp3", "ogg"]);
        assert!(parse_input_types("  ").is_empty());
    }

    #[test]
    fn preset_filter_by_text_and_category() {
        let p = preset("Scale 50%/To Jpg", OutputType::Jpg, &["png", "heic"]);
        assert!(preset_matches(&p, "", "All"));
        assert!(preset_matches(&p, "heic", "Image"));
        assert!(preset_matches(&p, "scale", ""));
        assert!(preset_matches(&p, "jpg", "All"));
        assert!(!preset_matches(&p, "", "Audio"));
        assert!(!preset_matches(&p, "flac", "All"));
        let pdf = preset("To Pdf", OutputType::Pdf, &[]);
        assert!(preset_matches(&pdf, "", "Doc"));
    }

    #[test]
    fn history_status_and_filter() {
        assert_eq!(
            history_status("Done"),
            ("Converted".into(), String::new(), true)
        );
        assert_eq!(
            history_status("Failed (Disk full)"),
            ("Failed".into(), "Disk full".into(), false)
        );
        assert!(!history_status("Canceled").2);
        assert!(history_matches(
            "To Mp3",
            "C:\\a.wav",
            "C:\\a.mp3",
            "Done",
            "a.wav",
            "Done"
        ));
        assert!(!history_matches(
            "To Mp3",
            "C:\\a.wav",
            "",
            "Failed (x)",
            "",
            "Done"
        ));
        assert!(history_matches(
            "To Mp3",
            "C:\\a.wav",
            "",
            "Failed (x)",
            "mp3",
            "Failed"
        ));
        assert!(!history_matches(
            "To Mp3",
            "C:\\a.wav",
            "",
            "Done",
            "png",
            "All"
        ));
    }

    #[test]
    fn every_output_type_has_a_label_and_sample() {
        for t in OutputType::ALL {
            assert!(output_type_label(t).contains(t.kind()));
            assert!(sample_input(t).contains('.'));
        }
        assert_eq!(output_type_label(OutputType::Mp3), "MP3 · Audio");
    }
}
