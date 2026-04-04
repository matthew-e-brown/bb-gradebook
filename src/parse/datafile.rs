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


// =====================================================================================================================
// ================================================= PREVIOUS ATTEMPTS =================================================
// =====================================================================================================================
//
// Below are some of the previous attempts that I made to write the parsing logic for the datafiles. No one of them ever
// felt complete or even like they were going in the right direction, and so I never ended up committing them. Just so
// that I don't lose them, I wanted to include them in at least one commit before deleting them fully. I may want to
// refer back to some of this stuff some day.
//
// I did delete _some_ pieces during my huge back-and-forth trying to decide on an approach, so there may be some
// missing bits and bobs here and there.
//
// -----------------------------
//
// I went through a *lot* of back-and-forth with myself when deciding on how I wanted to parse this file. I was trying
// to tick a lot of boxes at once:
//
// - I wanted to avoid the potential edge-cases that might arise from a student entering malicious text inside of the
//   Comments or Submission Field sections.
// - Even though the file format _should_ be pretty set in stone, I wanted to be able to provide good error messages so
//   that, if every something _does_ break, things are easier to fix or debug.
//
// I came up with a few regex-based methods, but I wasn't really confident in them. Despite the fact that I wrote a
// *ton* in that doc-comment down there explaining why it would work, I abandoned my first approach due to its poor
// error reporting capabilities. I tried several times to come up with nice and succinct parser APIs that would let me
// get good error reporting. Every time, though, I very quickly felt like things were growing out of scope. By the time
// I tried a second regex-based approach, my desire to avoid the regex compilation and/or LazyLock overhead had grown.
//
// -----------------------------
//
// Each block of /* */ comments down below is one "attempt." Sometimes, I was working on both simultaneously. That's how
// indecisive I was!
//
// Some of the errors referred to by these chunks of code live in `errors.rs`. You can go and see their definitions
// there (mostly).
//
// =====================================================================================================================
// =====================================================================================================================
// =====================================================================================================================

