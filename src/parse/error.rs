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

// Additional Gradebook error cases:
//
// - A datafile referencing a file which does not appear in the archive

// Datafile errors:
//
// - ✔️ Unexpected EOF before all four of the initial fields are encountered
// - ✔️ Malformed field:
//   - Missing colon in first four fields' lines
//   - Empty text before colon in first four fields' lines
// - ✔️ Unknown field name in first four fields
// - ✔️ Duplicate field name in four four fields
// - ✔️ Missing 'Name', 'Assignment', or 'Date Submitted' fields
// - ✔️ Malformed 'Name' field:
//   - Missing '(' or ')'
//   - Fullname or username is empty
// - ✔️ Malformed datetime for 'Date Submitted' (wraps chrono)
// - ✔️ Unexpected EOF before hitting 'Submission Field'
// - ✔️ Hitting a different line instead of 'Submission Field' after the first four fields
// - ✔️ Unexpected EOF before hitting 'Comments' section
// - ✔️ Unexpected EOF before hitting 'Files' section
// - ✔️ Malformed 'Files' section:
//   - Duplicate field before seeing both fields
//   - Unknown field
//
// All of these carry a line number with them.

#[derive(Debug, Clone, thiserror::Error)]
#[error("line {line}: {kind}")]
pub struct DatafileError {
    line: usize,
    kind: DfErrorKind,
}

// Hmm... at this rate, with so many variants (some of which literally represent one path), I think just wrapping a
// string might be better... this struct is 40 bytes vs. a `String`'s 24 or `Box<str>`'s 16.

#[derive(Debug, Clone, thiserror::Error)]
enum DfErrorKind {
    #[error("reached EOF before finding '{0}' field")]
    EarlyEof(&'static str),

    #[error("missing '{0}' field")]
    MissingField(&'static str),

    #[error("malformed field: {0}")]
    MalformedField(#[from] FieldError),

    #[error("unknown field '{0}'")]
    UnknownField(String),

    #[error("duplicate field '{0}'")]
    DuplicateField(&'static str),

    #[error("expected '{0}', found '{1}'")]
    Unexpected(&'static str, String),

    #[error("invalid datetime: {0}")]
    BadDatetime(#[from] chrono::ParseError),

    #[error("failed to parse 'Names' field: {0}")]
    BadNameField(#[from] NameFieldError),

    #[error("failed to parse 'Files' section: {0}")]
    BadFilesSection(#[from] FilesSectionError),
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
enum FieldError {
    #[error("expected ':' before EOL")]
    NoColon,
    #[error("expected name before ':'")]
    NoName,
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
enum NameFieldError {
    #[error("expected '(' around username")]
    MissingL,
    #[error("expected ')' around username")]
    MissingR,
    #[error("student's name is missing")]
    EmptyFullname,
    #[error("student's username is missing")]
    EmptyUsername,
}

#[derive(Debug, Clone, thiserror::Error)]
enum FilesSectionError {
    #[error("encountered two 'Original filename' fields in a row without a 'Filename' in between")]
    DuplicateOriginal,
    #[error("encountered two 'Filename' fields in a row without an 'Original filename' in between")]
    DuplicateZipped,
    #[error("unknown field '{0}'")]
    UnknownField(String),
}
