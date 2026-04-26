use std::fmt::Debug;
use std::io;

use zip::result::ZipError;

use crate::parse::DatafileError;

/// An error that occurs when attempting to read Gradebook information from a `.zip` file.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("i/o error occurred reading from zip file: {0}")]
    IO(#[source] io::Error),

    #[error("failed to open zip file: {0}")]
    ZipOpen(#[source] ZipError),

    #[error(transparent)]
    InvalidGradebook(#[from] GradebookError),
}

impl From<io::Error> for Error {
    fn from(inner: io::Error) -> Self {
        Error::IO(inner)
    }
}

/// An error that occurs when a valid Gradebook cannot be parsed from a `.zip` file.
#[derive(thiserror::Error)]
#[error(transparent)]
pub struct GradebookError(Box<GradebookErrorRepr>);

impl Debug for GradebookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(&self.0, f)
    }
}

impl GradebookError {
    pub(super) fn datafile(filename: &str, inner: DatafileError) -> Self {
        Self::from(GradebookErrorRepr::InvalidDatafile { filename: filename.into(), inner })
    }

    pub(super) fn bad_filename(datafile_name: &str, bad_filename: &str) -> Self {
        Self::from(GradebookErrorRepr::BadFileReference {
            datafile: datafile_name.into(),
            filename: bad_filename.into(),
        })
    }

    pub(super) fn empty() -> Self {
        Self::from(GradebookErrorRepr::EmptyGradebook)
    }
}

impl From<GradebookErrorRepr> for GradebookError {
    fn from(inner: GradebookErrorRepr) -> Self {
        Self(Box::new(inner))
    }
}

/// Private inner representation of [`GradebookError`].
#[derive(Debug, thiserror::Error)]
enum GradebookErrorRepr {
    #[error("failed to parse {filename}: {inner}")]
    InvalidDatafile {
        filename: Box<str>,
        #[source]
        inner: DatafileError,
    },

    #[error("submission {datafile} referenced a filename not found in zip file: {filename}")]
    BadFileReference { datafile: Box<str>, filename: Box<str> },

    #[error("gradebook contains no submissions")]
    EmptyGradebook,
}
