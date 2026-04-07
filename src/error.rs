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
    Datafile {
        filename: String,
        inner: datafile::DatafileError,
    },

    #[error("submission {datafile} specified a filename not present in zip file: {filename}")]
    FileNotFound { datafile: String, filename: String },

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

/// Errors and types related specifically the parsing of Blackboard's "datafiles."
pub mod datafile {
    use std::fmt::Display;

    #[derive(Debug, Clone, thiserror::Error)]
    pub enum DatafileError {
        #[error("duplicate field '{field}' on line {line_num}")]
        DuplicateField { field: Field, line_num: usize },

        #[error("failed to parse field '{field}' on line {line_num}: {inner}")]
        FieldParse {
            field: Field,
            inner: FieldParseError,
            line_num: usize,
        },

        #[error("unexpected text at line {line_num}: `{text}`")]
        Unexpected { text: Box<str>, line_num: usize },

        #[error("unknown field '{field}'")]
        UnknownField { field: Box<str>, line_num: usize },

        #[error("field '{field}' not found")]
        MissingField { field: Field },
    }

    /// The field at which an error occurred.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum Field {
        Name,
        Assignment,
        DateSubmitted,
        CurrentGrade,
        SubmissionField,
        Comments,
        Files,
    }

    impl Display for Field {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(match self {
                Field::Name => "Name",
                Field::Assignment => "Assignment",
                Field::DateSubmitted => "Date Submitted",
                Field::CurrentGrade => "Current Grade",
                Field::SubmissionField => "Submission Field",
                Field::Comments => "Comments",
                Field::Files => "Files",
            })
        }
    }

    #[derive(Debug, Clone, thiserror::Error)]
    #[error(transparent)]
    pub enum FieldParseError {
        Name(#[from] NameError),
        DateSubmitted(#[from] chrono::ParseError),
        Files(#[from] FilesError),
    }

    #[derive(Debug, Clone, thiserror::Error)]
    pub enum NameError {
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
    pub enum FilesError {
        #[error("file is missing an 'Original filename' field")]
        MissingOriginal,

        #[error("file is missing a 'Filename' field")]
        MissingArchive,
    }
}
