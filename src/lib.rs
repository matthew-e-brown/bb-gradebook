use std::error::Error;
use std::fmt::Debug;
use std::io::{Read, Seek};
use std::iter::Peekable;
use std::str::Lines;
use std::sync::LazyLock;

use chrono::NaiveDateTime;
use regex::Regex;
use zip::ZipArchive;

/// The format specifier used to parse datetimes out of Blackboard's '.txt' datafiles' `Date submitted:` lines.
///
/// Quick reference:
/// - `%A`: full weekday name.
/// - `%B`: full month name.
/// - `%-d`: day number, with modifier to ignore zero-padding.
/// - `%Y`: full year number, zero-padded to four digits.
/// - `%-I`: hour number, 12 hours, with non-zero-padded modifier.
/// - `%M`: minute number, zero-padded.
/// - `%S`: second number, zero-padded.
/// - `%p`: uppercase AM/PM.
/// - `%Z`: local timezone name (e.g. EST).
///
/// Full reference: https://docs.rs/chrono/latest/chrono/format/strftime/index.html
const SUBMISSION_DATE_FORMAT: &str = "%A, %B %-d, %Y %-I:%M:%S %p %Z";

/// The text used to detect that a Blackboard '.txt' datafile contains no text submission, since Blackboard doesn't
/// actually leave the field empty.
const EMPTY_SUBMISSION_FIELD: &str = "There is no student submission text data for this assignment.";

/// The text used to detect that a Blackboard '.txt' datafile contains no comments, since Blackboard doesn't actually
/// leave the field empty.
const EMPTY_COMMENTS_FIELD: &str = "There are no student comments for this assignment.";

/// Regex used to detect the `.txt` files used by Blackboard to document each student's submission inside of the
/// gradebook.
///
/// This regex ensures that the '.txt' appears right after the `attempt_<TIMESTAMP>` portion of the filename, so
/// it shouldn't ever catch any student-submitted '.txt' files (unless they submitted one that happened to have
/// `_attempt_TIMESTAMP` right at the end, which is unlikely).
///
/// The date format at the end is `YYYY-MM-DD-hh-mm-ss`.
static DATAFILE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?<assn_name>.+?)_(?<username>[a-z0-9_]+)_attempt_(?<timestamp>\d{4}(?:-\d\d){5})\.txt$").unwrap()
});

/// Regex used to extract a student's full name and username from Blackboard's '.txt' datafiles.
static STUDENT_NAME_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Name:\s+(?<fullname>.+?)\s+\((?<username>[a-z0-9_]+)\)$").unwrap());


/// A collection of submissions from a Blackboard assignment.
///
/// A "gradebook" is not really a gradebook at all; rather, it is so named because of the names of the zip files given
/// by Blackboard upon download.
#[allow(unused)]
pub struct Gradebook<R: Read + Seek> {
    /// The underlying [ZipArchive] that this gradebook comes from.
    archive: ZipArchive<R>,
    /// Metadata on all submissions found within the gradebook.
    submissions: Vec<Submission>,
    /// The name of the assignment, as parsed from Blackboard's datafiles.
    assn_name: String,
}

/// Metadata for a single assignment submission.
#[derive(Debug, Clone)]
pub struct Submission {
    /// The full name of the student who submitted this assignment.
    student_fullname: String,
    /// The username of the student who submitted this assignment.
    student_username: String,
    /// When this assignment was submitted.
    datetime: NaiveDateTime,
    /// Any text that the student provided in the "Text Submission" field on Blackboard's interface.
    text_sub: Option<String>,
    /// Any comments provided by the student when submitting.
    comments: Option<String>,
    /// A list of metadata for all of the files submitted by this student.
    files: Vec<SubmissionFile>,
}