/*
#[derive(Debug, Clone)]
pub struct MatchStream<'a> {
    buf: &'a str,
    pos: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMode {
    /// Stop the stream at the start of the matched token; do not advance past it.
    Exclusive,
    /// Advance past the matched token.
    Inclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    /// Do not include the terminating character/substring in the capture; do not advance the stream forwards.
    Exclude,
    /// Include the terminating character/substring in the match; advance the stream forwards.
    Include,
    /// Do not include the terminating character/substring in the capture; but do advance the stream forwards.
    ExcludeSkip,
}

impl From<MatchMode> for CaptureMode {
    fn from(value: MatchMode) -> Self {
        match value {
            MatchMode::Exclusive => CaptureMode::Exclude,
            MatchMode::Inclusive => CaptureMode::Include,
        }
    }
}

impl<'a> MatchStream<'a> {
    /// Create a new [`MatchStream`].
    pub fn new(buf: &'a str) -> Self {
        Self { buf, pos: 0 }
    }

    /// Get this stream's position in the underlying buffer.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Get the remaining slice of the original buffer from this stream.
    pub fn remaining(&self) -> &'a str {
        &self.buf[self.pos..]
    }

    /// Advance the buffer forwards a set number of bytes.
    ///
    /// # Panics
    ///
    /// Panics if `n` would push the pointer past the end of the stream (if `n > self.remaining().len()`).
    pub fn advance(&mut self, n: usize) {
        let rem = self.remaining().len();
        assert!(n <= rem, "advance by {n} bytes would overflow (stream has {rem} bytes remaining)");
        self.pos += n;
    }

    /// Look for exactly the given string, starting at the current stream position, and advance the stream past it.
    pub fn match_exactly(&mut self, s: &str) -> Result<&mut Self, ()> {
        if self.remaining().starts_with(s) {
            self.advance(s.len());
            Ok(self)
        } else {
            Err(())
        }
    }

    /// Look for exactly a CRLF sequence or a lone LF at the current position, and advance the stream past it.
    pub fn match_newline(&mut self) -> Result<&mut Self, ()> {
        self.capture_newline()?;
        Ok(self)
    }

    /// Look for, and capture, exactly a CRLF sequence or a lone LF at the current position, and advance the stream past
    /// it.
    ///
    /// A capture may be useful here to detect which of the two newline variants was found.
    pub fn capture_newline(&mut self) -> Result<&'a str, ()> {
        let scan_start = self.pos;
        if self.remaining().starts_with("\r\n") {
            self.advance("\r\n".len());
            Ok(&self.buf[scan_start..self.pos])
        } else if self.remaining().starts_with("\n") {
            self.advance("\n".len());
            Ok(&self.buf[scan_start..self.pos])
        } else {
            Err(())
        }
    }

    pub fn match_until_substr(&mut self, s: &str, mode: MatchMode) -> Result<&mut Self, ()> {
        self.capture_until_substr(s, mode.into())?;
        Ok(self)
    }

    pub fn capture_until_substr(&mut self, s: &str, mode: CaptureMode) -> Result<&'a str, ()> {
        if let Some(i) = self.remaining().find(s) {
            let scan_start = self.pos;
            let (match_end, new_start) = match mode {
                CaptureMode::Exclude => (i, i),
                CaptureMode::Include => (i + s.len(), i + s.len()),
                CaptureMode::ExcludeSkip => (i, i + s.len()),
            };

            self.advance(new_start);
            Ok(&self.buf[scan_start..match_end])
        } else {
            Err(())
        }
    }

    /// Works like [`capture_chars_until`][Self::capture_chars_until], except that `&mut self` is returned instead of
    /// the matched string. This allows for method chaining.
    pub fn match_chars_until<F>(&mut self, pred: F, mode: MatchMode) -> Result<&mut Self, ()>
    where
        F: FnMut(char) -> bool,
    {
        self.capture_chars_until(pred, mode.into())?;
        Ok(self)
    }

    /// Works like [`capture_chars_while`][Self::capture_chars_while], except that `&mut self` is returned instead of
    /// the matched string. This allows for method chaining.
    pub fn match_chars_while<F>(&mut self, pred: F) -> Result<&mut Self, ()>
    where
        F: FnMut(char) -> bool,
    {
        self.capture_chars_while(pred)?; // Ignore capture
        Ok(self)
    }

    /// Advance the stream forwards until a character is detected that matches the given predicate. The section of the
    /// stream that matched is returned.
    pub fn capture_chars_until<F>(&mut self, mut pred: F, mode: CaptureMode) -> Result<&'a str, ()>
    where
        F: FnMut(char) -> bool,
    {
        let scan_start = self.pos;
        let mut found = None;
        for (i, c) in self.remaining().char_indices() {
            if pred(c) {
                // This char is our stopping char. Stop the loop.
                found = match mode {
                    CaptureMode::Exclude => Some(i),
                    CaptureMode::Include => Some(i + c.len_utf8()),
                };
                break;
            }
        }

        // If all characters in the buffer matched, our new starting point is the end of the string. Then the region we
        // matched is then everything *up to*, but not including, the new start. Note that `new_start` is relative to
        // the already-trimmed buffer, not to the entire `self.buf`.
        let new_start = new_start.unwrap_or(self.remaining().len());
        self.advance(new_start);
        Ok(&self.buf[scan_start..scan_start + new_start])
    }

    /// Advance the stream forwards as long as its characters match the given predicate. The section of the stream that
    /// matched is returned.
    pub fn capture_chars_while<F>(&mut self, mut pred: F) -> Result<&'a str, ()>
    where
        F: FnMut(char) -> bool,
    {
        // Capture characters until we see one that *doesn't* match the predicate. We always want this one to be
        // exclusive, since including the terminating char doesn't really fit with "while".
        self.capture_chars_until(move |c| !pred(c), MatchMode::Exclusive.into())
    }

    pub fn capture_regex(&mut self) {}

    /// Advance the stream forward until a pattern matching a regular expression is found. If the regex is not found, an
    /// `Err` is returned and the stream is not advanced.
    ///
    /// Note that the [`Regex::find_at`] method is used for this operation; anchors in the regex will be based on the
    /// entire **original string** that was passed to [`MatchStream::new`], **not** just the remaining buffer.
    pub fn capture_until_regex(&mut self, re: &Regex, mode: CaptureMode) -> Result<&'a str, ()> {
        if let Some(m) = re.find_at(self.buf, self.pos) {
            let scan_start = self.pos;
            let (match_end, new_start) = match mode {
                CaptureMode::Exclude => (m.start(), m.start()),
                CaptureMode::Include => (m.end(), m.end()),
                CaptureMode::ExcludeSkip => (),
            };

            // Note this one is *not* relative to the pre-trimmed buffer.
            self.advance(match_end - self.pos);
            Ok(&self.buf[scan_start..match_end])
        } else {
            Err(())
        }
    }

    /// Works like [`capture_until_regex`][Self::capture_until_regex], except that `&mut self` is returned instead of
    /// the matched string. This allows for method chaining.
    pub fn match_until_regex(&mut self, re: &Regex, mode: MatchMode) -> Result<&mut Self, ()> {
        self.capture_until_regex(re, mode)?;
        Ok(self)
    }
}

fn tmp() -> Result<(), ()> {
    let mut stream = MatchStream::new("");

    let eol = Regex::new(r"(?:\r?\n|\z)").unwrap();
    let comments_start = Regex::new(r"(?Rm)^Comments:\s*$").unwrap();
    let files_start = Regex::new(r"(?Rm)^Files:\s*$").unwrap();

    // stream.match_exactly("Name:")?.match_chars_while(|c| c.is_whitespace())?;
    // let fullname = stream.capture_until_substr("(", CaptureMode::ExcludeSkip)?.trim_end();
    // let username = stream.capture_until_substr(")", CaptureMode::ExcludeSkip)?;
    // stream.match_newline()?;

    let name_line = stream
        .match_exactly("Name:")
        .and_then(|stream| stream.match_chars_while(|c| c.is_whitespace()))
        .and_then(|stream| stream.capture_until_regex(&eol, CaptureMode::ExcludeSkip))
        .map_err(|err| todo!("Failed to parse 'Name': {err:?}"))?;

    let assignment = stream
        .match_exactly("Assignment:")
        .and_then(|stream| stream.match_chars_while(|c| c.is_whitespace()))
        .and_then(|stream| stream.capture_until_regex(&eol, CaptureMode::ExcludeSkip))
        .map_err(|err| todo!("Failed to parse Assignment: {err:?}"))?;

    let datetime = stream
        .match_exactly("Date Submitted:")?
        .match_chars_while(|c| c.is_whitespace())?
        .capture_until_regex(&eol, CaptureMode::ExcludeSkip)?;

    let grade = stream
        .match_exactly("Current Grade:")?
        .match_chars_while(|c| c.is_whitespace())?
        .capture_until_regex(&eol, CaptureMode::ExcludeSkip)?;

    stream.match_newline()?;

    let text_submission = stream
        .match_exactly("Submission Field:")?
        .capture_until_regex(&comments_start, CaptureMode::Exclude)?;
    let comments = stream
        .match_exactly("Comments:")?
        .match_newline()?
        // TODO: make `capture_until_regex_greedy`
        .capture_until_regex(&files_start, CaptureMode::Exclude)?;
    let files = stream.match_exactly("Files:")?.match_newline()?;

    // Functionality needed:
    // - Matching an exact keyword
    // - Matching at least one whitespace
    // - Capturing till end of line, then skipping over the newline
    // - Capturing until a regex first appears
    // - Capturing until a regex last appears
    // - Capturing the remainder of the stream
    // - Scanning until the next whitespace character

    todo!();
}
*/

