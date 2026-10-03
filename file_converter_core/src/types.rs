use serde::{Deserialize, Serialize};
use strum::{AsRefStr, Display, EnumString};

#[derive(
    Debug,
    Serialize,
    Deserialize,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    Display,
    EnumString,
    AsRefStr,
)]
pub enum OutputType {
    #[default]
    None,
    Aac,
    Avi,
    Avif,
    Epub,
    Flac,
    Gif,
    Html,
    Ico,
    Jpg,
    Jxl,
    Mkv,
    Mp3,
    Mp4,
    Ogg,
    Ogv,
    Pdf,
    Png,
    Txt,
    Wav,
    Webm,
    Webp,
}

impl OutputType {
    pub fn extension(&self) -> &'static str {
        match self {
            OutputType::Aac => "aac",
            OutputType::Avi => "avi",
            OutputType::Avif => "avif",
            OutputType::Epub => "epub",
            OutputType::Flac => "flac",
            OutputType::Gif => "gif",
            OutputType::Html => "html",
            OutputType::Ico => "ico",
            OutputType::Jpg => "jpg",
            OutputType::Jxl => "jxl",
            OutputType::Mkv => "mkv",
            OutputType::Mp3 => "mp3",
            OutputType::Mp4 => "mp4",
            OutputType::Ogg => "ogg",
            OutputType::Ogv => "ogv",
            OutputType::Pdf => "pdf",
            OutputType::Png => "png",
            OutputType::Txt => "txt",
            OutputType::Wav => "wav",
            OutputType::Webm => "webm",
            OutputType::Webp => "webp",
            OutputType::None => "",
        }
    }

    /// True when the produced file is a raster image that needs pixel data.
    pub fn is_raster_image(&self) -> bool {
        matches!(
            self,
            OutputType::Avif
                | OutputType::Gif
                | OutputType::Ico
                | OutputType::Jpg
                | OutputType::Jxl
                | OutputType::Png
                | OutputType::Webp
        )
    }

    /// True when the produced file is a textual/markup document.
    pub fn is_textual_document(&self) -> bool {
        matches!(self, OutputType::Html | OutputType::Txt)
    }
}

#[derive(
    Debug,
    Serialize,
    Deserialize,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    Display,
    EnumString,
    AsRefStr,
)]
pub enum InputPostConversionAction {
    #[default]
    None,
    MoveInArchiveFolder,
    Delete,
}

#[derive(
    Debug,
    Serialize,
    Deserialize,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    Display,
    EnumString,
    AsRefStr,
)]
pub enum HardwareAccelerationMode {
    #[default]
    Auto,
    Off,
    #[serde(rename = "CUDA")]
    Cuda,
    #[serde(rename = "AMF")]
    Amf,
    #[serde(rename = "QSV")]
    Qsv,
}

#[derive(
    Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Display, EnumString, AsRefStr,
)]
pub enum EncodingMode {
    Wav8,
    Wav16,
    Wav24,
    Wav32,
    #[serde(rename = "Mp3VBR")]
    Mp3Vbr,
    #[serde(rename = "Mp3CBR")]
    Mp3Cbr,
    #[serde(rename = "OggVBR")]
    OggVbr,
    #[serde(rename = "AacVBR")]
    AacVbr,
}

#[derive(
    Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Display, EnumString, AsRefStr,
)]
#[serde(rename_all = "PascalCase")]
pub enum VideoEncodingSpeed {
    UltraFast,
    SuperFast,
    VeryFast,
    Faster,
    Fast,
    Medium,
    Slow,
    Slower,
    VerySlow,
}
#[derive(
    Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Display, EnumString, AsRefStr,
)]
#[serde(rename_all = "PascalCase")]
pub enum FileCategory {
    Audio,
    Video,
    Image,
    #[serde(rename = "Animated Image")]
    AnimatedImage,
    Document,
    Misc,
}

pub fn get_extension_category(ext: &str) -> FileCategory {
    match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "aac" | "aiff" | "ape" | "flac" | "mp3" | "m4a" | "m4b" | "oga" | "ogg" | "opus"
        | "wav" | "wma" => FileCategory::Audio,
        "3gp" | "3gpp" | "avi" | "bik" | "flv" | "m4v" | "mp4" | "mpg" | "mpeg" | "mov" | "mkv"
        | "ogv" | "rm" | "ts" | "vob" | "webm" | "wmv" => FileCategory::Video,
        "arw" | "avif" | "bmp" | "cr2" | "dds" | "dng" | "exr" | "heic" | "heif" | "ico"
        | "jfif" | "jpg" | "jpeg" | "jxl" | "nef" | "png" | "psd" | "raf" | "tga" | "tif"
        | "tiff" | "svg" | "xcf" | "webp" => FileCategory::Image,
        "gif" => FileCategory::AnimatedImage,
        "pdf" | "doc" | "docx" | "ppt" | "pptx" | "odp" | "ods" | "odt" | "xls" | "xlsx"
        | "epub" | "mobi" | "azw" | "azw3" | "kfx" | "fb2" | "cbz" | "kepub" | "lit" | "rtf"
        | "md" | "markdown" | "typ" | "txt" | "text" | "log" | "csv" | "json" | "xml" | "html"
        | "htm" | "xhtml" => FileCategory::Document,
        _ => FileCategory::Misc,
    }
}

