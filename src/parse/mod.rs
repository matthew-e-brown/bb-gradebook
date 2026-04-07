//! Implementation of Gradebook parsing routines.
//!
//! The main entrypoint of this module is the [`parse_gradebook`] function.

use std::collections::hash_map::RandomState; // Use the same default hasher as the std hashmap
use std::hash::BuildHasher;
use std::io::{Read, Seek};

use hashbrown::HashTable;
use smallvec::SmallVec;

use self::datafile::DatafileInfo;
use crate::error::GradebookLoadError;
use crate::{AttemptInfo, FileInfo, GradebookInfo, StudentInfo, ZipArchive};

mod datafile;

/// Attempts to parse the contents of a [`ZipArchive`] into a [`GradebookInfo`].
pub fn parse_gradebook<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<GradebookInfo, GradebookLoadError> {
    GradebookParser::new().parse(archive)
}

/// Parser state for parsing a [`GradebookInfo`] from a Blackboard gradebook file.
struct GradebookParser {
    assignment_name: Option<String>,
    attempts: Vec<AttemptInfo>,
    students: Vec<StudentInfo>,
    files: Vec<FileInfo>,
    // Attempts and files each appear only once; students, however, can appear multiple times throughout the parsing
    // process. So we use a lookup table to check for students that already exist as we parse.
    student_table: HashTable<usize>,
    student_hasher: RandomState,
}

impl GradebookParser {
    pub fn new() -> Self {
        Self {
            assignment_name: None,
            attempts: Vec::new(),
            students: Vec::new(),
            files: Vec::new(),
            student_table: HashTable::new(),
            student_hasher: RandomState::new(),
        }
    }

    pub fn parse<R: Read + Seek>(mut self, archive: &mut ZipArchive<R>) -> Result<GradebookInfo, GradebookLoadError> {
        let datafiles = find_datafiles(&archive);
        if datafiles.len() == 0 {
            return Err(GradebookLoadError::Empty);
        }

        // There may be fewer students than attempts, but it's probably a good enough default; we can shrink it at the
        // end if we need to. Likewise, there will likely be more files than there are attempts, but it feels like a
        // pretty sane default.
        self.attempts.reserve_exact(datafiles.len());
        self.students.reserve_exact(datafiles.len());
        self.files.reserve_exact(datafiles.len());

        let mut df_buffer = String::new(); // Re-use the same buffer for all files
        for df_index in datafiles {
            // Indices from `find_datafiles` are safe to unwrap since they came right from
            df_buffer.clear();
            archive.by_index(df_index).unwrap().read_to_string(&mut df_buffer)?;

            let DatafileInfo {
                names,
                assignment: assignment_name,
                date_submitted,
                current_grade,
                submission_field,
                comments,
                files,
            } = match datafile::parse_datafile(&df_buffer) {
                Ok(info) => info,
                Err(err) => {
                    // [TODO] Don't cancel the entire thing over one bad datafile; find some way to gracefully collect
                    // failed datafile parses elsewhere and report them separately.
                    let filename = archive.name_for_index(df_index).unwrap().to_owned();
                    return Err(GradebookLoadError::Datafile { filename, inner: err });
                },
            };

            // Each new datafile is a new attempt, so we know this index without needing to check anything; but the
            // student's index might be re-used. For files, all the files we're about to push will live contiguously
            // starting right after the current set of files we're storing, so we can use a range (unless we're not
            // pushing *any* files).
            let attempt_index = self.attempts.len();
            let student_index = self.get_or_create_student(names.fullname, names.username);
            let files_range = (files.len() > 0).then_some(self.files.len()..self.files.len() + files.len());

            self.students[student_index].attempts.push(attempt_index);

            self.attempts.push(AttemptInfo {
                student: student_index,
                datetime: date_submitted,
                text_submission: submission_field.map(str::to_owned),
                comments: comments.map(str::to_owned),
                current_grade: current_grade.map(str::to_owned),
                files: files_range,
            });

            for filenames in files {
                let zip_index = match archive.index_for_name(filenames.archive) {
                    Some(index) => index,
                    None => {
                        let datafile = archive.name_for_index(df_index).unwrap().to_owned();
                        let filename = filenames.archive.to_owned();
                        return Err(GradebookLoadError::FileNotFound { datafile, filename });
                    },
                };

                let zipfile = archive.by_index(zip_index).unwrap();
                self.files.push(FileInfo {
                    attempt: attempt_index,
                    zip_index,
                    original_name: filenames.original.to_owned(),
                    archive_name: filenames.archive.to_owned(),
                    size_zipped: zipfile.compressed_size(),
                    size_unzipped: zipfile.size(),
                });
            }

            // Finally, write down the assignment name if we don't have it already.
            if self.assignment_name.is_none() {
                self.assignment_name = Some(assignment_name.to_string());
            }
        }

        // [NOTE] Will have to change this unwrap once addressing the `TODO` up there, since it will then be possible to
        // get down this far without having exited the function. But in that case, we can just make "all datafiles
        // failed" be an error case; then this unwrap will be in a different branch.
        let assignment_name = self
            .assignment_name
            .expect("at least one datafile has been successfully parsed");

        Ok(GradebookInfo {
            assignment_name,
            students: self.students,
            attempts: self.attempts,
            files: self.files,
        })
    }