/*
fn parse_datafile<'a>(body: &'a str) -> Result<DatafileInfo<'a>, DatafileError> {
    let mut stream = TextStream::new(body);

    // The first four sections are all single lines and are all parsed the same.
    macro_rules! get_field {
        ($field:literal) => {
            stream
                .consume_str(concat!($field, ":"))
                .and_then(|stream| stream.consume_spaces(1))
                .and_then(|stream| stream.capture_rest_of_line())
                .map_err(|err| DatafileError::syntax_field($field, err))
        };
    }

    let names = get_field!("Name")?;
    let assignment = get_field!("Assignment")?;
    let datetime = get_field!("Date Submitted")?;
    let grade = get_field!("Current Grade")?;

    // Empty line between 'Current Grade' and 'Submission Field'
    stream.consume_newline().map_err(DatafileError::syntax)?;

    let sub_field = stream
        .consume_str("Submission Field:")
        .and_then(|stream| stream.consume_rest_of_line())
        .and_then(|stream| stream.capture_until_line_lazy("Comments:"))
        .map_err(|err| DatafileError::syntax_field("Submission Field", err))?;

    let comments = stream
        .consume_str("Comments:")
        .and_then(|stream| stream.consume_rest_of_line())
        .and_then(|stream| stream.capture_until_line_greedy("Files:"))
        .map_err(|err| DatafileError::syntax_field("Comments", err))?;

    let files = stream
        .consume_str("Files:")
        .and_then(|stream| stream.consume_rest_of_line())
        .and_then(|stream| stream.capture_remaining())
        .map_err(|err| DatafileError::syntax_field("Files", err))?;

    Ok(DatafileInfo {
        names: parse_names(names).map_err(|err| DatafileError::syntax_field("Name", err))?,
        assignment,
        date_submitted: NaiveDateTime::parse_from_str(datetime, SUBMISSION_DATE_FORMAT)
            .map_err(DatafileError::datetime)?,
        current_grade: Some(grade).filter(|&txt| txt != EMPTY_GRADES_FIELD),
        text_sub: Some(sub_field).filter(|&txt| txt != EMPTY_SUBMISSION_FIELD),
        comments: Some(comments).filter(|&txt| txt != EMPTY_COMMENTS_FIELD),
        files: parse_files(files).map_err(|err| DatafileError::syntax_field("Files", err))?,
    })
}

/// Parses a student's real name and username out of the `Name:` line.
fn parse_names<'a>(line: &'a str) -> Result<StudentNames<'a>, SyntaxError> {
    // I'm not sure if it's possible for names to have `()` in them... but just in case, we'll look for the *last*
    // occurrences of both to find where the username sits.
    let bi = line.rfind('(').ok_or_else(|| SyntaxError {  })?;
    let bj = line.rfind(')').ok_or_else(|| SyntaxError {  })?;

    todo!("parse names");
}

fn parse_files<'a>(body: &'a str) -> Result<SmallVec<[FileNames<'a>; 8]>, SyntaxError> {
    if body == EMPTY_FILES_FIELD {
        return Ok(SmallVec::new());
    }

    let mut files = SmallVec::new();
    let mut stream = TextStream::new(body);

    todo!("parse files");

    Ok(files)
}

#[derive(Debug, Clone)]
struct TextStream<'a> {
    buf: &'a str,
    pos: usize,
}

impl<'a> TextStream<'a> {
    /// Create a new stream.
    pub fn new(buf: &'a str) -> Self {
        Self { buf, pos: 0 }
    }

    /// Get the remaining slice of the original buffer from this stream.
    pub fn remaining(&self) -> &'a str {
        &self.buf[self.pos..]
    }

    /// Helper function for generating the
    fn syntax_error(&self, expected: String) -> SyntaxError {
        // The most sensible thing to report is probably just the next series of characters up to whitespace; but if
        // we're already at whitespace, we'll want to come up with a printable form.
        let mut chars = self.remaining().chars();
        let actual = match chars.next() {
            // Format LF / CRLF specially:
            Some('\n') => String::from("end of line"),
            Some('\r') if chars.next().is_some_and(|c| c == '\n') => String::from("end of line"),
            // Otherwise just say how many whitespace characters we found:
            Some(c) if c.is_whitespace() => {
                let n = chars.take_while(|c| c.is_whitespace()).count() + 1;
                format!("whitespace ({n} characters)")
            },
            // For all other text, grab up to the next whitespace.
            Some(_) => String::from(self.scan_until_whitespace()),
            // If there are zero more characters, we hit EOF.
            None => String::from("end of file"),
        };

        SyntaxError {
            pos: self.pos,
            expected: expected.into_boxed_str(),
            actual: actual.into_boxed_str(),
        }
    }

    /// Scan forwards from the stream's current position until a whitespace character is found. The whitespace character
    /// is not included in the returned string. If the current character is whitespace, an empty string is returned.
    ///
    /// "Scanning" does not move the stream forwards.
    fn scan_until_whitespace(&self) -> &'a str {
        let scan_start = self.pos;
        let mut offset = None;
        for (i, c) in self.remaining().char_indices() {
            if c.is_whitespace() {
                offset = Some(i);
                break;
            }
        }
        let offset = offset.unwrap_or(self.remaining().len());
        &self.buf[scan_start..scan_start + offset]
    }

    /// Look for an exact copy of the given string at the stream's current position, and advance the stream past it if
    /// found.
    pub fn consume_str(&mut self, s: &str) -> Result<&mut Self, SyntaxError> {
        if self.remaining().starts_with(s) {
            self.pos += s.len();
            Ok(self)
        } else {
            Err(self.syntax_error(format!("text \"{s}\"")))
        }
    }

    /// Look for whitespace characters (**other than newlines**) at the stream's current position, and advance the
    /// stream past it if found.
    ///
    /// Use `at_least = 0` to look for optional spaces.
    pub fn consume_spaces(&mut self, at_least: usize) -> Result<&mut Self, SyntaxError> {
        let mut n = 0; // num characters
        let mut b = 0; // num utf-8 bytes
        let mut prev = None;
        for c in self.remaining().chars() {
            // Loop until we hit whitespace;
            if !c.is_whitespace() {
                break;
            } else if c == '\n' {
                // If we hit a newline, we're also done; we want to advance right up to that newline. ...unless the one
                // before the newline was a `\r`. In that case we want to advance up to right before the `\r`.
                if prev == Some('\r') {
                    n -= 1;
                    b -= '\r'.len_utf8();
                }
                break;
            }

            n += 1;
            b += c.len_utf8();
            prev = Some(c);
        }

        if n < at_least {
            let chars = if at_least == 1 { "character" } else { "characters" };
            Err(self.syntax_error(format!("at least {at_least} whitespace {chars}")))
        } else {
            self.pos += b;
            Ok(self)
        }
    }

    pub fn consume_newline(&mut self) -> Result<&mut Self, SyntaxError> {
        if self.remaining().starts_with("\r\n") {
            self.pos += 2;
            Ok(self)
        } else if self.remaining().starts_with("\n") {
            self.pos += 1;
            Ok(self)
        } else {
            Err(self.syntax_error(format!("linebreak")))
        }
    }

    /// Scan until the next line-break in the stream and return all text up to that point. The stream is advanced to the
    /// start of the next line.
    ///
    /// An error is returned if the stream is empty.
    pub fn capture_rest_of_line(&mut self) -> Result<&'a str, SyntaxError> {
        if self.remaining().len() == 0 {
            return Err(self.syntax_error(format!("text")));
        }

        // Look for the next line-break (or EOF)
        let buf = self.remaining();
        let (line_end, advance) = match buf.find("\n") {
            // If we find an LF, but it is immediately preceded by a CR, we want to capture up to the CR but advance
            // past the LF.
            Some(i) if i > 0 && &buf[i - 1..i] == "\r" => (i - 1, i + 1),
            // Otherwise, we just want to capture up to the LF and skip past it.
            Some(i) => (i, i + 1),
            // If there are no more LFs, that's the whole file; capture everything and advance all the way.
            None => (buf.len(), buf.len()),
        };

        self.pos += advance;
        Ok(&buf[..line_end])
    }

    /// Like [`Self::capture_rest_of_line`], but ignores the captured line and returns `self` for method chaining.
    pub fn consume_rest_of_line(&mut self) -> Result<&mut Self, SyntaxError> {
        let _ = self.capture_rest_of_line()?;
        Ok(self)
    }

    /// Captures from the stream's current position until the next line that starts with the given text, or until the
    /// end of the file.
    pub fn capture_until_line_lazy(&mut self, line: &str) -> Result<&'a str, SyntaxError> {
        if self.remaining().len() == 0 {
            return Err(self.syntax_error(format!("text")));
        }

        let mut pos = self.pos;
        let end = loop {
            // Look for very next occurrence:
            if let Some(i) = self.buf[pos..].find(line) {
                // When we find the pattern, we only want to break out of the loop if there is a newline right before it
                // (or if it's right at the start of the file). The `i` we get is relative to the remaining part of the
                // buffer; we need to check if what comes right before it is a newline.
                let real_pos = pos + i;
                if real_pos == 0 || &self.buf[real_pos - 1..real_pos] == "\n" {
                    break real_pos;
                }

                // If there's no newline, bump the scan forwards past the search pattern and try again.
                pos = real_pos + line.len();
            } else {
                // If we didn't find the pattern anywhere, let the end of the capture be the end of the string.
                break self.buf.len();
            }
        };

        let res = &self.buf[self.pos..end];
        self.pos = end;
        Ok(res)
    }

    /// Captures from the stream's current position until the last line that starts with the given text, or until the
    /// end of the file.
    pub fn capture_until_line_greedy(&mut self, line: &str) -> Result<&'a str, SyntaxError> {
        if self.remaining().len() == 0 {
            return Err(self.syntax_error(format!("text")));
        }

        // This method's implementation is almost identical to the one right above it... but I don't think it's worth
        // the effort to try to efficiently combine/abstract the two... And they're functionally different enough that
        // they should probably be two separate methods anyways.
        let mut end = self.buf.len();
        let end = loop {
            if let Some(i) = self.buf[self.pos..end].rfind(line) {
                // Same as the non-greedy version of this method. When we get an `i`, need to determine its actual
                // position in the overall buffer so that we can look and see if a newline comes before it.
                let real_pos = self.pos + i;
                if real_pos == 0 || &self.buf[real_pos - 1..real_pos] == "\n" {
                    break real_pos;
                }

                // Otherwise, shift the end of the search range backwards (to the start of our found match) and try
                // again.
                end = real_pos;
            } else {
                // Didn't find the pattern at the start of a line. Simply return the entire rest of the string.
                break self.buf.len();
            }
        };

        let res = &self.buf[self.pos..end];
        self.pos = end;
        Ok(res)
    }

    /// Advance the stream to the end of the entire rest of the buffer.
    pub fn capture_remaining(&mut self) -> Result<&'a str, SyntaxError> {
        if self.remaining().len() == 0 {
            return Err(self.syntax_error(format!("text")));
        }

        let res = &self.buf[self.pos..];
        self.pos = self.buf.len();
        Ok(res)
    }
}
*/

