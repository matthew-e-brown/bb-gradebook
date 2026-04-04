use std::error::Error;

use chrono::NaiveDateTime;
use smallvec::SmallVec;

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
    names: StudentNames<'a>,
    assignment: &'a str,
    date_submitted: NaiveDateTime,
    current_grade: Option<&'a str>,
    submission_field: Option<&'a str>,
    comments: Option<&'a str>,
    files: SmallVec<[FileNames<'a>; 8]>,
}

/// Struct for the captured text in the `Name: Fullname (username)` line in a datafile.
#[derive(Debug)]
pub struct StudentNames<'a> {
    fullname: &'a str,
    username: &'a str,
}

/// Struct for the captured text in the `Original Filename: ...` and `Filename: ...` lines of the `Files:` section in a
/// datafile.
#[derive(Debug)]
pub struct FileNames<'a> {
    original: &'a str,
    zipped: &'a str,
}

/// Splits a line of text at the first colon `:` character it sees and tidies up either side with some trimming.
///
/// Returns an error if no `:` character is present, or if there is no text before the colon.
fn split_field_name(line: &str) -> Result<(&str, &str), Box<dyn Error>> {
    let (name, body) = line.split_once(':').ok_or_else(|| format!("malformed field: expected ':'"))?;

    // Trim any spaces or tabs from the start of the field, but only remove at most a single space from the actual body
    // (so that a field that actually starts with whitespace will be correctly returned).
    let name = name.trim_start_matches(&[' ', '\t']);
    let body = body.strip_prefix(' ').unwrap_or(body);

    if name.is_empty() {
        Err(format!("malformed field: empty name before ':'").into())
    } else {
        Ok((name, body))
    }
}

