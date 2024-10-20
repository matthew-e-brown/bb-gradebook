use std::cmp::Ordering;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Cursor, Read, Seek};
use std::iter::Peekable;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::Lines;
use std::sync::LazyLock;
use std::{env, io};

use chrono::NaiveDateTime;
use regex::Regex;
use thiserror::Error;
use zip::result::ZipError;
use zip::ZipArchive;

// cspell:words gradebook fullname
// cspell:words firstnamelastname firstlast

const HELP_STR: &'static str = "
Unzips a 'gradebook' zip from Blackboard into folders named after students.

A parent folder named after the assignment will be created in the current
working directory.

Usage:

    $ bb-gradebook <gradebook> [output_dir] [--full-names|-n]

Parameters:

    gradebook       A filepath to a zip downloaded from the
                    \"Assignment File Download\" tab in Blackboard.

    output_dir      A name to use for the parent directory, instead of the name
                    pulled from the submissions.

    -n
    --full-names    Use students' full names for folders, instead of their
                    shortened 'firstnamelastname' usernames.
";

const SUBMISSION_DATE_FORMAT: &'static str = "%A, %B %-d, %Y %-I:%M:%S %p %Z";
const EMPTY_SUBMISSION_FIELD: &'static str = "There is no student submission text data for this assignment.";
const EMPTY_COMMENTS_FIELD: &'static str = "There are no student comments for this assignment.";

/// Regex used to find the `.txt` files that Blackboard uses to document each student submission inside of a gradebook.
///
/// This regex ensures that the '.txt' appears right after the `attempt_<TIMESTAMP>` portion of the filename, so it
/// shouldn't ever catch any student-submitted '.txt' files (unless they submitted one that happened to have
/// `_attempt_TIMESTAMP` right at the end, which is unlikely).
///
/// The date format at the end is `YYYY-MM-DD-hh-mm-ss`.
static SUBMISSION_FILE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^.+?_[a-z]+_attempt_\d{4}(?:-\d\d){5}\.txt$").unwrap());

/// Regex used to extract a student's full name and username at the same time, since we can't just rely on reading to
/// the end of the line.
static STUDENT_NAME_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Name:\s+(?<fullname>.+?)\s+\((?<username>[a-z]+)\)$").unwrap());


