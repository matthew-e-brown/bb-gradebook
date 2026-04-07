use std::ops::ControlFlow;

use chrono::NaiveDateTime;
use smallvec::SmallVec;

use crate::error::datafile::{self as error, DatafileError, FieldParseError, FilesError, NameError};

/// The format specifier used to parse datetimes out of datafiles' `Date Submitted:` lines.
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
/// Example string: "Thursday, December 12, 2024 1:22:26 PM EST"
///
/// Full reference: https://docs.rs/chrono/latest/chrono/format/strftime/index.html
const SUBMISSION_DATE_FORMAT: &str = "%A, %B %-d, %Y %-I:%M:%S %p %Z";

/// The string used to detect if the "Current Grade:" section does not currently have a grade.
const EMPTY_GRADES_FIELD: &str = "Needs Grading";

/// The string used to detect if a datafile contains no text submission, since Blackboard doesn't actually leave the
/// field empty.
const EMPTY_SUBMISSION_FIELD: &str = "There is no student submission text data for this assignment.";

/// The string used to detect if a datafile contains no comments, since Blackboard doesn't actually leave the field
/// empty.
const EMPTY_COMMENTS_FIELD: &str = "There are no student comments for this assignment.";

/// The string used to detect if a datafile contains no files, since Blackboard doesn't actually leave the field empty.
const EMPTY_FILES_FIELD: &str = "No files were attached to this submission.";

/// Struct containing all the captured pieces of text from a Blackboard datafile.
#[derive(Debug)]
pub struct DatafileInfo<'a> {
    pub names: StudentNames<'a>,
    pub assignment: &'a str,
    pub date_submitted: NaiveDateTime,
    pub current_grade: Option<&'a str>,
    pub submission_field: Option<&'a str>,
    pub comments: Option<&'a str>,
    pub files: SmallVec<[FileNames<'a>; 8]>,
}

/// Struct for the captured text in the `Name: Fullname (username)` line in a datafile.
#[derive(Debug, Clone, Copy)]
pub struct StudentNames<'a> {
    pub fullname: &'a str,
    pub username: &'a str,
}

/// Struct for the captured text in the `Original Filename: ...` and `Filename: ...` lines of the `Files:` section in a
/// datafile.
#[derive(Debug, Clone, Copy)]
pub struct FileNames<'a> {
    pub original: &'a str,
    pub archive: &'a str,
}

pub fn parse_datafile<'a>(body: &'a str) -> Result<DatafileInfo<'a>, DatafileError> {
    DatafileParser::new(body).parse()
}

/// The name of a field parsed out of a datafile.
///
/// Since short (single-line) and long (multi-line) fields are parsed differently, it's convenient to be able to
/// distinguish between the two. Having distinct enums makes it easier to work with match statements. For
/// error-reporting purposes, though, we don't need that much granularity; hence the existence of the separate (and
/// public) [`error::Field`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Short(ShortField),
    Long(LongField),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShortField {
    Name,
    Assignment,
    DateSubmitted,
    CurrentGrade,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LongField {
    SubmissionField,
    Comments,
    Files,
}

impl From<ShortField> for Field {
    fn from(value: ShortField) -> Self {
        Field::Short(value)
    }
}

impl From<LongField> for Field {
    fn from(value: LongField) -> Self {
        Field::Long(value)
    }
}

impl Field {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "Name" => Some(Field::Short(ShortField::Name)),
            "Assignment" => Some(Field::Short(ShortField::Assignment)),
            "Date Submitted" => Some(Field::Short(ShortField::DateSubmitted)),
            "Current Grade" => Some(Field::Short(ShortField::CurrentGrade)),
            "Submission Field" => Some(Field::Long(LongField::SubmissionField)),
            "Comments" => Some(Field::Long(LongField::Comments)),
            "Files" => Some(Field::Long(LongField::Files)),
            _ => None,
        }
    }
}

