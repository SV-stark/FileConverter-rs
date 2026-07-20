use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum OutputType {
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

impl Default for OutputType {
    fn default() -> Self {
        OutputType::None
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum InputPostConversionAction {
    None,
    MoveInArchiveFolder,
    Delete,
}

impl Default for InputPostConversionAction {
    fn default() -> Self {
        InputPostConversionAction::None
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum HardwareAccelerationMode {
    Off,
    #[serde(rename = "CUDA")]
    Cuda,
    #[serde(rename = "AMF")]
    Amf,
}

impl Default for HardwareAccelerationMode {
    fn default() -> Self {
        HardwareAccelerationMode::Off
    }
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
