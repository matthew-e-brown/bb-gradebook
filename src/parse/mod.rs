//! Implementation of Gradebook parsing routines.
//!
//! The main entrypoint of this module is the [`parse_gradebook`] function.

use std::collections::hash_map::RandomState; // Use the same default hasher as the std hashmap
use std::hash::BuildHasher;
use std::io::{Read, Seek};

use hashbrown::HashTable;
use smallvec::SmallVec;

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
            df_buffer.clear();
            archive.by_index(df_index)?.read_to_string(&mut df_buffer)?;

            // [TODO] Don't cancel the entire thing over one bad datafile; find some way to gracefully collect failed
            // datafile parses elsewhere and report them separately.
            let datafile = match datafile::parse_datafile(&df_buffer) {
                Ok(info) => info,
                Err(err) => {
                    let filename = archive
                        .name_for_index(df_index)
                        .expect("df_index is already known to be valid")
                        .to_owned();
                    return Err(GradebookLoadError::Datafile { filename, inner: err });
                },
            };

            println!("Parsed datafile:\n{:#?}\n", datafile);
        }

        todo!();
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

    let mut datafiles = Vec::new();

    if archive.len() > 0 {
        let mut files = archive.file_names().collect::<Vec<_>>();
        files.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        let mut files = files.into_iter();

        // The first file is guaranteed to be a datafile, since (a) there is at least one 1 submission, (b) all
        // submissions have a datafile, and (c) each submission's datafile will be lexicographically before its others.
        let first_df = files.next().unwrap();
        datafiles.push(archive.index_for_name(first_df).unwrap());

        // To find where the next submission starts, we loop through the list of files, skipping them as long as they
        // share a common prefix with the most recent datafile. Specifically, a common prefix with a length equal to
        // `prev.len() - ".txt".len()`. This is the longest common prefix there could possibly be with a datafile. If
        // the current file's prefix length is *shorter* than that, it means they differ somewhere before the end of the
        // `<timestamp>` section, and therefore belong to different submissions; `curr` marks the start of the next
        // submission, and is therefore a datafile.
        let mut prev = first_df;
        while let Some(curr) = files.next() {
            let s1 = prev.as_bytes().into_iter();
            let s2 = curr.as_bytes().into_iter();
            let prefix_len = s1.zip(s2).take_while(|(a, b)| a == b).count();
            if prefix_len < prev.len() - ".txt".len() {
                datafiles.push(archive.index_for_name(curr).unwrap());
                prev = curr;
            }
        }

        // Now that we've found all the datafiles, sort the indices so they come out in the original order of the zip
        // file (which are sorted by submission time, not by student username).
        datafiles.sort_unstable();
    }

    datafiles
}