    fn get_or_create_student(&mut self, fullname: &str, username: &str) -> usize {
        let entry = self
            .student_table
            .entry(
                self.student_hasher.hash_one(&username[..]),
                |&i| &self.students[i].username[..] == username,
                |&i| self.student_hasher.hash_one(&self.students[i].username[..]),
            )
            .or_insert_with(|| {
                self.students.push(StudentInfo {
                    username: username.to_owned(),
                    fullname: fullname.to_owned(),
                    attempts: SmallVec::new(),
                });
                self.students.len() - 1 // Insert the index of the new student
            });
        *entry.get()
    }
}

/// Inspects the list of files in a gradebook archive and figures out which ones are Blackboard's auto-generated `txt`
/// files (**datafiles**).
///
/// Returns a vector containing the indices of the datafiles in the archive. Use this with [`ZipArchive::by_index`] to
/// actually access the files.
fn find_datafiles<R: Read + Seek>(archive: &ZipArchive<R>) -> Vec<usize> {
    // --- APPROACH ---
    //
    // Every gradebook file contains a series of auto-generated `.txt` files that give metadata on each assignment.
    // These data files are our ticket to reliably determining information about the submissions without having to rely
    // on trying to parse that information from filenames, which would be messy and fragile.
    //
    // Every filename in the gradebook looks like: `<dropbox>_<username>_attempt_<datetime>_<original-filename>`. The
    // datafiles can be identified because they lack the `<original-filename>` section, and instead just end with a
    // `.txt` extension. Attempting to find these files by regex-matching on `_attempt_<datetime>.txt$` is tempting, but
    // error-prone.
    //
    // We want some other way to differentiate between:
    //
    // - `<dropbox>_<username>_attempt_<timestamp>.txt`
    // - `<dropbox>_<username>_attempt_<timestamp>_<filename>.<ext>`
    //
    // Key observation: no matter what the student names their file, it will have an underscore where the Blackboard
    // file has a dot. Dots are is `U+002E`; underscores are `U+005F`. So, if we sort the list of filenames
    // lexicographically, the dot will always be first!

    // [FIXME] Hmm... One problem I've just noticed. This new algorithm doesn't actually verify that the filenames are
    // actually valid datafile names! You can feed it any random zip file, and it'll try and interpret its contents as
    // if it was a Blackboard gradebook file... So maybe it would be wise to bring back a (less strict) regex to
    // validate that the filenames do actually match.

    let mut datafiles = Vec::new();

    if archive.len() > 0 {
        let mut files = (0..archive.len())
            .map(|index| (index, archive.name_for_index(index).expect("index is between 0 and length")))
            .collect::<Vec<_>>();
        files.sort_unstable_by(|(_, a), (_, b)| a.as_bytes().cmp(b.as_bytes()));
        let mut files = files.into_iter();

        // The first file is guaranteed to be a datafile, since (a) there is at least one 1 submission, (b) all
        // submissions have a datafile, and (c) each submission's datafile will be lexicographically before its others.
        let (first_idx, first_df) = files.next().unwrap();
        datafiles.push(first_idx);

        // To find where the next submission starts, we loop through the list of files, skipping them as long as they
        // share a common prefix with the most recent datafile. Specifically, a common prefix with a length equal to
        // `prev.len() - ".txt".len()`. This is the longest common prefix there could possibly be with a datafile. If
        // the current file's prefix length is *shorter* than that, it means they differ somewhere before the end of the
        // `<timestamp>` section, and therefore belong to different submissions; `curr` marks the start of the next
        // submission, and is therefore a datafile.
        let mut prev = first_df;
        while let Some((index, curr)) = files.next() {
            let s1 = prev.as_bytes().into_iter();
            let s2 = curr.as_bytes().into_iter();
            let prefix_len = s1.zip(s2).take_while(|(a, b)| a == b).count();
            if prefix_len < prev.len() - ".txt".len() {
                datafiles.push(index);
                prev = curr;
            }
        }

        // Now that we've found all the datafiles, sort the indices so they come out in the original order of the zip
        // file (which are sorted by submission time, not by student username).
        datafiles.sort_unstable();
    }

    datafiles
}