/*
/// The regular expression used to extract the bulk of the content out of a datafile.
///
/// I know, I know... "I solved a problem with Regex, and now I've got two problems." But it's actually a good solution
/// this time, I swear!
///
/// # Datafile Format
///
/// "Datafiles", as I've been calling them, are files that Blackboard inserts into assignment file downloads to provide
/// meta information about the attempt/submission to the grader. Rather annoyingly, they are formatted as plain text
/// files, and are generally meant to be human readable.
///
/// Every datafile is broken up into **sections**, or **fields**. Each section has a **heading** that ends with a colon.
/// The file starts with some single-line sections, and ends with some are multi-line sections.
///
/// ```txt
/// Name: <FIRST NAME> <LAST NAME> (<USERNAME>)
/// Assignment: <GRADE CENTER NAME>
/// Date Submitted: <DATE, fmt: Thursday, December 12, 2024 1:22:26 PM EST>
/// Current Grade: <GRADE / "Needs Grading">
///
/// Submission Field:
/// <HTML TEXT>
///
/// Comments:
/// <PLAIN TEXT>
///
/// Files:
///     Original filename: <FILENAME>
///     Filename: <ASSN NAME>_<USERNAME>_attempt_<DATE, fmt: 2024-12-12-13-22-26>_<FILENAME>
///
///     Original filename: <FILENAME>
///     Filename: <ASSN NAME>_<USERNAME>_attempt_<DATE, fmt: 2024-12-12-13-22-26>_<FILENAME>
/// ```
///
/// ## Empty sections
///
/// The `Submission Field:`, `Comments:`, and `Files:` sections can also be "empty." However, Blackboard doesn't
/// actually leave the fields empty or omit them. Instead, the sections will look like:
///
/// - Submission Field: `There is no student submission text data for this assignment.` ([`EMPTY_SUBMISSION_FIELD`]).
/// - Comments: `There are no student comments for this assignment.` ([`EMPTY_COMMENTS_FIELD`]).
/// - Files: `No files were attached to this submission.` ([`EMPTY_FILES_FIELD`]).
///
/// # So, why a Regex?
///
/// Or, RE: "I solved a problem with Regex, and now I've got two problems."
///
/// ## The problem with the format
///
/// The main problem with parsing a datafile manually is the fact that the `Submission Field` and `Comments` sections
/// allow the student to enter _whatever text they would like_, but the fields are **not delineated** in the file! That
/// makes doing a line-by-line scan difficult: we can't just scan forwards for lines starting with a section name and a
/// colon to know when to stop, since it's very possible that the student has, for example, put the text `"Files:"` at
/// the start of a line within their submission comments.
///
/// The most problematic case would be if a student placed the words `"Comments:"` at the start of a line within their
/// `Comments` field. Since this field has zero structure whatsoever, there would be no way to determine which of the
/// two is the real section heading. To do so, we need to rely on some key observations about how the other fields in
/// these files are generated:
///
/// 1.  Sections are always in the exact same order, and they are never omitted.
/// 2.  The `Files` section at the end does not allow for free-form input, which means that it will never be possible
///     for one of the other `Section:` keywords to appear there. This allows us to accurately determine when the
///     `Comments` section ends and the `Files` section begins by reading until the _last_ possible occurrence of the
///     `^Files:$` heading in the document.
/// 3.  The `Submission Field:` comes from a [WYSIWYG] editor that always outputs HTML. That means that, under regular
///     usage, a student entering the text `"Comments:"` at the start of a line will see it wrapped in a paragraph tag,
///     stopping it from truly being "at the start of the line". Without this fact, there would be no way to accurately
///     locate the split between `Submission Field` and `Comments` sections, which are otherwise fully free-form fields.
///     - Keen-eyed or experienced Blackboard users will know that, in WYSIWYG content fields, Blackboard lets the
///       author access the HTML source code for the field.
///     - This sounds like it could spell the end of our chances to reliably determine the end of the `Submission
///       Field:` section... But, thankfully, it turns out that **Blackboard sanitizes and formats the HTML from the
///       editor before submitting.** Attempting to edit the source of the text field and add lines _outside_ of the
///       `<p>` tags will see them be wrapped into their own `<p>` tags upon submitting.
///
/// ## The regex
///
/// To make use of those observations, we need to be able to consume lines greedily in some places and lazily in others.
/// Trying to express all of the logic above by hand would be difficult, and would require implementing many complex
/// matching routines. A regular expression provides a concise way to express all these needs all at once.
///
/// To actually extract the fields, we match on the entire file at once. It is necessary to match at least on the last
/// three files at once due to the greedy/lazy behaviour we need, as described above.
///
/// - For the `Submission Field`, we consume lines **non-greedily** up until we see the first occurrence of `Comments:`
///   at the beginning of a line. This uses Fact #3 to accurately locate the start of the `Comments` section.
/// - For `Comments:`, we consume **greedily** until we see the last `Files:` instance in the text.
///
/// ## Downsides
///
/// There are two main downsides to this approach.
///
/// The first is **bad error reporting.** Using a single large regex makes the process simple, and is quite ergonomic
/// from a developer's perspective because it is declarative. However, it means that the only feedback we'll get from
/// [`Regex::captures`] is a simple `None` when a datafile doesn't match exactly. It would be nice to know how far into
/// the matching process the regex engine got before being unable to find a match. Perhaps a more elegant solution to
/// this whole thing would be to use a proper parser generator library like [`chumsky`] or [`winnow`].
///
/// The second issue is **fragility.** Using one regex to match the entire file means that any breaking change from
/// Blackboard _anywhere_ in the file will break the _entire_ thing.
///
/// **However!** Because of the slow-moving, "corporate-ness" of Blackboard, I feel like the odds of them changing the
/// format of their gradebook files anytime soon are quite small.
///
/// [WYSIWYG]: https://en.wikipedia.org/wiki/WYSIWYG
/// [`chumsky`]: https://github.com/zesterer/chumsky
/// [`winnow`]: https://github.com/winnow-rs/winnow
static RE_DATAFILE_CONTENTS: LazyLock<Regex> = LazyLock::new(|| {
    // NB: Because we explicitly match every newline, we don't need any `^` or `$` anywhere. And, actually, we **need**
    // to use explicit matches, since `^` and `$` are zero-width matches; they wouldn't actually "consume" the newline.
    Regex::new(
        r#"(?x)
        \A                                          # Anchor start of haystack.
        Name:\ (?<names>.*?)            \s* \r?\n
        Assignment:\ (?<assn>.*?)       \s* \r?\n   # Capture these fields non-greedily to exclude an any whitespace at
        Date\ Submitted:\ (?<date>.*?)  \s* \r?\n   # the ends of the lines from the capture groups.
        Current\ Grade:\ (?<grade>.*?)  \s* \r?\n
                                        \r?\n
        Submission\ Field:              \r?\n
        (?<sub_field>(?s).*?)           \r?\n       # NB: (?s) flag captures lines. Non-greedy to stop at *first*
                                        \r?\n       # instance of 'Comments:' that we see.
        Comments:                       \r?\n
        (?<comments>(?s).*)             \r?\n       # NB: Greedy this time, to stop at *last* 'Files:' instance we see.
                                        \r?\n
        Files:                          \r?\n       # NB: Files capture is non-greedy to exclude the final newline from
        (?<files>(?s).*?)               (?:\r?\n)?  # group; newline is optional, just in case there are none at all.
        \z                                          # Anchor end of haystack.
        "#,
    )
    .unwrap()
});

/// The format specifier used to parse datetimes out of datafiles' `Date submitted:` lines.
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

/// Locates and parses all the textual pieces of a datafile's contents before it gets merged back into the main parser
/// state.
fn read_datafile<'a>(body: &'a str) -> Result<DatafileInfo<'a>, DatafileParseError> {
    use self::datafile::SyntaxError;

    if !body.starts_with("Name:") {
        return Err(SyntaxError {  })
    }


    let caps = RE_DATAFILE_CONTENTS.captures(body).ok_or(DatafileParseError::FileFormat)?;

    // Unwrapping all the capture groups is fully safe since the regex is anchored to the start and end of the haystack,
    // and none of the capture groups are optional. That means that either the full pattern will match and include all
    // captures, or nothing will match (which is handled by `ok_or` on the match).
    let names = caps.name("names").unwrap().as_str();
    let assignment = caps.name("assn").unwrap().as_str();
    let datetime = caps.name("date").unwrap().as_str();
    let grade = caps.name("grade").unwrap().as_str();
    let sub_field = caps.name("sub_field").unwrap().as_str();
    let comments = caps.name("comments").unwrap().as_str();
    let files = caps.name("files").unwrap().as_str();

    Ok(DatafileInfo {
        names: parse_names(names)?,
        assignment,
        datetime: NaiveDateTime::parse_from_str(datetime, SUBMISSION_DATE_FORMAT)?,
        current_grade: Some(grade).filter(|&txt| txt != EMPTY_GRADES_FIELD),
        text_sub: Some(sub_field).filter(|&txt| txt != EMPTY_SUBMISSION_FIELD),
        comments: Some(comments).filter(|&txt| txt != EMPTY_COMMENTS_FIELD),
        files: parse_files(files)?,
    })
}
*/