#[derive(Debug, Error)]
enum GradebookError {
    #[error("could not read gradebook file: {0}")]
    IORead(#[from] io::Error),

    #[error("could not decompress gradebook file: {0}")]
    Extract(#[from] ZipError),

    #[error("the provided gradebook is empty")]
    Empty,
}

#[derive(Debug, Error)]
enum SubmissionError {
    #[error("failed to write a submitted file for Student '{student}' (attempt {attempt}):\n{detail}")]
    IOWrite {
        student: String,
        attempt: NaiveDateTime,
        detail: io::Error,
    },

    #[error("failed to unzip a submitted zip for {student} (attempt {attempt}):\n{detail}")]
    Extract {
        student: String,
        attempt: NaiveDateTime,
        detail: ZipError,
    },
}

impl SubmissionError {
    pub fn from_io(submission: &Submission, detail: io::Error) -> Self {
        let student = submission.fullname.to_string();
        let attempt = submission.datetime;
        Self::IOWrite { student, attempt, detail }
    }

    pub fn from_zip(submission: &Submission, detail: ZipError) -> Self {
        let student = submission.fullname.to_string();
        let attempt = submission.datetime;
        Self::Extract { student, attempt, detail }
    }
}


fn main() -> ExitCode {
    // Get the input filename from arguments
    // --------------------------------------------------------------------------------------------

    let args = env::args().skip(1).collect::<Vec<_>>();

    let mut use_full_names = false;
    for arg in &args {
        match &arg[..] {
            "-n" | "--full-names" => use_full_names = true,
            "-h" | "--help" => {
                println!("{HELP_STR}");
                return ExitCode::SUCCESS;
            },
            _ => (),
        }
    }

    // Filter out non-positional arguments
    let mut pos_args = args.iter().filter(|arg| !arg.starts_with("-"));

    let Some(archive_path) = pos_args.next() else {
        eprintln!("Please provide a 'gradebook' zip downloaded from Blackboard to unzip.");
        return ExitCode::FAILURE;
    };

    let out_dir = pos_args.next().map(|string| &string[..]); // borrow owned string from arguments.

    // Process the entire gradebook, erroring
    match process_gradebook(&archive_path, out_dir, use_full_names) {
        Ok(results) => {
            let n_total = results.len();
            let errors = results.into_iter().filter_map(|r| r.err()).collect::<Vec<_>>();

            let n_err = errors.len();
            let n_ok = n_total - n_err;

            println!("\nSuccessfully extracted {n_ok} submissions.");

            if n_err > 0 {
                eprintln!("Encountered {n_err} errors when extracting:");
                for error in errors {
                    eprintln!("\t{error}");
                }

                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        },
        Err(err) => {
            eprint!("{}", err);
            ExitCode::FAILURE
        },
    }
}


fn process_gradebook(
    archive_path: &str,
    out_directory: Option<&str>,
    use_full_names: bool,
) -> Result<Vec<Result<(), SubmissionError>>, GradebookError> {
    // Read file into memory so that we can run through it multiple times
    // --------------------------------------------------------------------------------------------

    println!("Loading zip file...");
    let archive_data = fs::read(archive_path)?;

    // Open those bytes as a zip archive
    // --------------------------------------------------------------------------------------------

    let cursor = Cursor::new(archive_data);
    let mut gradebook = ZipArchive::new(cursor)?;

    if gradebook.len() == 0 {
        return Err(GradebookError::Empty);
    }

    // Get all of Blackboard's text files
    // --------------------------------------------------------------------------------------------

    println!("Parsing submission data...");

    // We need to allocate and collect the filenames first because pulling the actual files out of the archive requires
    // a mutable reference to the archive, but the iterators holds an immutable one.
    let datafile_names = gradebook
        .file_names()
        .filter(|name| SUBMISSION_FILE_REGEX.is_match(name))
        .map(str::to_string)
        .collect::<Vec<_>>();

    // Same goes for reading the text files all the way through: the `Submission` struct that holds the names of files
    // and stuff needs to hold string slices, which have to point somewhere. They point into the Strings owned by this
    // vector.
    let datafile_contents = datafile_names
        .into_iter()
        .map(|filename| {
            // We can unwrap because we got this list of names from the archive itself, and we know no I/O problems can
            // happen since we already read the file from disk.
            let mut file = gradebook.by_name(&filename).unwrap();
            let mut contents = String::new();
            file.read_to_string(&mut contents)
                .expect("Blackboard's submission 'txt' should be valid UTF-8");
            (filename, contents)
        })
        .collect::<Vec<_>>();

    // Now we can parse the text files. Again, we collect. This time, so that we can count how many times each student
    // submitted.
    let mut attempt_counts = HashMap::new();
    let mut submissions = datafile_contents
        .iter()
        .map(|(filename, contents)| {
            let submission = Submission::new(&filename, contents);
            // Count up the number of times this student submitted
            let count = attempt_counts.entry(submission.username).or_insert(0u32);
            *count += 1;
            submission
        })
        .collect::<Vec<_>>();

    submissions.sort_by(|a, b| match a.username.cmp(b.username) {
        Ordering::Equal => a.datetime.cmp(&b.datetime),
        unequal => unequal,
    });


    // Onto processing!
    // --------------------------------------------------------------------------------------------

    println!("Processing submissions...");

    // Get the assignment name from the first one, they should always be the same. Use the parsed name instead of the
    // one from the gradebook filename, since that one is always garbled.
    let assn_name = submissions[0].assn_name;
    let assn_path = out_directory.unwrap_or(assn_name);

    // For the students with multiple attempts, keep track of which ones we've seen so far so we know what number to
    // give each attempt. They're sorted by date-time, so the 1st, 2nd, 3rd, etc submissions for each student should
    // always be in order.
    let mut attempts_processed = HashMap::new();
    let mut results = Vec::with_capacity(submissions.len());
    for submission in submissions {
        // Start by determining what the folder for this submission should be called
        let mut path = PathBuf::from(assn_path);
        if use_full_names {
            path.push(submission.fullname);
        } else {
            path.push(submission.username);
        }

        // If they made more than one attempt, add an 'Attempt N' folder to the path.
        let total_attempts = attempt_counts.get(submission.username).unwrap();
        let attempt_number = attempts_processed.entry(submission.username).or_insert(0u32);
        *attempt_number += 1;

        if *total_attempts > 1 {
            let digits = total_attempts.checked_ilog10().unwrap() as usize;
            path.push(format!("Attempt {:0>1$}", attempt_number, digits));
        }

        println!("Processing student {} attempt #{}:", submission.fullname, attempt_number);
        let sub_result = process_submission(&mut gradebook, submission, &path);

        results.push(sub_result);
    }

    Ok(results)
}


fn process_submission(
    gradebook: &mut ZipArchive<impl Read + Seek>,
    submission: Submission,
    path: &Path,
) -> Result<(), SubmissionError> {
    // Error handling

    let io_err = |e: io::Error| SubmissionError::from_io(&submission, e);
    let zip_err = |e: ZipError| SubmissionError::from_zip(&submission, e);

    // Make the output directory and create metadata files
    // --------------------------------------------------------------------------------------------

    fs::create_dir_all(&path).map_err(io_err)?;

    // Blackboard 'datafile' (the .txt)
    {
        let mut data_path = path.to_owned();
        data_path.push("[Blackboard] Info.txt");
        println!("\tWriting   \t[Blackboard] Info.txt",);

        let mut datafile_zip = gradebook.by_name(submission.datafile_name).unwrap();
        let mut datafile_out = File::create(&data_path).map_err(io_err)?;
        io::copy(&mut datafile_zip, &mut datafile_out).map_err(io_err)?;
    }

    // Any comments
    if let Some(contents) = submission.comments {
        let mut path = path.to_owned();
        path.push("[Blackboard] Comments.txt");
        println!("\tWriting   \t[Blackboard] Comments.txt");

        fs::write(path, contents).map_err(io_err)?;
    }

    // Any text submissions.
    if let Some(contents) = submission.text_submission {
        let mut path = path.to_owned();
        path.push("[Blackboard] Text Submission.txt");
        println!("\tWriting   \t[Blackboard] Text Submission.txt");

        fs::write(path, contents).map_err(io_err)?;
    }

    // Write all the actual files
    // --------------------------------------------------------------------------------------------

    for SubmissionFile { original_name, archive_name } in &submission.files {
        let mut file = gradebook.by_name(archive_name).map_err(zip_err)?;
        let mut buff = Vec::new();
        file.read_to_end(&mut buff).map_err(io_err)?;

        // If they submitted any ZIP files, unzip them here.
        let output_name = Path::new(original_name);
        if output_name.extension().is_some_and(|ext| ext == "zip") {
            // Output directory is named after the zip file
            let mut folder_name = OsString::from("[Unzipped] ");
            folder_name.push(output_name.file_stem().unwrap());

            let mut folder_path = path.to_owned();
            folder_path.push(folder_name);

            println!("\tExtracting\t{}", output_name.to_string_lossy());
            let cursor = Cursor::new(buff);
            ZipArchive::new(cursor)
                .map_err(zip_err)?
                .extract(folder_path)
                .map_err(zip_err)?;
        } else {
            let mut path = path.to_owned();
            path.push(output_name);
            println!("\tWriting   \t{}", output_name.to_string_lossy());
            fs::write(path, &mut buff).map_err(io_err)?;
        }
    }

    Ok(())
}


#[derive(Debug)]
struct Submission<'a> {
    pub datafile_name: &'a str,
    pub fullname: &'a str,
    pub username: &'a str,
    pub assn_name: &'a str,
    pub datetime: NaiveDateTime,
    pub text_submission: Option<&'a str>,
    pub comments: Option<&'a str>,
    pub files: Vec<SubmissionFile<'a>>,
}

impl<'a> Submission<'a> {
    pub fn new(datafile_name: &'a str, contents: &'a str) -> Self {
        let mut lines = contents.lines().peekable(); // Section reading depends on being peekable

        let mut fullname = None;
        let mut username = None;
        let mut assn_name = None;
        let mut datetime = None;
        let mut text_submission = None;
        let mut comments = None;
        let mut files = Vec::new();

        while let Some(line) = lines.next() {
            if line.starts_with("Name:") {
                let captures = STUDENT_NAME_REGEX
                    .captures(line)
                    .expect("Blackboard submission 'txt' should have a 'Name' line with \"First Last (firstlast)\"");
                fullname = Some(captures.name("fullname").unwrap().as_str());
                username = Some(captures.name("username").unwrap().as_str());
            } else if line.starts_with("Assignment:") {
                const START: usize = "Assignment:".len();
                assn_name = Some(line[START..].trim())
            } else if line.starts_with("Date Submitted:") {
                const START: usize = "Date Submitted:".len();
                let sub_date = line[START..].trim();
                let sub_date = NaiveDateTime::parse_from_str(sub_date, SUBMISSION_DATE_FORMAT).unwrap();
                datetime = Some(sub_date);
            } else if line.trim() == "Submission Field:" {
                let section_text = read_section_until(&mut lines, &["Comments:", "Files:"]).trim();
                if section_text != EMPTY_SUBMISSION_FIELD {
                    text_submission = Some(section_text);
                }
            } else if line.trim() == "Comments:" {
                let section_text = read_section_until(&mut lines, &["Submission Field:", "Files:"]).trim();
                if section_text != EMPTY_COMMENTS_FIELD {
                    comments = Some(section_text);
                }
            } else if line.trim() == "Files:" {
                let section_text = read_section_until(&mut lines, &["Submission Field:", "Comments:"]);
                let mut file_lines = section_text.lines();
                while let Some(file) = SubmissionFile::new(&mut file_lines) {
                    files.push(file);
                }
            }
        }

        Self {
            datafile_name,
            fullname: fullname.expect("Blackboard submission 'txt' should have a 'Name' line"),
            username: username.expect("Blackboard submission 'txt' should have a 'Name' line"),
            assn_name: assn_name.expect("Blackboard submission 'txt' should have an 'Assignment' line"),
            datetime: datetime.expect("Blackboard submission 'txt' should have a 'Date Submitted' line"),
            text_submission,
            comments,
            files,
        }
    }
}


#[derive(Debug)]
struct SubmissionFile<'a> {
    pub original_name: &'a str,
    pub archive_name: &'a str,
}

impl<'a> SubmissionFile<'a> {
    pub fn new(lines: &mut Lines<'a>) -> Option<Self> {
        let mut original_name = None;
        let mut archive_name = None;

        for line in lines {
            let trimmed_line = line.trim_start();

            if trimmed_line.starts_with("Original filename:") {
                const START: usize = "Original filename:".len();
                original_name = Some(trimmed_line[START..].trim());
            } else if trimmed_line.starts_with("Filename:") {
                const START: usize = "Filename:".len();
                archive_name = Some(trimmed_line[START..].trim());
            }
            // Stop after this set of files
            else if trimmed_line.len() == 0 {
                break;
            }
        }

        let original_name = original_name?;
        let archive_name = archive_name.expect("'Files' section had 'Original filename', but no 'Filename'");

        Some(Self { original_name, archive_name })
    }
}


fn read_section_until<'a>(lines: &mut Peekable<Lines<'a>>, stop_at: &[&str]) -> &'a str {
    let mut ptr_start = None;
    let mut ptr_end = None;

    let should_stop = |line: &str| stop_at.iter().any(|&stop| line.trim() == stop);

    while lines.peek().is_some_and(|line| !should_stop(line)) {
        let line = lines.next().unwrap();
        let ptrs = line.as_bytes().as_ptr_range();

        // Only set start pointer if this is the first line we've read.
        if ptr_start.is_none() {
            ptr_start = Some(ptrs.start);
        }

        // Always advance end pointer.
        ptr_end = Some(ptrs.end);
    }

    match (ptr_start, ptr_end) {
        // SAFETY: all involved pointers are derived from already-valid UTF-8 strings; all of those strings come from
        // the same `Lines` iterator, so they're all from the same `String`.
        (Some(start), Some(end)) => unsafe {
            let size = end.offset_from(start) as usize; // end > start, cast should never truncate
            let slice = std::slice::from_raw_parts(start, size);
            std::str::from_utf8_unchecked(slice)
        },
        (None, None) => "",
        _ => unreachable!("either both pointers should be set or neither should be set"),
    }
}