impl From<Field> for error::Field {
    fn from(value: Field) -> Self {
        match value {
            Field::Short(ShortField::Name) => Self::Name,
            Field::Short(ShortField::Assignment) => Self::Assignment,
            Field::Short(ShortField::DateSubmitted) => Self::DateSubmitted,
            Field::Short(ShortField::CurrentGrade) => Self::CurrentGrade,
            Field::Long(LongField::SubmissionField) => Self::SubmissionField,
            Field::Long(LongField::Comments) => Self::Comments,
            Field::Long(LongField::Files) => Self::Files,
        }
    }
}

impl From<ShortField> for error::Field {
    fn from(value: ShortField) -> Self {
        Field::from(value).into()
    }
}

impl From<LongField> for error::Field {
    fn from(value: LongField) -> Self {
        Field::from(value).into()
    }
}

/// The main parser struct responsible for pulling the pieces out of a datafile.
///
/// This struct keeps track of the currently-encountered pieces of the datafile as the main loop in the
/// [`parse`][Self::parse] method does its loops through the lines of the file.
struct DatafileParser<'a> {
    names: Option<StudentNames<'a>>,
    assignment: Option<&'a str>,
    date_submitted: Option<NaiveDateTime>,
    current_grade: Option<&'a str>,
    submission_field: Option<&'a str>,
    comments: Option<&'a str>,
    files: Option<SmallVec<[FileNames<'a>; 8]>>,
    source: &'a str,
    stream: LineStream<'a>,
}

impl<'a> DatafileParser<'a> {
    pub fn new(body: &'a str) -> Self {
        Self {
            names: None,
            assignment: None,
            date_submitted: None,
            current_grade: None,
            submission_field: None,
            comments: None,
            files: None,
            source: body,
            stream: LineStream::new(body),
        }
    }

    fn err_duplicate_field<F: Into<error::Field>>(&self, field: F) -> DatafileError {
        DatafileError::DuplicateField {
            field: field.into(),
            line_num: self.stream.line_num(),
        }
    }

    fn err_field_parse<F: Into<error::Field>, E: Into<FieldParseError>>(&self, field: F, inner: E) -> DatafileError {
        DatafileError::FieldParse {
            field: field.into(),
            inner: inner.into(),
            line_num: self.stream.line_num(),
        }
    }

    fn err_unknown_field(&self, field_name: &str) -> DatafileError {
        DatafileError::UnknownField {
            field: field_name.into(),
            line_num: self.stream.line_num(),
        }
    }

    fn err_unexpected(&self, text: &str) -> DatafileError {
        DatafileError::Unexpected {
            text: text.into(),
            line_num: self.stream.line_num(),
        }
    }

    fn err_missing<F: Into<error::Field>>(&self, field: F) -> DatafileError {
        DatafileError::MissingField { field: field.into() }
    }

    fn has_value<F: Into<Field>>(&self, field: F) -> bool {
        match field.into() {
            Field::Short(ShortField::Name) => self.names.is_some(),
            Field::Short(ShortField::Assignment) => self.assignment.is_some(),
            Field::Short(ShortField::DateSubmitted) => self.date_submitted.is_some(),
            Field::Short(ShortField::CurrentGrade) => self.current_grade.is_some(),
            Field::Long(LongField::SubmissionField) => self.submission_field.is_some(),
            Field::Long(LongField::Comments) => self.comments.is_some(),
            Field::Long(LongField::Files) => self.files.is_some(),
        }
    }

