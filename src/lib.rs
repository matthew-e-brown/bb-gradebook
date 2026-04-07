use std::io::{Read, Seek};
use std::ops::Range;

use chrono::NaiveDateTime;
use smallvec::SmallVec;
pub use zip::read::ZipArchive;

pub use crate::error::GradebookLoadError;

pub mod error;
mod parse;

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
    datetime: NaiveDateTime,
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
    /// The compressed size of this file within the gradebook.
    size_zipped: u64,
    /// The approximate size of this file after unzipping the gradebook.
    size_unzipped: u64,
}

impl<R: Read + Seek> GradebookArchive<R> {
    pub fn from_reader(reader: R) -> Result<Self, GradebookLoadError> {
        let mut archive = ZipArchive::new(reader)?;
        let info = GradebookInfo::from_archive(&mut archive)?;
        Ok(Self { archive, info })
    }

    pub fn info(&self) -> &GradebookInfo {
        &self.info
    }
}

impl GradebookInfo {
    pub fn from_reader<R: Read + Seek>(reader: R) -> Result<GradebookInfo, GradebookLoadError> {
        let mut archive = ZipArchive::new(reader)?;
        Self::from_archive(&mut archive)
    }

    pub fn from_archive<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<GradebookInfo, GradebookLoadError> {
        parse::parse_gradebook(archive)
    }
}
