use std::io;

use zip::result::ZipError;

/// An error that occurs when attempting to load Gradebook information from a zip file.
#[derive(Debug, thiserror::Error)]
pub enum GradebookLoadError {
    #[error("IO error occurred reading from zip file: {0}")]
    IO(#[from] io::Error),

    #[error("failed to open zip file: {0}")]
    ZipOpen(ZipError),

    #[error("failed to parse {filename}: {inner}")]
    Datafile { filename: String, inner: Box<dyn std::error::Error> },

    #[error("gradebook contains no submissions")]
    Empty,
}

impl From<ZipError> for GradebookLoadError {
    fn from(value: ZipError) -> Self {
        match value {
            ZipError::Io(err) => Self::IO(err),
            err => Self::ZipOpen(err),
        }
    }
}