/// Reads the body of a Blackboard `.txt` datafile and extracts the metadata within.
///
/// Parsed values are returned as plain string slices into the original buffer to avoid an unnecessary allocations.
pub fn parse_datafile<'a>(body: &'a str) -> Result<DatafileInfo<'a>, Box<dyn Error>> {
    let mut lines = LineStream::new(body);

    // Citing the robustness principle, we allow the first four fields to appear in any order. Ideally, we would allow
    // *all* the fields to appear in any order. Unfortunately, we need to enforce that the latter three sections appear
    // in a known order in order to accurately find the separation points between them.
    let mut name_field = None;
    let mut assignment_field = None;
    let mut date_submitted_field = None;
    let mut current_grade_field = None;

    // Instead of just looping 4x, count the number of fields we find; that way we can skip over blank lines (again,
    // trying to be "robust"). This loop will also break if any of the next three fields (Submission Field, Comments,
    // Files) are encountered.
    let mut count = 0;
    while count < 4 {
        let line = lines
            .peek_line()
            .ok_or_else(|| format!("line {}: unexpected EOF: expected one of 'Name:', 'Assignment:', 'Current Grade:', or 'Date Submitted:'", lines.line_num()))?;

        // Skip over any blank lines:
        if line.trim().is_empty() {
            lines.move_next().expect("peek already returned Some");
            continue;
        }

        let (field_name, contents) =
            split_field_name(line).map_err(|err| format!("line {}: {err}", lines.line_num()))?;

        let field = match field_name {
            "Name" => &mut name_field,
            "Assignment" => &mut assignment_field,
            "Date Submitted" => &mut date_submitted_field,
            "Current Grade" => &mut current_grade_field,
            // If we encounter any of the other fields early, break out of the loop and let the logic afterwards handle
            // any missing fields. whatever line gets left in the buffer. NB: Do this without advancing the stream: that
            // way, if they get lucky and one of the optional fields is missing, the line will remain in the buffer and
            // this function can proceed gracefully with trying to parse from there.
            "Submission Field" | "Comments" | "Files" => break,
            _ => return Err(format!("line {}: unknown field {field_name}", lines.line_num()).into()),
        };

        if field.is_some() {
            return Err(format!("line {}: duplicate field {field_name}", lines.line_num()).into());
        } else {
            *field = Some((contents, lines.line_num()));
            count += 1;
            lines.move_next();
        }
    }

    let names = {
        let (field, line_num) = name_field.ok_or_else(|| format!("missing 'Name' field"))?;
        parse_names(field).map_err(|err| format!("line {line_num}: failed to parse 'Name' field: {err}"))?
    };

    let (assignment, _) = assignment_field.ok_or_else(|| format!("missing 'Assignment' field"))?;

    let date_submitted = {
        let (field, line_num) = date_submitted_field.ok_or_else(|| format!("missing 'Date Submitted' field"))?;
        NaiveDateTime::parse_from_str(field, SUBMISSION_DATE_FORMAT)
            .map_err(|err| format!("line {line_num}: invalid datetime: {err}"))?
    };

    // We'll let 'Current Grade' be optional without throwing an error, again citing robustness principle.
    let current_grade = current_grade_field.map(|(field, _)| field);

    // Now we can move onto the three larger, block-level fields/sections.

    // Skip over blank lines first (in a well-formed datafile, there should only ever be a single blank).
    while let Some(line) = lines.peek_line()
        && line.trim().is_empty()
    {
        lines.move_next();
    }

    // The next three depend on their ordering to parse properly. The first one should be the submission field.
    let line = lines
        .move_next()
        .ok_or_else(|| format!("line {}: unexpected EOF before 'Submission Field' section", lines.line_num()))?;
    if line.trim_end() != "Submission Field:" {
        let line_num = lines.line_num() - 1; // Since we just stepped forwards
        return Err(format!("line {line_num}: expected 'Submission Field:', found '{line}'").into());
    }

    // The 'Submission Field:' field does not have free-form text: instead, it has WYSIWYG-formatted HTML content.
    // Importantly, that means that, under normal circumstances, it's impossible for a student to manage to get the text
    // "Comments:" at the start of a line (any attempt to do so will place it inside of a `<p>` tag). That means that,
    // no matter what content the student enters, we can reliably find the end of the 'Submission Field' field by
    // searching for "Comments:" as long as we only check specifically the start of lines.

    // We've already moved the stream past the end of the 'Submission Field:' line; the current start of the stream
    // should be the start of the field. It would be an error for there to be no more lines, since that'd mean we have
    // no 'Comments' or 'Files' section.
    let sf_line1 = lines
        .move_next()
        .ok_or_else(|| format!("line {}: unexpected EOF before 'Comments' section", lines.line_num()))?;

    // To be more robust, what we actually want to search for is a blank line immediately followed by a "Comments:"
    // line. So, as we scan forwards, we need to look for a sequence of three lines that looks like:
    // ```
    // ["<some text>", "", "Comments:"]
    //  ^ oldest           ^ most recent
    // ```
    // To do that, we keep track of the last 3 lines we've seen, and then check them all as one (they're stored in
    // reverse order so our if-lets read nicely, front-to-back).
    let mut last3: [Option<&'a str>; 3] = [None, None, Some(sf_line1)];
    let sf_last_line = loop {
        let next_line = lines
            .move_next()
            .ok_or_else(|| format!("line {}: unexpected EOF before 'Comments' section", lines.line_num()))?;

        last3[0] = last3[1];
        last3[1] = last3[2];
        last3[2] = Some(next_line);

        if let [_, Some(""), Some("Comments:")] = last3 {
            // `last3[0]` should always be `Some` in the case of a well-formed datafile. The only way that the two most
            // recent lines could be `["", "Comments:"]` *and* for the third-most recent to be `None` would be if the
            // line right after 'Submission Field:' was the blank line before comments. We *could* throw an error in
            // that case, but RE: robustness principle, we'll just treat it as the whole section being empty, same as if
            // the "there is no submission data" message was there..
            break last3[0].unwrap_or(sf_line1);
        }
    };

    // Now use some pointer math to find out where the first line starts and the last line ends:
    let sfi = subslice_offset_start(body, sf_line1).expect("sf_line1 is a substring of body");
    let sfj = subslice_offset_end(body, sf_last_line).expect("sf_last_line is a substring of body");
    let submission_field = &body[sfi..sfj];

    // Once we have reached this point, the stream is sitting just beyond the 'Comments:' line (that's the only way the
    // above loop could have broken, since the only other way out would be from the `?` checking for EOF).
    //
    // To parse the 'Comments:' section, we now want to grab lines greedily until the *last* occurrence of 'Files:',
    // since Blackboard's auto-generated 'Files' section will always come after anything the user might have typed. To
    // find the last one greedily, we have this last loop handle finding both the 'Comments' and 'Files' section, going
    // all the way to the end.

    let comments_line1 = lines
        .move_next()
        .ok_or_else(|| format!("line {}: unexpected EOF before 'Files' section", lines.line_num()))?;

    let mut comments_last_line = comments_line1;
    let mut files_line1 = None;

    // This time, since we want to simultaneously write down the last line of comments (two lines prior to the 'Files:'
    // heading) and the first line of the Files (one line after the 'Files:' heading), we keep track of a window of four
    // lines.
    let mut last4: [Option<&'a str>; 4] = [None, None, None, Some(comments_line1)];
    while let Some(next_line) = lines.move_next() {
        last4[0] = last4[1];
        last4[1] = last4[2];
        last4[2] = last4[3];
        last4[3] = Some(next_line);
        if let [_, Some(""), Some("Files:"), _] = last4 {
            // Take note of this as the potential last line of the comments, but keep scanning forwards till we hit the
            // end. Once again, the only way for `last4[0]` to be None at this point would be if `comments_line1` had
            // been the blank line between 'Comments:' and 'Files:'. Otherwise, at least 3 lines have been pushed to
            // `last4`, and all the original `None`s are gone.
            comments_last_line = last4[0].unwrap_or(comments_line1);

            // Also keep track of the line number the files section started on, since we'll want to pass that along to
            // the dedicated parser for the files section (so it can report error accurately). -1 because we just
            // stepped forwards, we want the index of the line we just grabbed.
            files_line1 = Some((next_line, lines.line_num() - 1));
        }
    }

    // We've now hit the end of the file, and we should be good to pull out the last two sections.
    let ci = subslice_offset_start(body, comments_line1).expect("comments_line1 is a substring of body");
    let cj = subslice_offset_end(body, comments_last_line).expect("comments_last_line is a substring of body");
    let comments = &body[ci..cj];

    let files = match files_line1 {
        // If `files_line1` is still None, that means we never saw a 'Files:' line with a blank line before it and
        // another line after it. That means that either the datafile ended exactly at "Files:", or there was no
        // "Files:" at all.
        None => {
            // A well-formed datafile will never *end* with "Files:", but we can be generous with our parsing and treat
            // it as an empty section. If not even that happened, that means we hit EOF before finding our files
            // section.
            if let [_, _, Some(""), Some("Files:")] = last4 {
                SmallVec::new()
            } else {
                return Err(format!("line {}: unexpected EOF before 'Files' section", lines.line_num()).into());
            }
        },
        Some((files_line1, start_line)) => {
            // The body of the 'Files' section goes all the way to the end of the file.
            let fi = subslice_offset_start(body, files_line1).expect("files_line1 is a substring of body");
            let files = parse_files(&body[fi..], start_line)
                .map_err(|err| format!("failed to parse 'Files' section: {err}"))?;
            files
        },
    };

    Ok(DatafileInfo {
        names,
        assignment,
        date_submitted,
        current_grade: current_grade.filter(|&line| line != EMPTY_GRADES_FIELD),
        submission_field: Some(submission_field).filter(|&text| text != EMPTY_SUBMISSION_FIELD),
        comments: Some(comments).filter(|&text| text != EMPTY_COMMENTS_FIELD),
        files,
    })
}

fn parse_names<'a>(field: &'a str) -> Result<StudentNames<'a>, Box<dyn Error>> {
    // I'm *pretty sure* that student usernames can't have brackets in them, so finding a `(username)` at the end of the
    // line shouldn't require any counting of L/R parentheses. I have *no idea* if the students' actual names can have
    // brackets in them, though, so we'll sidestep that possibility by searching for the brackets from the end.
    let i = field.rfind('(').ok_or_else(|| format!("expected '(' around username"))?;

    // Search for closing bracket only after the opening bracket:
    let j = field[i + 1..]
        .rfind(')')
        .ok_or_else(|| format!("expected ')' around username"))?;

    let fullname = field[..i].trim();
    let username = field[i + 1..i + j].trim();

    if fullname.is_empty() {
        return Err(format!("student's name is missing").into());
    } else if username.is_empty() {
        return Err(format!("student's username is missing").into());
    }

    Ok(StudentNames { fullname, username })
}