    pub fn parse(mut self) -> Result<DatafileInfo<'a>, DatafileError> {
        while !self.stream.is_empty() {
            let line = self.stream.current();

            if line.trim().is_empty() {
                self.stream.next();
                continue;
            }

            match split_field_name(line) {
                Some((field_name, field_body)) => match Field::from_str(field_name) {
                    // Check for duplicate fields first:
                    Some(field) if self.has_value(field) => return Err(self.err_duplicate_field(field)),
                    // Now we can handle each field individually.
                    Some(Field::Short(field)) => self.parse_short_field(field, field_body)?,
                    Some(Field::Long(field)) => {
                        // Each of the long fields start on the next line, which means the remainder of this line should
                        // be empty.
                        if !field_body.trim().is_empty() {
                            return Err(self.err_unexpected(field_body));
                        }

                        // Additionally, there should be at least one more line afterwards. If not, we can break out of
                        // the main loop now. Since each of these long sections are optional, our option-unwrapping down
                        // below should handle this gracefully.
                        if !self.stream.next() {
                            break;
                        }

                        self.parse_long_field(field)?;
                    },
                    None => return Err(self.err_unknown_field(field_name)),
                },
                None => return Err(self.err_unexpected(line)),
            }
        }

        // Required fields:
        let names = self.names.ok_or(self.err_missing(error::Field::Name))?;
        let assignment = self.assignment.ok_or(self.err_missing(error::Field::Assignment))?;
        let date_submitted = self.date_submitted.ok_or(self.err_missing(error::Field::DateSubmitted))?;

        // Optional/recoverable fields:
        let current_grade = self.current_grade.filter(|&txt| txt != EMPTY_GRADES_FIELD);
        let submission_field = self.submission_field.filter(|&txt| txt != EMPTY_SUBMISSION_FIELD);
        let comments = self.comments.filter(|&txt| txt != EMPTY_COMMENTS_FIELD);
        let files = self.files.unwrap_or(SmallVec::new());