/// Metadata for a single file within a [Submission].
#[derive(Debug, Clone)]
pub struct SubmissionFile {
    /// The index within its original [ZipArchive] that this file lives at.
    zip_index: usize,
    /// The original name of the file, as uploaded by the student.
    original_name: String,
    /// The name of the file within Blackboard's gradebook file.
    archive_name: String,
    /// The size of this file within the zip archive.
    size_zipped: u64,
    /// The approximate size of this file post-unzip.
    size_unzipped: u64,
}

impl<R: Read + Seek> Gradebook<R> {
    /// Loads a Blackboard gradebook from a reader.
    pub fn load(reader: R) -> Result<Self, /*TODO*/ Box<dyn Error>> {
        println!("Loading gradebook...");

        let mut archive = ZipArchive::new(reader)?;

        // Start by collecting a list of all of the datafiles in the zip file. Doing this first lets us more efficiently
        // allocate vectors for later.
        println!("Finding datafiles...");
        let mut datafiles = Vec::new();
        for i in 0..archive.len() {
            let zipfile = archive.by_index(i)?; // TODO map err
            if DATAFILE_REGEX.is_match(zipfile.name()) {
                datafiles.push(i);
            }
        }

        if datafiles.len() == 0 {
            return /*TODO*/ Err("Empty zip file".into());
        }

        let mut assn_name = None;
        let mut df_buffer = String::new();
        let mut submissions = Vec::with_capacity(datafiles.len());

        println!("Parsing datafiles...");
        for i in datafiles {
            df_buffer.clear();
            archive.by_index(i)?.read_to_string(&mut df_buffer)?; // TODO map err

            let (submission, parsed_assn_name) = parse_datafile(&df_buffer, &mut archive)?;

            // Keep track of the assignment name we read: If this isn't the first one we've parsed, ensure that it's the
            // same as previous ones.
            if assn_name.is_none() {
                assn_name = Some(parsed_assn_name);
            } else if assn_name.as_deref().is_some_and(|curr| parsed_assn_name != curr) {
                todo!("Handle what to do when two datafiles parse to provide different assignment names");
            }

            submissions.push(submission);
        }

        let assn_name =
            assn_name.ok_or_else(|| /*TODO*/ "Failed to parse assignment name from Blackboard 'txt' files.")?;

        // Sort submissions by student username -> datetime:
        submissions.sort_by(|a, b| {
            a.student_username()
                .cmp(b.student_username())
                .then_with(|| a.datetime().cmp(&b.datetime()))
        });

        Ok(Gradebook { archive, assn_name, submissions })
    }

    /// The name of the assignment, as parsed from Blackboard's datafiles.
    pub fn assn_name(&self) -> &str {
        &self.assn_name
    }

    /// The number of submissions found in the zip file.
    pub fn num_submissions(&self) -> usize {
        self.submissions.len()
    }
}

impl<R: Read + Seek> Debug for Gradebook<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gradebook")
            .field("archive", &"ZipArchive(...)") // exclude entire archive from debug output
            .field("assn_name", &self.assn_name)
            .field("submissions", &self.submissions)
            .finish()
    }
}

impl Submission {
    /// Returns the full name of the student who submitted this assignment.
    pub fn student_fullname(&self) -> &str {
        &self.student_fullname
    }

    /// Returns the username of the student who submitted this assignment.
    pub fn student_username(&self) -> &str {
        &self.student_username
    }

    /// Returns any text provided by the student in the _Text Submission_ field on Blackboard, if any.
    pub fn text_sub(&self) -> Option<&str> {
        self.text_sub.as_deref()
    }

    /// Returns any text provided by the student in the _Comments_ field on Blackboard, if any.
    pub fn comments(&self) -> Option<&str> {
        self.comments.as_deref()
    }

    /// Returns the number of files in this submission.
    pub fn num_files(&self) -> usize {
        self.files.len()
    }

    /// Returns an iterator of metadata for all of the files in this submission.
    pub fn files(&self) -> impl Iterator<Item = &SubmissionFile> {
        (&self.files).into_iter()
    }