/*
fn parse_datafile<'a>(body: &'a str) -> Result<DatafileInfo<'a>, DatafileParseError> {
    static FIELD_REGEXES: LazyLock<[Regex; 7]> = LazyLock::new(|| {
        // Using multiple regexes like this is a trade-off between simplicity and error-handling. Splitting things up
        // lets us match on the individual pieces, which allows us to detect at exactly which stage things went wrong.
        // This lets us provide a more accurate error message for when Blackboard does inevitably change the file format
        // they're using.
        //
        // Each regex will be run against the haystack starting directly after the one before it.
        [
            // Each field should be on its own line. So, for each regex, the preceding pattern matches on a newline. We
            // also check for an end-of-haystack anchor; this lets us detect missing subsequent fields with subsequent
            // regexes, instead of throwing an error on the missing newline.
            r"(?x)\A            Name:               \s+         (?<field>.*)            (?:\r?\n|\z)",
            r"(?x)^             Assignment:         \s+         (?<field>.*)            (?:\r?\n|\z)",
            r"(?x)^             Date\ Submitted:    \s+         (?<field>.*)            (?:\r?\n|\z)",
            r"(?x)^             Current\ Grade:     \s+         (?<field>.*)            (?:\r?\n|\z)",
            // For this one, we anchor the end of the Submission Field based on the start of the Comments field. We
            // capture non-greedily so that we stop on the *first* occurrence of 'Comments:'. This prevents us from
            // going too far in the event that the student puts 'Comments:' *inside* their Comments field. Again,
            // though, if we don't detect a 'Comments' field, we also allow matching till the very end of the string.
            r"(?x)(?:\r?\n)?^   Submission\ Field:  \s*?\r?\n   (?<field>(?s).*?)       (?:(?:\r?\n)+Comments:\s*$|\z)",
            // This one doesn't need the 'Comments:' heading in it since it would have been matched by the previous
            // line.
            r"(?x)\r?\n         (?<field>(?s).*)",
            r"(?x)^     ",
        ]
        .map(|pat| RegexBuilder::new(pat).multi_line(true).crlf(true).build().unwrap())
    });

    todo!();
}

/// Regex used to extract a student's full name and username from datafiles.
static STUDENT_NAME_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(.+)\s+\((.+)\)\s*$").unwrap());

/// Parses a student's real name and username out of the `Name:` line.
fn parse_names<'a>(line: &'a str) -> Result<StudentNames<'a>, DatafileParseError> {
    let caps = STUDENT_NAME_REGEX.captures(line).ok_or(DatafileParseError::FileFormat)?;
    let fullname = caps.get(1).unwrap().as_str();
    let username = caps.get(2).unwrap().as_str();
    Ok(StudentNames { fullname, username })
}

fn parse_files<'a>(body: &'a str) -> Result<SmallVec<[FileNames<'a>; 8]>, DatafileParseError> {
    if body == EMPTY_FILES_FIELD {
        return Ok(SmallVec::new());
    }

    let mut files = SmallVec::new();

    todo!("parse files");

    Ok(files)
}
*/