fn parse_files<'a>(body: &'a str, start_line: usize) -> Result<SmallVec<[FileNames<'a>; 8]>, Box<dyn Error>> {
    if body == EMPTY_FILES_FIELD {
        return Ok(SmallVec::new());
    }

    let mut lines = LineStream::new_at_line(body, start_line);
    let mut files = SmallVec::new();

    let mut original_name = None;
    let mut archive_name = None;

    while let Some(line) = lines.move_next() {
        if line.trim().is_empty() {
            continue;
        }

        // NB: `split_field_name` will trim the name.
        let (field_name, filename) = split_field_name(line)?;
        match (field_name, original_name, archive_name) {
            ("Original filename", None, _) => original_name = Some(filename),
            ("Filename", _, None) => archive_name = Some(filename),
            // If we encounter a second "Original filename" before first encountering a "Filename", or a second
            // "Filename" before first encountering an "Original filename", then we missed a field.
            ("Original filename", Some(_), None) => {
                let msg = format!("line {}: duplicate 'Original filename' before 'Filename'", lines.line_num());
                return Err(msg.into());
            },
            ("Filename", None, Some(_)) => {
                let msg = format!("line {}: duplicate 'Filename' before 'Original filename'", lines.line_num());
                return Err(msg.into());
            },
            (_other, _, _) => {
                let msg = format!("line {}: unknown field '{field_name}'", lines.line_num());
                return Err(msg.into());
            },
        }

        if let (Some(original), Some(zipped)) = (original_name, archive_name) {
            files.push(FileNames { original, zipped });
            original_name = None;
            archive_name = None;
        }
    }

    Ok(files)
}

