use std::path::{Path, PathBuf};

use clap::ValueEnum;

/// Unzip a "gradebook" zip file from Blackboard into a neatly organized folder, with subfolders named after each
/// student/attempt.
///
/// By default, any `.zip`, `.tar`, `.tar.gz`, or `.rar` files in a submission will be automatically extracted into
/// folders.
#[derive(Debug, clap::Parser)]
#[command(version)]
pub struct Args {
    /// Path to a .zip file downloaded from the "Assignment File Download" tab in Blackboard's Grade Center.
    #[arg(name = "gradebook", value_name = "FILE")]
    path: PathBuf,

    /// The directory into which to extract student submissions.
    ///
    /// If one is not provided, a new directory will be created based on the name of the assignment.
    output_dir: Option<PathBuf>,

    /// Disable automatic extraction of student-submitted archives altogether.
    #[arg(long)]
    no_unzip: bool,

    /// Disable extraction of student-submitted archives larger than the given size.
    ///
    /// Sizes may be given with multiplicative unit suffixes. The following suffixes are supported:
    /// - B=1,
    /// - K=1024
    /// - M=1048576
    /// - G=1073741824
    /// - KB=1000
    /// - MB=1000000
    /// - GB=1000000000
    #[arg(long, value_name = "SIZE", value_parser = parse_filesize)]
    unzip_max: Option<u64>,
}

#[allow(unused)]
impl Args {
    /// Parse a copy of [`Args`] from the command line, and exit on error.
    pub fn parse() -> Self {
        // This way we don't need to bring `clap::Parser` into scope elsewhere.
        <Self as clap::Parser>::parse()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn output_dir(&self) -> Option<&Path> {
        self.output_dir.as_deref()
    }

    pub fn no_unzip(&self) -> bool {
        self.no_unzip
    }

    pub fn unzip_max(&self) -> Option<u64> {
        self.unzip_max
    }
}

#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameStyle {
    /// Use students' usernames (johnsmith).
    Username,
    /// Use students' full names (John Smith).
    FirstLast,
    /// Use students' full names in "Last, First" order (Smith, John).
    LastFirst,
}

#[derive(Debug, Clone, thiserror::Error)]
enum FilesizeError {
    #[error(transparent)]
    InvalidInt(#[from] std::num::ParseIntError),
    #[error("'{0}' is not a supported byte suffix")]
    InvalidSuffix(String),
}

fn parse_filesize(s: &str) -> Result<u64, FilesizeError> {
    let i = s
        .char_indices()
        .rfind(|(_, c)| c.is_ascii_digit()) // Find last digit before the suffix starts.
        .map(|(i, _)| i + 1) // The position *after* the first digit is the start of the suffix.
        .unwrap_or(s.len()); // If there is no suffix, the prefix goes all the way to the end.
    let (num, suffix) = s.split_at(i);

    let num = num.parse::<u64>()?;
    let mult = match suffix {
        "" => 1,
        "B" | "b" => 1,
        "K" | "k" => 1024,
        "M" | "m" => 1024 * 1024,
        "G" | "g" => 1024 * 1024 * 1024,
        "KB" | "kb" => 1000,
        "MB" | "mb" => 1000 * 1000,
        "GB" | "gb" => 1000 * 1000 * 1000,
        x => return Err(FilesizeError::InvalidSuffix(x.to_string())),
    };

    Ok(num * mult)
}