        Ok(DatafileInfo {
            names,
            assignment,
            date_submitted,
            current_grade,
            submission_field,
            comments,
            files,
        })
    }

    /// Parses a single-line field.
    fn parse_short_field(&mut self, field: ShortField, field_body: &'a str) -> Result<(), DatafileError> {
        match field {
            ShortField::Name => {
                self.names = match parse_names(field_body) {
                    Ok(value) => Some(value),
                    Err(inner) => return Err(self.err_field_parse(ShortField::Name, inner)),
                };
            },
            ShortField::Assignment => {
                self.assignment = Some(field_body);
            },
            ShortField::DateSubmitted => {
                self.date_submitted = match NaiveDateTime::parse_from_str(field_body, SUBMISSION_DATE_FORMAT) {
                    Ok(value) => Some(value),
                    Err(inner) => return Err(self.err_field_parse(ShortField::DateSubmitted, inner)),
                };
            },
            ShortField::CurrentGrade => {
                self.current_grade = Some(field_body);
            },
        }

        // Leave the stream in the right place for the next main loop iteration
        self.stream.next();
        Ok(())
    }

    /// Parses a multi-line field out of the parser's stream starting at its current position.
    fn parse_long_field(&mut self, field: LongField) -> Result<(), DatafileError> {
        match field {
            LongField::SubmissionField => {
                // To parse the 'Submission Field' section, we read lines until we see any of the other fields. We know
                // that the field can only contain WYSIWYG HTML; that means that as soon as we see another 'XXX:' field,
                // we know the 'Submission Field' is done.

                let start_line = self.stream.current();
                let mut end_line = start_line;

                while self.stream.next() {
                    let curr = self.stream.current();
                    // Does this line have a field name on it? If so, break out. Otherwise, take note of this line as
                    // being the last one of the section (as long as it's not a blank line, which will automatically
                    // trim off trailing blanks).
                    if split_field_name(curr).and_then(|(name, _)| Field::from_str(name)).is_some() {
                        break;
                    } else if !curr.trim().is_empty() {
                        end_line = curr;
                    }
                }

                // If we broke out of this loop, we either hit the name of another section, or we ran out of lines. We
                // *don't* want to advance the stream after this point, since this will leave the stream's position
                // sitting at the field to be re-handled by the main loop.
                let i = subslice_offset_start(self.source, start_line).expect("start_line is derived from body");
                let j = subslice_offset_end(self.source, end_line).expect("end_line is derived from body");
                self.submission_field = Some(&self.source[i..j]);
            },
            LongField::Comments => {
                // To parse the 'Comments' section, we want to greedily capture lines until we see another section.
                // Keyword *greedily*: the (unfortunate) trick here is that 'Comments' contains *any* freeform text from
                // the user. That means the user could theoretically put 'Name:' or 'Files:' at the start of the line
                // inside their 'Comments'.
                //
                // To address this: when we do encounter a line that starts with "XXX:", then:
                // - If we've already seen that `XXX` so far in the file before the 'Comments' section, then any "XXX:"
                //   we see inside the comments must be a part of the comments' body.
                // - If we haven't, then this instance of `XXX:` is part of the comments' body as long as it is not the
                //   *last* instance of `XXX:` in the entire file.

                let start_line = self.stream.current();
                let mut end_line = start_line;

                while self.stream.next() {
                    let curr = self.stream.current();
                    if let Some(field) = split_field_name(curr).and_then(|(name, _)| Field::from_str(name)) {
                        // Has this field already been successfully parsed?
                        if self.has_value(field) {
                            end_line = curr;
                        } else {
                            // If not, then this line is included in the 'Comments:' body only if it is *not* the last
                            // time this field appears in the file; otherwise, this current line is the start of another
                            // section, and we want to stop. To test that, we'll clone the stream at its current
                            // position and loop through the remainder of the lines up till the end.
                            let mut test_stream = self.stream.clone();
                            let mut found_again = false;
                            while test_stream.next() {
                                if split_field_name(test_stream.current())
                                    .and_then(|(name, _)| Field::from_str(name))
                                    .is_some_and(|test_field| test_field == field)
                                {
                                    found_again = true;
                                    break;
                                }
                            }

                            if found_again {
                                end_line = curr;
                            } else {
                                break;
                            }
                        }
                    } else if !curr.trim().is_empty() {
                        end_line = curr;
                    }
                }

                let i = subslice_offset_start(self.source, start_line).expect("start_line is derived from body");
                let j = subslice_offset_end(self.source, end_line).expect("end_line is derived from body");
                self.comments = Some(&self.source[i..j]);
            },
            LongField::Files => {
                // To parse the 'Files' section, we want to keep grabbing lines as long as those lines look like
                // 'Original filename: ...' or 'Filename: ...'. When we see a line that isn't one of those, we
                // break and continue with the other sections.
                match parse_files(&mut self.stream) {
                    Ok(value) => self.files = Some(value),
                    Err(inner) => return Err(self.err_field_parse(LongField::Files, inner)),
                }
            },
        }

        Ok(())
    }
}

/// Splits a line of text at the first colon `:` character it sees and tidies up either side with some trimming.
///
/// Returns an error if no `:` character is present, or if there is no text before the colon.
fn split_field_name(line: &str) -> Option<(&str, &str)> {
    let (name, body) = line.split_once(':')?;
    // Remove at most a single space from the actual body (so that a field that actually starts with whitespace will be
    // correctly returned).
    let body = body.strip_prefix(' ').unwrap_or(body);
    if name.trim().is_empty() { None } else { Some((name, body)) }
}

fn parse_names<'a>(field: &'a str) -> Result<StudentNames<'a>, NameError> {
    // I'm *pretty sure* that student usernames can't have brackets in them, so finding a `(username)` at the end of the
    // line shouldn't require any counting of L/R parentheses. I have *no idea* if the students' actual names can have
    // brackets in them, though, so we'll sidestep that possibility by searching for the brackets from the end.
    let i = field.rfind('(').ok_or(NameError::MissingL)?;

    // Search for closing bracket only after the opening bracket:
    let j = field[i + 1..].rfind(')').ok_or(NameError::MissingR)?;

    let fullname = field[..i].trim();
    let username = field[i + 1..i + 1 + j].trim();

    if fullname.is_empty() {
        return Err(NameError::EmptyFullname);
    } else if username.is_empty() {
        return Err(NameError::EmptyUsername);
    }

    Ok(StudentNames { fullname, username })
}