/// An "iterator" that yields lines from a text buffer and gives access to the current line number.
///
/// This struct is more or less equivalent to a `std::iter::Peekable<std::iter::Enumerate<std::str::Lines<'a>>>`. The
/// only difference is that it provides more convenient access to the current line number. Using a standard `Enumerate`
/// iterator means we can only access the line number by stepping; we can wrap it in a peekable, but then the line
/// number is always behind `peek()` which returns an `Option`. In our case, we want to be able to grab the current line
/// number for error reporting, even if the iterator is already exhausted.
struct LineStream<'a> {
    /// The remaining portion of the buffer.
    buf: &'a str,
    /// The index of the next `\n` character in `buf` (if there are no more `\n` characters, holds the length of `buf`).
    next_eol: usize,
    /// The line number (starting from 1) of the line at the start of `buf`.
    line_num: usize,
}

impl<'a> LineStream<'a> {
    /// Creates a new [`LineStream`].
    pub fn new(buf: &'a str) -> Self {
        LineStream {
            buf,
            next_eol: buf.find('\n').unwrap_or(buf.len()),
            line_num: 1,
        }
    }

    /// Creates a new [`LineStream`] whose line number counter starts at the given value.
    ///
    /// This can be used to create a new `LineStream` out of a substring of a larger original buffer while still keeping
    /// the line numbers correct with respect to the original buffer.
    pub fn new_at_line(buf: &'a str, start_line: usize) -> Self {
        LineStream {
            buf,
            next_eol: buf.find('\n').unwrap_or(buf.len()),
            line_num: start_line,
        }
    }

    /// Gets a reference to the remaining contents of the buffer.
    pub fn buf_remaining(&self) -> &'a str {
        self.buf
    }

    /// Gets the current line number of this stream.
    ///
    /// This number is merely a counter and should only be used for reporting purposes; it may not accurately reflect
    /// the amount of lines in the original buffer that was used to create this `LineStream`.
    pub fn line_num(&self) -> usize {
        self.line_num
    }

    /// Returns the current line in the stream, excluding the line terminator.
    pub fn peek_line(&self) -> Option<&'a str> {
        if self.buf.is_empty() {
            None
        } else {
            // Since `next_eol` is the index of the `\n`, we don't need to chop it off here. But we do need to handle a
            // potential `\r` from CRLF.
            let line = &self.buf[..self.next_eol];
            let line = line.strip_suffix('\r').unwrap_or(line);
            Some(line)
        }
    }

    /// Returns the current line in the stream and advances the stream's buffer onto the next line.
    ///
    /// The returned line does not include the line terminator.
    pub fn move_next(&mut self) -> Option<&'a str> {
        if self.buf.is_empty() {
            None
        } else {
            // Here, instead of slicing *up to* `next_eol`, we're splitting *at* `next_eol`. That means the remainder
            // will include the `\n` (unless we split right at the end of the string).
            let (line, rest) = self.buf.split_at(self.next_eol);
            let line = line.strip_suffix('\r').unwrap_or(line);
            let rest = rest.strip_prefix('\n').unwrap_or(line);

            self.buf = rest;
            self.next_eol = rest.find('\n').unwrap_or(rest.len());
            self.line_num += 1;

            Some(line)
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
