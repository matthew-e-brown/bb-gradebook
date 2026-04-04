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

/*

// =====================================================================================================================
// See the note in `datafile.rs` about PREVIOUS ATTEMPTS.
// =====================================================================================================================

/// An error that occurs when trying to parse one of Blackboard's `.txt` info files.
#[derive(Debug, Clone)]
pub struct DatafileError {
    /// The field that the error occurred in/around, if applicable.
    field: Option<&'static str>,
    /// The encountered error.
    inner: DatafileErrorInner,
}

impl DatafileError {
    /* pub(crate) fn syntax(inner: SyntaxError) -> Self {
        Self(DatafileErrorInner::Syntax(inner))
    }

    pub(crate) fn syntax_field(field: &'static str, inner: SyntaxError) -> Self {
        Self(DatafileErrorInner::SyntaxField(inner, field))
    }

    pub(crate) fn datetime(inner: chrono::ParseError) -> Self {
        Self(DatafileErrorInner::DateFormat(inner))
    } */

    pub(crate) fn expected() -> Self {
        todo!();
    }

    pub(crate) fn for_field(mut self, field: &'static str) -> Self {
        self.field = Some(field);
        self
    }
}

// The "actual" error type, representing all the different *reasons* parsing failed.
#[derive(Debug, Clone, thiserror::Error)]
pub(crate) enum DatafileErrorInner {
    // A general syntax error within the file.
    #[error(transparent)]
    Syntax(#[from] SyntaxError),

    // A syntax error while looking for a specific field.
    #[error("{0} while attempting to parse field {1}")]
    SyntaxField(#[source] SyntaxError, &'static str),

    #[error("invalid or unsupported date/time format: {0}")]
    DateFormat(#[from] chrono::ParseError),
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("expected {expected} at position {pos} (found {actual})")]
pub(crate) struct SyntaxError {
    pub pos: usize,
    pub expected: Box<str>,
    pub actual: Box<str>,
}

pub(crate) struct Unexpected {
    pub pos: usize,
}

pub(crate) struct Expected {

}

// impl From<SyntaxError> for DatafileParseError {
//     fn from(inner: SyntaxError) -> Self {
//         Self::syntax(inner)
//     }
// }

// impl From<chrono::ParseError> for DatafileParseError {
//     fn from(inner: chrono::ParseError) -> Self {
//         Self::datetime(inner)
//     }
// }
*/
