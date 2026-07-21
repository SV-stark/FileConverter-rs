use thiserror::Error;

#[derive(Error, Debug)]
pub enum FileConverterError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("XML parsing error: {0}")]
    Xml(#[from] quick_xml::DeError),

    #[error("FFmpeg error: {0}")]
    Ffmpeg(String),

    #[error("Office conversion error: {0}")]
    Office(String),

    #[error("Image processing error: {0}")]
    Image(String),

    #[error("Invalid preset or path: {0}")]
    Invalid(String),

    #[error("Job failed: {0}")]
    JobFailed(String),

    #[error("Process timeout: {0}")]
    Timeout(String),
}

pub type Result<T> = std::result::Result<T, FileConverterError>;
