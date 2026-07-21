use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputType {
    #[default]
    None,
    Aac,
    Avi,
    Avif,
    Flac,
    Gif,
    Ico,
    Jpg,
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
            OutputType::Flac => "flac",
            OutputType::Gif => "gif",
            OutputType::Ico => "ico",
            OutputType::Jpg => "jpg",
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

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputPostConversionAction {
    #[default]
    None,
    MoveInArchiveFolder,
    Delete,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
pub enum HardwareAccelerationMode {
    #[default]
    Off,
    #[serde(rename = "CUDA")]
    Cuda,
    #[serde(rename = "AMF")]
    Amf,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
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
pub fn get_extension_category(ext: &str) -> &'static str {
    match ext {
        "aac" | "aiff" | "ape" | "flac" | "mp3" | "m4a" | "m4b" | "oga" | "ogg" | "opus"
        | "wav" | "wma" => "Audio",
        "3gp" | "3gpp" | "avi" | "bik" | "flv" | "m4v" | "mp4" | "mpg" | "mpeg" | "mov" | "mkv"
        | "ogv" | "rm" | "ts" | "vob" | "webm" | "wmv" => "Video",
        "arw" | "avif" | "bmp" | "cr2" | "dds" | "dng" | "exr" | "heic" | "ico" | "jfif"
        | "jpg" | "jpeg" | "nef" | "png" | "psd" | "raf" | "tga" | "tif" | "tiff" | "svg"
        | "xcf" | "webp" => "Image",
        "gif" => "Animated Image",
        "pdf" | "doc" | "docx" | "ppt" | "pptx" | "odp" | "ods" | "odt" | "xls" | "xlsx" => {
            "Document"
        }
        _ => "Misc",
    }
}

pub fn is_output_type_compatible_with_category(output_type: OutputType, category: &str) -> bool {
    if category == "Misc" {
        return true;
    }
    match output_type {
        OutputType::Aac
        | OutputType::Flac
        | OutputType::Mp3
        | OutputType::Ogg
        | OutputType::Wav => category == "Audio" || category == "Video",
        OutputType::Avi
        | OutputType::Mkv
        | OutputType::Mp4
        | OutputType::Ogv
        | OutputType::Webm => category == "Video" || category == "Animated Image",
        OutputType::Avif
        | OutputType::Ico
        | OutputType::Jpg
        | OutputType::Png
        | OutputType::Webp => {
            category == "Image" || category == "Document" || category == "Animated Image"
        }
        OutputType::Gif => {
            category == "Image" || category == "Video" || category == "Animated Image"
        }
        OutputType::Pdf => category == "Image" || category == "Document",
        OutputType::None => false,
    }
}
