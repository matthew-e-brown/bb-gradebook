use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use regex::Regex;

/// Unzip a 'gradebook' zip file from Blackboard into a neatly organized folder, with subfolders named after each
/// student.
#[derive(Parser, Debug)]
#[command(author, version)]
pub struct App {
    /// The directory into which to extract student submissions.
    ///
    /// If one is not provided, a new directory will be created based on the name of the assignment.
    #[arg(global = true)]
    output_dir: Option<PathBuf>,

    /// The naming scheme to use for student folders.
    #[arg(short = 'n', long, value_name = "STYLE", default_value = "username", global = true, value_enum)]
    name_style: NameStyle,

    /// Skip extracting students' zip files.
    ///
    /// By default, any archive files submitted by students are automatically extracted into a folder with the same name
    /// as the original file.
    #[arg(short = 'X', long, global = true)]
    no_extract: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Extract all submissions from a gradebook.
    Extract(ExtractArgs),

    /// Open a gradebook and display a list of submissions for manual handling.
    Open(OpenArgs),
}

#[derive(Args, Debug)]
pub struct CommonArgs {
    /// Path to a .zip file downloaded from the "Assignment File Download" tab in Blackboard's Grade Center.
    #[arg(name = "gradebook", value_name = "FILE")]
    path: PathBuf,
}

#[derive(Args, Debug)]
pub struct OpenArgs {
    #[command(flatten)]
    common: CommonArgs,
}

#[derive(Args, Debug)]
pub struct ExtractArgs {
    #[command(flatten)]
    common: CommonArgs,

    /// Extract only students whose names match the given pattern.
    ///
    /// Supported pattern syntax is given from the Rust `regex` crate: https://docs.rs/regex/latest/regex#syntax.
    username_pattern: Option<Regex>, // Regex type supports FromStr out of the box!
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