    /// Returns the local time this assignment was submitted.
    ///
    /// Blackboard does not provide any timezone information in its zip files, so [NaiveDateTime] is as accurate as we
    /// can get.
    pub fn datetime(&self) -> NaiveDateTime {
        self.datetime
    }

    /// Computes the total size of this submission's files within the zip archive.
    pub fn size_zipped(&self) -> u64 {
        self.files().fold(0, |acc, f| acc + f.size_zipped())
    }

    /// Computes the approximate total size of this submission's files after unzipping.
    ///
    /// FIXME: Does not account for nested archives at the moment.
    pub fn size_unzipped(&self) -> u64 {
        self.files().fold(0, |acc, f| acc + f.size_unzipped())
    }
}

impl SubmissionFile {
    /// Returns the index at which this file lives within its corresponding [ZipArchive].
    pub fn zip_index(&self) -> usize {
        self.zip_index
    }

    /// Returns the original name of the file, as uploaded by the student.
    pub fn original_name(&self) -> &str {
        &self.original_name
    }

    /// Returns the name of the file within Blackboard's gradebook file.
    pub fn archive_name(&self) -> &str {
        &self.archive_name
    }

    /// Returns the size of this file within the zip archive.
    pub fn size_zipped(&self) -> u64 {
        self.size_zipped
    }

    /// Returns the approximate size that this file will be after unzipping.
    pub fn size_unzipped(&self) -> u64 {
        self.size_unzipped
    }
}

/// Macro that sets the given option to `Some(T)` if it is not already set, and returns an error otherwise.
macro_rules! set_if_none {
    ($txt:literal, $opt:ident, $val:expr) => {
        if $opt.is_none() {
            $opt = Some($val);
        } else {
            const MSG: &str = concat!("Duplicate field ", $txt, " in Blackboard 'txt' file!");
            return /* TODO */ Err(MSG.into());
        }
    };
}

/// Parses a Blackboard `.txt` datafile, returning [submission-specific metadata][Submission] as well as the assignment
/// name.
fn parse_datafile<R: Read + Seek>(
    df_contents: &str,
    archive: &mut ZipArchive<R>,
) -> Result<(Submission, String), /*TODO*/ Box<dyn Error>> {
    // Parse the bits we need one-by-one:
    let mut student_fullname = None;
    let mut student_username = None;
    let mut datetime = None;
    let mut text_sub = None;
    let mut comments = None;
    let mut files = None;

    // Even though we don't store the assignment name, we want to return it to the caller just in case.
    let mut assn_name = None;

    let mut lines = df_contents.lines().peekable();
    while let Some(line) = lines.next() {
        // Find the contents of the line up to the first colon, to read for section headers; if there isn't one, skip
        // forwards.
        let Some(c) = line.find(":") else { continue };
        match &line[..c] {
            "Name" => {
                let captures = STUDENT_NAME_REGEX
                    .captures(line)
                    .ok_or_else(|| /*TODO*/ "Malformed 'txt' datafile")?;
                // Can unwrap capture groups because the regex is strict enough that either the groups are there or the
                // entire thing would have failed:
                let fullname = captures.name("fullname").unwrap().as_str().to_owned();
                let username = captures.name("username").unwrap().as_str().to_owned();
                set_if_none!("Name", student_fullname, fullname);
                set_if_none!("Name", student_username, username);
            },
            "Assignment" => {
                let assn_substr = line["Assignment:".len()..].trim();
                set_if_none!("Assignment", assn_name, assn_substr.to_owned());
            },
            "Date Submitted" => {
                let date_substr = line["Date Submitted:".len()..].trim();
                let date_parsed = NaiveDateTime::parse_from_str(date_substr, SUBMISSION_DATE_FORMAT)
                    .map_err(|_| /*TODO*/ "Malformed date in 'txt' datafile")?;
                set_if_none!("Date Submitted", datetime, date_parsed);
            },
            "Submission Field" => {
                let text = read_section_until(lines.by_ref(), &["Comments:", "Files:"]).trim();
                let value = (text != EMPTY_SUBMISSION_FIELD).then(|| text.to_owned());
                set_if_none!("Submission Field", text_sub, value);
            },
            "Comments" => {
                let text = read_section_until(lines.by_ref(), &["Submission Field:", "Files:"]).trim();
                let value = (text != EMPTY_COMMENTS_FIELD).then(|| text.to_owned());
                set_if_none!("Comments", comments, value);
            },
            "Files" => {
                let text = read_section_until(lines.by_ref(), &["Submission Field:", "Comments:"]);
                let mut section_lines = text.lines();

                let mut parsed_files = Vec::new();
                while let Some(file) = parse_files_section(section_lines.by_ref(), archive)? {
                    parsed_files.push(file);
                }

                set_if_none!("Files", files, parsed_files);
            },
            _ => {},
        }
    }

    if comments.is_none() {
        /* TODO: warn that section was missing, but don't drop the whole submission because of it */
    }

    let assn_name = assn_name.ok_or("Missing 'Assignment:' in datafile")?;
    let submission = Submission {
        // TODO errors
        student_fullname: student_fullname.ok_or("Missing 'Name:' in datafile")?,
        student_username: student_username.ok_or("Missing 'Name:' in datafile")?,
        datetime: datetime.ok_or("Missing 'Date Submitted' in datafile")?,
        text_sub: text_sub.ok_or("Missing 'Submission Field' in datafile")?,
        comments: comments.flatten(),
        files: files.ok_or("Missing 'Files' section in datafile")?,
    };

    Ok((submission, assn_name))
}