/// File extensions that the pure-Rust image decoders cannot handle natively.
///
/// They are still categorised as images so presets can advertise them, but the
/// engine returns an actionable error instead of a generic "unknown format".
pub const UNSUPPORTED_NATIVE_IMAGE_EXTENSIONS: &[&str] = &[
    "arw", "cr2", "dng", "nef", "raf", "psd", "psb", "xcf", "orf", "rw2", "pef", "srw", "erf",
    "kdc", "dcr", "raw", "3fr", "iiq", "bay", "cap", "dcs", "drf", "eip", "mdc", "obm", "pxn",
    "rwl", "sr2", "srf", "sti",
];

/// True for still/animated images whose container is decodable in pure Rust.
pub fn is_native_image_extension(ext: &str) -> bool {
    !UNSUPPORTED_NATIVE_IMAGE_EXTENSIONS
        .contains(&ext.trim_start_matches('.').to_ascii_lowercase().as_str())
}

/// Extensions handled by the plain-text / markup document engine (`txt`, `html`, ...).
pub const TEXT_DOCUMENT_EXTENSIONS: &[&str] = &[
    "txt", "text", "log", "csv", "json", "xml", "xhtml", "htm", "html", "rtf",
];

/// Extensions handled by the ebook engine.
pub const EBOOK_DOCUMENT_EXTENSIONS: &[&str] = &[
    "epub", "mobi", "azw", "azw3", "kfx", "fb2", "cbz", "kepub", "lit",
];

pub fn is_text_document_extension(ext: &str) -> bool {
    TEXT_DOCUMENT_EXTENSIONS.contains(&ext.trim_start_matches('.').to_ascii_lowercase().as_str())
}

pub fn is_ebook_extension(ext: &str) -> bool {
    EBOOK_DOCUMENT_EXTENSIONS.contains(&ext.trim_start_matches('.').to_ascii_lowercase().as_str())
}

/// True for any document that is delivered as (or converted to) text/markup
/// rather than through an office automation back-end.
pub fn is_textual_document_extension(ext: &str) -> bool {
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    is_text_document_extension(&ext)
        || is_ebook_extension(&ext)
        || matches!(ext.as_str(), "md" | "markdown" | "typ")
}

impl FileCategory {
    pub const fn as_str(&self) -> &'static str {
        match self {
            FileCategory::Audio => "Audio",
            FileCategory::Video => "Video",
            FileCategory::Image => "Image",
            FileCategory::AnimatedImage => "Animated Image",
            FileCategory::Document => "Document",
            FileCategory::Misc => "Misc",
        }
    }
}

pub fn get_extension_category_str(ext: &str) -> &'static str {
    get_extension_category(ext).as_str()
}

pub fn is_output_type_compatible_with_category(
    output_type: OutputType,
    category: FileCategory,
) -> bool {
    if category == FileCategory::Misc {
        return true;
    }
    match output_type {
        OutputType::Aac
        | OutputType::Flac
        | OutputType::Mp3
        | OutputType::Ogg
        | OutputType::Wav => category == FileCategory::Audio || category == FileCategory::Video,
        OutputType::Avi
        | OutputType::Mkv
        | OutputType::Mp4
        | OutputType::Ogv
        | OutputType::Webm => {
            category == FileCategory::Video || category == FileCategory::AnimatedImage
        }
        OutputType::Avif
        | OutputType::Ico
        | OutputType::Jpg
        | OutputType::Jxl
        | OutputType::Png
        | OutputType::Webp => {
            category == FileCategory::Image
                || category == FileCategory::Document
                || category == FileCategory::AnimatedImage
        }
        OutputType::Gif => {
            category == FileCategory::Image
                || category == FileCategory::Video
                || category == FileCategory::AnimatedImage
        }
        OutputType::Pdf => category == FileCategory::Image || category == FileCategory::Document,
        OutputType::Epub => category == FileCategory::Document,
        OutputType::Txt | OutputType::Html => category == FileCategory::Document,
        OutputType::None => false,
    }
}

/// Validates a preset against a concrete input file.
///
/// This mirrors the check the UI performs when highlighting presets, and is the
/// single source of truth used by the scheduler before a job is queued.
pub fn is_preset_applicable_to_file(
    preset_output: OutputType,
    input_types: &[String],
    file_path: &str,
) -> bool {
    let ext = std::path::Path::new(file_path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let category = if ext.is_empty() {
        FileCategory::Misc
    } else {
        get_extension_category(&ext)
    };

    if !is_output_type_compatible_with_category(preset_output, category) {
        return false;
    }

    if input_types.is_empty() || ext.is_empty() {
        return true;
    }

    input_types.iter().any(|it| {
        let clean = it.trim().trim_start_matches('.').to_ascii_lowercase();
        clean == "*" || clean == ext
    })
}
