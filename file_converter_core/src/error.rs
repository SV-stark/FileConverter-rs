use thiserror::Error;

#[derive(Error, Debug)]
pub enum FileConverterError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    #[error("xml parsing error: {0}")]
    Xml(#[from] quick_xml::DeError),

    #[error("ffmpeg error: {0}")]
    Ffmpeg(String),

    #[error("office conversion error: {0}")]
    Office(String),

    #[error("image processing error: {0}")]
    Image(String),

    #[error("invalid preset or path: {0}")]
    Invalid(String),

    #[error("job failed: {0}")]
    JobFailed(String),

    #[error("process timeout: {0}")]
    Timeout(String),
}

pub type Result<T> = std::result::Result<T, FileConverterError>;
