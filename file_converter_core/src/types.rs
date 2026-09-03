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
            OutputType::Wav => "wav",
            OutputType::Webm => "webm",
            OutputType::Webp => "webp",
            OutputType::None => "",
        }
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
        | "md" | "markdown" | "typ" | "txt" | "html" | "htm" => FileCategory::Document,
        _ => FileCategory::Misc,
    }
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
        OutputType::None => false,
    }
}