/// Borrows the iterator created by [`parse_datafile`] to consume all lines related to files.
///
/// Returns `Ok(None)` when the lines iterator is exhausted.
fn parse_files_section<'a, R: Read + Seek>(
    lines: &mut Lines<'a>,
    archive: &mut ZipArchive<R>,
) -> Result<Option<SubmissionFile>, /*TODO*/ Box<dyn Error>> {
    let mut original_name = None;
    let mut archive_name = None;

    for line in lines {
        let line = line.trim_start();
        if line.starts_with("Original filename:") {
            let trimmed = line["Original filename:".len()..].trim().to_owned();
            set_if_none!("Original filename:", original_name, trimmed);
        } else if line.starts_with("Filename:") {
            let trimmed = line["Filename:".len()..].trim().to_owned();
            set_if_none!("Filename", archive_name, trimmed);
        } else if line.len() == 0 {
            break;
        }
    }

    let original_name = match original_name {
        Some(name) => name,
        None => return Ok(None),
    };

    let archive_name = match archive_name {
        Some(name) => name,
        None => return Err("'Files' section had 'Original filename', but no 'Filename'".into()),
    };

    let zip_index = archive
        .index_for_name(&archive_name)
        .ok_or_else(|| "Filename specified in Blackboard 'txt' not found within zipfile.")?;

    let zipfile = archive
        .by_name(&archive_name)
        .expect("file referenced by datafile should be present in zip");
    let size_zipped = zipfile.compressed_size();
    let size_unzipped = zipfile.size();

    Ok(Some(SubmissionFile {
        archive_name,
        original_name,
        size_zipped,
        size_unzipped,
        zip_index,
    }))
}

/// Reads a section of a Blackboard `.txt` datafile up until one of a specified set of lines is reached.
fn read_section_until<'a>(lines: &mut Peekable<Lines<'a>>, stop_at: &[&str]) -> &'a str {
    let mut ptr_start = None;
    let mut ptr_end = None;

    // NB: use trim_end() to require that the .txt file actually have a line that *starts* the same as one of the
    // provided delimiter lines, in an attempt to avoid a potential problem when (if) a student uses the text
    // "Comments:" or "Text Submission:", etc., in their assignment.
    let should_stop = |line: &str| stop_at.iter().any(|&stop| line.trim_end() == stop);

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