fn parse_files<'a>(stream: &mut LineStream<'a>) -> Result<SmallVec<[FileNames<'a>; 8]>, FilesError> {
    // Since the 'Files' section has further parsing to do, we need to check for the special "No files" text here
    // instead of at the end.
    if stream.current() == EMPTY_FILES_FIELD {
        stream.next();
        return Ok(SmallVec::new());
    }

    // Once again, it's convenient to have access to the data we're building up through the `self`.
    struct FilesParser<'a> {
        files: SmallVec<[FileNames<'a>; 8]>,
        original_name: Option<&'a str>,
        archive_name: Option<&'a str>,
    }

    impl<'a> FilesParser<'a> {
        fn new() -> Self {
            FilesParser {
                files: SmallVec::new(),
                original_name: None,
                archive_name: None,
            }
        }

        fn handle_line(&mut self, line: &'a str) -> Result<ControlFlow<()>, FilesError> {
            if line.trim().is_empty() {
                return Ok(ControlFlow::Continue(()));
            }

            let Some((field, filename)) = split_field_name(line) else {
                return Ok(ControlFlow::Continue(()));
            };

            match (field, self.original_name, self.archive_name) {
                ("Original filename", None, _) => self.original_name = Some(filename),
                ("Filename", _, None) => self.archive_name = Some(filename),
                // If we encounter a second "Original filename" before first encountering a "Filename", or a second
                // "Filename" before first encountering an "Original filename", then we missed a field.
                ("Original filename", Some(_), None) => return Err(FilesError::MissingArchive),
                ("Filename", None, Some(_)) => return Err(FilesError::MissingOriginal),
                // Any field we find that is not "Original filename" or "Filename" is totally okay, it just means we
                // want to break out of the loop.
                (_, _, _) => return Ok(ControlFlow::Break(())),
            }

            if let (Some(original), Some(archive)) = (self.original_name, self.archive_name) {
                self.files.push(FileNames { original, archive });
                self.original_name = None;
                self.archive_name = None;
            }

            Ok(ControlFlow::Continue(()))
        }

        fn finish(mut self) -> Result<SmallVec<[FileNames<'a>; 8]>, FilesError> {
            match (self.original_name, self.archive_name) {
                (Some(original), Some(archive)) => {
                    self.files.push(FileNames { original, archive });
                    self.original_name = None;
                    self.archive_name = None;
                },
                (Some(_), None) => return Err(FilesError::MissingArchive),
                (None, Some(_)) => return Err(FilesError::MissingOriginal),
                (None, None) => {},
            }
            Ok(self.files)
        }
    }

    let mut parser = FilesParser::new();

    while !stream.is_empty() {
        // Lines in the 'Files' section start with tabs.
        let line = stream.current().trim_start_matches(&[' ', '\t']);
        match parser.handle_line(line)? {
            ControlFlow::Continue(_) => {
                stream.next();
            },
            ControlFlow::Break(_) => {
                break;
            },
        }
    }

    parser.finish()
}

/// An almost-iterator that steps through lines from a text buffer and gives access to the current line number.
///
/// This struct is functionally similar to a [`std::str::Lines`] wrapped in an [`Enumerate`] and [`Peekable`], but it
/// provides more purpose-built and explicit control for when exactly the stream is advanced. Using a standard
/// `Enumerate<Lines>` would mean that the current line number (which we want for error-reporting inside
/// [`DatafileParser`]) is only accessible when stepping forwards. Wrapping it in a `Peekable` helps, but then the line
/// number is always behind `peek()`, which returns an `Option`. For the most control when parsing, it is convenient to
/// have precise control of when the lines are advanced.
///
/// [`Enumerate`]: std::iter::Enumerate
/// [`Peekable`]: std::iter::Peekable
#[derive(Clone)]
struct LineStream<'a> {
    /// The remaining portion of the buffer.
    buf: &'a str,
    /// The index of the next `\n` character in `buf` (if there are no more `\n` characters, holds the length of `buf`).
    next_eol: usize,
    /// The line number (starting from 1) of the line at the start of `buf`.
    line_num: usize,
}