/*
fn parse_datafile<'a>(body: &'a str) -> Result<DatafileInfo<'a>, Box<dyn Error>> {
    static FIELD_REGEXES: LazyLock<[Regex; 7]> = LazyLock::new(|| {
        // Using multiple regexes like this is a trade-off between simplicity and error-handling. Splitting things up
        // lets us match on the individual pieces, which allows us to detect at exactly which stage things went wrong.
        // This lets us provide a more accurate error message for when Blackboard does inevitably change the file format
        // they're using.
        //
        // Each regex will be run against the haystack starting directly after the one before it.
        [
            // Each field should be on its own line. Since we need to actually "consume" the line breaks, each pattern
            // ends with a match on CRLF/LF. However, we also allow each pattern to match successfully if end-of-string
            // is encountered instead. That way, if for example the file contains only "Name: ..." and ends, the error
            // we report will not be "invalid Name field" but "missing Assignment field."
            //
            // The first four fields are relatively straightforward. There are no blank lines between anything, and
            // everything is a single line.
            r"(?x)\A            Name:               \s+         (?<field>.*)            (?:\r?\n|\z)",
            r"(?x)^             Assignment:         \s+         (?<field>.*)            (?:\r?\n|\z)",
            r"(?x)^             Date\ Submitted:    \s+         (?<field>.*)            (?:\r?\n|\z)",
            r"(?x)^             Current\ Grade:     \s+         (?<field>.*)            (?:\r?\n|\z)",
            // For this one, we anchor the end of the Submission Field based on the start of the Comments field. We
            // capture non-greedily so that we stop on the *first* occurrence of 'Comments:'. This prevents us from
            // going too far in the event that the student puts 'Comments:' *inside* their Comments field. Again,
            // though, if we don't detect a 'Comments' field, we also allow matching till the very end of the string.

            // There should be a newline before the 'Submission Field' field, so we capture an extra newline at the
            // start of this one.
            //
            //
            r"(?x)(?:\r?\n)?^   Submission\ Field:  \s*?\r?\n   (?<field>(?s).*?)       (?:(?:\r?\n)+Comments:\s*$|\z)",
            // This one doesn't need the 'Comments:' heading in it since it would have been matched by the previous
            // line.
            r"(?x)\r?\n         (?<field>(?s).*)",
            r"(?x)^     ",
        ]
        .map(|pat| RegexBuilder::new(pat).multi_line(true).crlf(true).build().unwrap())
    });

    todo!()
}
*/

