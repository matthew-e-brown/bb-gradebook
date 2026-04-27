//! Implementation of Gradebook parsing routines.
//!
//! The main entrypoint of this module is the [`parse_gradebook`] function.

use std::collections::hash_map::RandomState; // Use the same default hasher as the std hashmap
use std::hash::BuildHasher;
use std::io::{Read, Seek};

use hashbrown::HashTable;
use smallvec::SmallVec;

use self::datafile::DatafileInfo;
pub use self::datafile::error::ParseError as DatafileError;
use crate::error::{Error, GradebookError};
use crate::{AttemptInfo, FileInfo, GradebookInfo, StudentInfo, ZipArchive};

mod datafile;

/// Attempts to parse the contents of a [`ZipArchive`] into a [`GradebookInfo`].
pub fn parse_gradebook<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<GradebookInfo, Error> {
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

    pub fn parse<R: Read + Seek>(mut self, archive: &mut ZipArchive<R>) -> Result<GradebookInfo, Error> {
        let datafiles = find_datafiles(&archive)?;

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
                    let filename = archive.name_for_index(df_index).unwrap();
                    return Err(GradebookError::datafile(filename, err).into());
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
                date_submitted,
                text_submission: submission_field.map(str::to_owned),
                comments: comments.map(str::to_owned),
                current_grade: current_grade.map(str::to_owned),
                files: files_range,
            });

            for filenames in files {
                let zip_index = match archive.index_for_name(filenames.archive) {
                    Some(index) => index,
                    None => {
                        let datafile = archive.name_for_index(df_index).unwrap();
                        let filename = filenames.archive;
                        return Err(GradebookError::bad_filename(datafile, filename).into());
                    },
                };

                let zipfile = archive.by_index(zip_index).unwrap();
                self.files.push(FileInfo {
                    attempt: attempt_index,
                    zip_index,
                    original_name: filenames.original.to_owned(),
                    archive_name: filenames.archive.to_owned(),
                    size_unzipped: zipfile.size(),
                    size_zipped: zipfile.compressed_size(),
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

/// Inspects the list of files in a [`ZipArchive`] to determine which ones are Blackboard's auto-generated `txt` files
/// (**datafiles**).
///
/// Returns a vector containing the indices of the datafiles in the archive. Use this with [`ZipArchive::by_index`] to
/// actually access the files. Indices are returned in place of filenames to avoid keeping a borrow on the `ZipArchive`,
/// which would prevent pulling files out (since accessing files requires a mutable reference).
///
/// # Approach
///
/// Every gradebook file contains a series of auto-generated `.txt` files that give metadata on each assignment. These
/// "datafiles" are our ticket to reliably determining information about the submissions without having to rely on
/// trying to parse that information out of the filenames, which would be messy and fragile.
///
/// Every filename in the gradebook looks like: `<dropbox>_<username>_attempt_<datetime>_<original-filename>`. The
/// datafiles can be identified because they lack the `<original-filename>` section, and instead end with just a `.txt`.
/// Attempting to find these files by regex-matching on `_attempt_yyyy-mm-dd-HH-MM-SS.txt$` is tempting, but
/// error-prone. While unlikely, a student _could_ technically submit a file that ends with `_attempt_<datetime>.txt`,
/// which would break the whole thing.
///
/// We want some other way to differentiate between Blackboard's datafiles and the things the students submit. Something
/// that relies on the inherent structure of the zip file. Thankfully, a solution becomes somewhat obvious when viewing
/// the list of files sorted alphabetically:
///
/// - `<dropbox>_<bob>_attempt_<timestamp>.txt`
/// - `<dropbox>_<bob>_attempt_<timestamp>_<filename>.<ext>`
/// - `<dropbox>_<alice>_attempt_<timestamp>.txt`
/// - `<dropbox>_<alice>_attempt_<timestamp>_<filename>.<ext>`
///
/// Key observation: no matter what the student names their file, it will have an underscore in its name in the same
/// position its associated datafile has a dot. Dots are `U+002E`; underscores are `U+005F`. So, if we sort the list of
/// filenames lexicographically (alphabetically), the dot will always be first! That means that, after sorting, the
/// first file in the list will always be a datafile. Additionally, all the files for each attempt will be right up
/// against one another, since they'll all share a common prefix. Then, to find the start of each subsequent submission,
/// we loop through the list of files, skipping them as long as they share a common prefix with the most recent
/// datafile.
fn find_datafiles<R: Read + Seek>(archive: &ZipArchive<R>) -> Result<Vec<usize>, GradebookError> {
    if archive.len() == 0 {
        return Err(GradebookError::empty());
    }

    let mut datafiles = Vec::new();

    // Get the list of all filenames and sort them
    let mut files = {
        let mut vec = (0..archive.len())
            .map(|index| (index, archive.name_for_index(index).expect("index is between 0 and len()")))
            .collect::<Vec<(usize, &str)>>();
        vec.sort_unstable_by(|(_, name1), (_, name2)| name1.as_bytes().cmp(name2.as_bytes()));
        vec.into_iter()
    };

    // Again, the first file is guaranteed to be a datafile... if this zip actually contains datafiles, that is!
    let (first_idx, first_df) = files.next().unwrap();
    if !is_valid_datafile_name(&first_df) {
        // If it doesn't start with a valid datafile, this probably isn't actually a gradebook.
        return Err(GradebookError::not_a_gradebook());
    }

    datafiles.push(first_idx);

    // Now loop through the list of files and look for a common prefix with the most recent datafile. Specifically, a
    // common prefix with a length equal to `prev.len() - ".txt".len()`. This is the longest common prefix there could
    // possibly be with a datafile. If the current file's prefix length is *shorter* than that, it means they differ
    // somewhere before the end of the `<timestamp>` section, and therefore belong to different submissions; `curr`
    // is therefore the datafile at the start of the next submission.
    let mut prev = first_df;
    while let Some((index, curr)) = files.next() {
        let prev_bytes = prev.as_bytes().into_iter();
        let curr_bytes = curr.as_bytes().into_iter();
        let prefix_len = prev_bytes.zip(curr_bytes).take_while(|(a, b)| a == b).count();
        if prefix_len < prev.len() - ".txt".len() {
            // New datafile! Just make sure it's actually a datafile...
            if is_valid_datafile_name(curr) {
                datafiles.push(index);
                prev = curr;
            } else {
                return Err(GradebookError::not_a_gradebook());
            }
        }
    }

    Ok(datafiles)
}

fn is_valid_datafile_name(name: &str) -> bool {
    // Every valid datafile name should look like this:
    //
    // ```
    // _attempt_YYYY-MM-DD-hh-mm-ss.txt
    // 0         1         2         3
    // 01234567890123456789012345678901
    // ```
    //
    // There are a bunch of magic numbers in this function: this diagram is their reference.
    // This is way faster than compiling and running an entire heap-allocated regex.
    if name.len() >= 32 {
        let end_string = &name[name.len() - 32..];
        let end_bytes = end_string.as_bytes();

        let is_number_in_range = |s: &str, min, max| s.parse::<usize>().is_ok_and(|n| min <= n && n <= max);

        &end_bytes[28..32] == b".txt"
            && &end_bytes[0..9] == b"_attempt_"
            && end_bytes[13] == b'-'
            && end_bytes[16] == b'-'
            && end_bytes[19] == b'-'
            && end_bytes[22] == b'-'
            && end_bytes[25] == b'-'
            && is_number_in_range(&end_string[09..13], 1, 9999) // Year
            && is_number_in_range(&end_string[14..16], 1, 12) // Month
            && is_number_in_range(&end_string[17..19], 1, 31) // Day
            && is_number_in_range(&end_string[20..22], 0, 23) // Hour
            && is_number_in_range(&end_string[23..25], 0, 59) // Minute
            && is_number_in_range(&end_string[26..28], 0, 59) // Second
    } else {
        false
    }
}