impl<'a> LineStream<'a> {
    /// Creates a new [`LineStream`] with a line number starting at 1.
    pub fn new(buf: &'a str) -> Self {
        LineStream {
            buf,
            next_eol: buf.find('\n').unwrap_or(buf.len()),
            line_num: 1,
        }
    }

    /// Returns `true` if there are no more lines in this stream.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Gets the current line number of this stream.
    ///
    /// If the stream has been exhausted, this number will point to one past the last line number.
    pub fn line_num(&self) -> usize {
        self.line_num
    }

    /// Returns the current line in the stream, excluding the line terminator.
    ///
    /// If the stream is currently empty, this will return an empty string. Note however that an empty string will also
    /// be returned if the stream contains an empty line; check [`Self::is_empty`] or the return value of [`Self::next`]
    /// when stepping to determine if the stream is truly empty or not.
    pub fn current(&self) -> &'a str {
        // `next_eol` is the index of the `\n` itself, so don't need to chop it off here. We do need to handle a
        // potential `\r` from CRLF, though.
        let line = &self.buf[..self.next_eol];
        let line = line.strip_suffix('\r').unwrap_or(line);
        line
    }

    /// Advances the stream to the next line and increments the line counter.
    ///
    /// Returns `true` if there is still at least one line left in the buffer after advancing.
    ///
    /// Does nothing if the stream is already empty.
    pub fn next(&mut self) -> bool {
        if self.is_empty() {
            false
        } else {
            self.line_num += 1;
            if self.next_eol < self.buf.len() {
                self.buf = &self.buf[self.next_eol + 1..]; // Skip past `\n`
                self.next_eol = self.buf.find('\n').unwrap_or(self.buf.len());
                true // At least one more line
            } else {
                self.buf = &self.buf[self.next_eol..]; // Advance all the way up to the end
                self.next_eol = self.buf.len();
                false // No more lines
            }
        }
    }
}

/// Returns the number of bytes between the start of `outer` and the start of `inner`.
///
/// `inner` must start before or exactly at the end of `outer`. Otherwise, `None` is returned.
fn subslice_offset_start(outer: &str, inner: &str) -> Option<usize> {
    let outer_range = outer.as_bytes().as_ptr_range();
    let inner_start = inner.as_bytes().as_ptr();
    if outer_range.contains(&inner_start) || inner_start == outer_range.end {
        // SAFETY: Already checked that this pointer is inside the range; range comes from a known-valid `&str` slice,
        // so there won't be any weird address-space wrapping happening.
        Some(unsafe { inner_start.byte_offset_from_unsigned(outer_range.start) })
    } else {
        None
    }
}

/// Returns the number of bytes between the start of `outer` and the **end** of `inner`.
///
/// `inner` must end before or exactly at the end of `outer`. Otherwise, `None` is returned.
fn subslice_offset_end(outer: &str, inner: &str) -> Option<usize> {
    let outer_range = outer.as_bytes().as_ptr_range();
    let inner_end = inner.as_bytes().as_ptr_range().end;
    if outer_range.contains(&inner_end) || inner_end == outer_range.end {
        // SAFETY: Already checked that this pointer is inside the range; range comes from a known-valid `&str` slice,
        // so there won't be any weird address-space wrapping happening.
        Some(unsafe { inner_end.byte_offset_from_unsigned(outer_range.start) })
    } else {
        None
    }
}
