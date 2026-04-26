use std::io::{Read, Seek};
use std::ops::Range;

use smallvec::SmallVec;

pub use crate::error::Error;

pub mod error;
pub mod parse;

/// Re-export of [`zip::ZipArchive`], used as the source for a [`GradebookArchive`].
pub use zip::ZipArchive;

/// A rich representation of a gradebook file downloaded from Blackboard.
///
/// A **gradebook** in this context is not _really_ a "gradebook" (as in, a thing where grades are stored). Rather, it
/// is so named because of the format of the zip files given by Blackboard upon download:
/// `gradebook_<course-code>_<assignment-name>_<date>.zip`.
///
/// This struct holds a [`ZipArchive`] reader of the underlying zip file alongside submission metadata so that it may be
/// inspected and filtered before extracting. See [`GradebookInfo`] for a lower-level type that does not keep hold of
/// the initial [`ZipArchive`], which may be more versatile.
#[derive(Debug, Clone)]
pub struct GradebookArchive<R> {
    archive: ZipArchive<R>,
    info: GradebookInfo,
}

/// Information about a [`GradebookArchive`][GradebookArchive].
///
/// This type is a lower-level type that holds *only* metadata; it does not hold onto the original [`ZipArchive`] reader
/// it was parsed from. This is helpful for decoupling the processing an extraction steps.
#[derive(Debug, Clone)]
pub struct GradebookInfo {
    assignment_name: String,
    students: Vec<StudentInfo>,
    attempts: Vec<AttemptInfo>,
    files: Vec<FileInfo>,
}

/// Information about a single student in a [`GradebookArchive`].
#[derive(Debug, Clone)]
pub struct StudentInfo {
    /// The student's username.
    username: String,
    /// The student's full name.
    fullname: String,
    /// A list of indices into [`GradebookInfo::attempts`] for all the attempts that belong to this student.
    attempts: SmallVec<[usize; 8]>,
}

#[derive(Debug, Clone)]
pub struct AttemptInfo {
    /// The index of the [StudentInfo] (within [`GradebookInfo::students`]) who submitted this attempt.
    student: usize,
    /// When this attempt was submitted.
    date_submitted: chrono::NaiveDateTime,
    /// Any text that the student provided in the "Text Submission" field on Blackboard's interface.
    text_submission: Option<String>,
    /// Any comments provided by the student when submitting.
    comments: Option<String>,
    /// The current grade for this attempt, if applicable.
    current_grade: Option<String>,
    /// A range of indices inside of [`GradebookInfo::files`] for all the [`FileInfo`] attached to this attempt.
    files: Option<Range<usize>>,
}

#[derive(Debug, Clone)]
pub struct FileInfo {
    /// The index of the [AttemptInfo] (within [`GradebookInfo::attempts`]) that this file belongs to.
    attempt: usize,
    /// The index within the underlying [`ZipArchive`] that this file lives at.
    zip_index: usize,
    /// The original name of the file, as uploaded by the student.
    original_name: String,
    /// The name of the file within Blackboard's gradebook file.
    archive_name: String,
    /// The approximate size of this file after unzipping the gradebook.
    size_unzipped: u64,
    /// The compressed size of this file within the gradebook.
    size_zipped: u64,
}

impl<R: Read + Seek> GradebookArchive<R> {
    /// Creates a new [`ZipArchive`] over the given reader and parses its contents as a Blackboard gradebook.
    pub fn from_reader(reader: R) -> Result<Self, Error> {
        let archive = ZipArchive::new(reader).map_err(Error::ZipOpen)?;
        Self::from_archive(archive)
    }

    /// Parses a [`ZipArchive`]'s contents as a Blackboard gradebook.
    pub fn from_archive(mut archive: ZipArchive<R>) -> Result<Self, Error> {
        let info = GradebookInfo::from_archive(&mut archive)?;
        Ok(Self { archive, info })
    }

    /// Returns a reference to this gradebook's [`GradebookInfo`].
    pub fn info(&self) -> &GradebookInfo {
        &self.info
    }

    /// Discard this gradebook archive's underlying reader and return just the underlying [`GradebookInfo`].
    pub fn into_info(self) -> GradebookInfo {
        self.info
    }
}

impl<R: Read + Seek> AsRef<GradebookInfo> for GradebookArchive<R> {
    /// Returns a reference to this gradebook's [`GradebookInfo`].
    fn as_ref(&self) -> &GradebookInfo {
        self.info()
    }
}

// Not sure if I'm a huge fan of repeating all the getters from GradebookInfo... but it'll make the two more
// interchangeable.
impl<R: Read + Seek> GradebookArchive<R> {
    /// Returns the name of this gradebook's associated assignment.
    pub fn assignment_name(&self) -> &str {
        self.info.assignment_name()
    }

    /// Returns a list of information about the students in this gradebook.
    pub fn students(&self) -> &[StudentInfo] {
        self.info.students()
    }