/* struct TextStream<'a> {
    buf: &'a str,
    pos: usize,
}

impl<'a> TextStream<'a> {} */

// - Given a substring, return everything up to, but not including that substring; do not go past the end of the line. Return an
// - Given a substring, advance past that exact substring.
//
// I want an API that looks something like:
//
// ```
// stream.parse(&[
//   Exact("Name:"),
//   OneOrMore(Exact(|c| c.is_whitespace())),
//   Capture(Anything, &mut fullname),
//   Exact("("),
//   Capture(Anything, &mut username),
//   Exact(")"),
//   Exact(LineBreak),
// ]);
//
// stream.parse(&[
// ]);
// ```
//
// To do this, we'll need backtracking.
//
// - `&str`, `char`, and `Fn(char) -> bool` all implement a trait that lets me search for them (like the standard
//   `Pattern` trait). Make a few unit structs that also implement that trait (`Anything`, `LineBreak`, etc.).
// - `Exact`, `OneOrMore`, `Capture`, etc. are like... a `Piece::*` enum or something.
//
// In the `parse` method:
//
// - Try to match that specific piece.

/* fn parse_exact<'s>(buf: &mut &'s str, str: &str) -> Result<&'s str, ()> {

}

fn one_or_more<'s>(buf: &mut &'s str, parser: impl FnMut()) -> Result<>
*/