    /// Returns a list of information about the individual attempts in this gradebook.
    pub fn attempts(&self) -> &[AttemptInfo] {
        self.info.attempts()
    }

    /// Returns a list of information about all the submitted files in this gradebook.
    pub fn files(&self) -> &[FileInfo] {
        self.info.files()
    }
}

impl GradebookInfo {
    /// Creates a new [`ZipArchive`] over the given reader and parses its contents as a Blackboard gradebook.
    pub fn from_reader<R: Read + Seek>(reader: R) -> Result<GradebookInfo, Error> {
        let mut archive = ZipArchive::new(reader).map_err(Error::ZipOpen)?;
        Self::from_archive(&mut archive)
    }

    /// Parses a [`ZipArchive`]'s contents as a Blackboard gradebook.
    pub fn from_archive<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<GradebookInfo, Error> {
        parse::parse_gradebook(archive)
    }

    /// Returns the name of this gradebook's associated assignment.
    pub fn assignment_name(&self) -> &str {
        &self.assignment_name
    }

    /// Returns a list of information about the students in this gradebook.
    pub fn students(&self) -> &[StudentInfo] {
        &self.students
    }

    /// Returns a list of information about the individual attempts in this gradebook.
    pub fn attempts(&self) -> &[AttemptInfo] {
        &self.attempts
    }

    /// Returns a list of information about all the submitted files in this gradebook.
    pub fn files(&self) -> &[FileInfo] {
        &self.files
    }
}

impl StudentInfo {
    /// Returns the student's username.
    pub fn username(&self) -> &str {
        &self.username
    }

    /// Returns the student's full name.
    pub fn fullname(&self) -> &str {
        &self.fullname
    }

    /// Returns the indices of the attempts that belong to this student.
    ///
    /// See [`GradebookInfo::attempts()`].
    pub fn attempt_indices(&self) -> &[usize] {
        // [NOTE] [TODO] Something to think about.
        //
        // Instead of returning a `usize` slice directly, it may be preferable to return a custom `Attempts` iterator.
        // That way, if we ever decide to update the internals to use a different data structure (i.e. a BTree or
        // something), it won't be a breaking change when we no longer have access to a contiguous slice.
        &self.attempts
    }
}

impl AttemptInfo {
    /// Returns the index of the student that made this attempt.
    ///
    /// See [`GradebookInfo::students()`].
    pub fn student_index(&self) -> usize {
        self.student
    }

    /// Returns the date and time at which this attempt was submitted to Blackboard.
    ///
    /// Blackboard's datafiles do not include any timezone information, so a [`NaiveDateTime`][chrono::NaiveDateTime] is
    /// the best we can do.
    pub fn date_submitted(&self) -> chrono::NaiveDateTime {
        self.date_submitted
    }

    /// Returns the "Text Submission" for this attempt, if one was provided.
    pub fn text_submission(&self) -> Option<&str> {
        self.text_submission.as_deref()
    }

    /// Returns any student comments for this attempt, if they were provided.
    pub fn comments(&self) -> Option<&str> {
        self.comments.as_deref()
    }

    /// Returns the current grade of this attempt, if it has one.
    ///
    /// Attempts that were marked as "Needs Grading" will return `None`.
    ///
    /// The current grade is returned as a string because Blackboard allows the primary grade display to be configured
    /// as anything from a percentage score, to an A–F letter grade, to institution-specific formats.
    pub fn current_grade(&self) -> Option<&str> {
        self.current_grade.as_deref()
    }

    /// Returns the range of indices of all the files associated with this attempt.
    ///
    /// See [`GradebookInfo::files()`].
    pub fn file_indices(&self) -> Option<Range<usize>> {
        self.files.clone() // std::ops::Range<usize> still isn't Copy for some reason
    }
}

impl FileInfo {
    /// Returns the index of the attempt this file belongs to.
    ///
    /// See [`GradebookInfo::attempts()`].
    pub fn attempt_index(&self) -> usize {
        self.attempt
    }

    /// Returns this file's index in the original [`ZipArchive`] that the gradebook was parsed from.
    ///
    /// This index can be used directly with [`ZipArchive::by_index`] to retrieve a [`ZipFile`] from the `zip` crate to
    /// allow for extracting the file.
    ///
    /// [`ZipFile`]: zip::read::ZipFile
    pub fn zip_index(&self) -> usize {
        self.zip_index
    }

    /// Returns the name of this file, as submitted by the student.
    pub fn original_name(&self) -> &str {
        &self.original_name
    }

    /// Returns the name of this file as it appears inside the [`ZipArchive`] (after being renamed by Blackboard for
    /// packaging in the gradebook zip).
    pub fn archive_name(&self) -> &str {
        &self.archive_name
    }

    /// Returns the approximate size of this file post-extraction from the [`ZipArchive`], in bytes.
    pub fn size(&self) -> u64 {
        self.size_unzipped
    }

    /// Returns the compressed size of this file within the [`ZipArchive`], in bytes.
    pub fn size_zipped(&self) -> u64 {
        self.size_zipped
    }
}
