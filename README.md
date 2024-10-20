# Blackboard submission extractor

This is a simple command-line program to extract student-submitted files from
archives downloaded from the _Blackboard Learn_ learning management software.

When downloading all submissions for an assignment, Blackboard provides them all
in a single flat `.zip` file, and renames every single file. This renaming can
be quite irritating, especially when dealing with programming assignments, where
assignment files may reference each other by name or by path.

This program takes care of all the busywork that would otherwise be necessary to
re-organize student submissions for local marking.


## Features

- Extracts files into an organized folder structure:
  - A top-level folder is created for the assignment itself
  - Each student is given a subfolder containing their files
  - When multiple attempts are present, 
  - Files are renamed according to the files' original names from the student
- Zip files submitted by students are automatically extracted into folders.
- Any comments or text submissions left by the student are extracted into their
  own text files, if present.


## Where to get the zip file

To download a zip file for organized extraction:

1.  Go into the Grade Center on Blackboard
2.  On the column for the assignment you wish to download submissions for, click
    the little down arrow, then select _Assignment File Download._
3.  Scroll to the bottom of the list and click _Show All._
4.  Select every submission by clicking the checkbox at the top of the list.
5.  Select the _All attempt files_ radio button.
6.  Click on _Submit_ and wait.

Obviously you don't need to include _every_ submission, if you want to leave out
earlier attempts that won't be included in grading. But personally, I like to
download everything at once so that it's readily available if it ever comes up
(oftentimes students may include a file in their first attempt, but forget to
include it in subsequent attempts).


## Roadmap

Things I'd like to add in the future are:

- Support for auto-extracting more archive types: `.7z`, `.tar.gz`, `.rar`, etc.
  Zip files were trivial to support because the gradebooks themselves are zips,
  so the `zip` library was already included. Supporting other archives will
  require bringing in their respective crates. Not too hard, just another thing
  to get around to some day.
- Maybe writing a GitHub action to create pre-built binaries for various
  platforms. As much as I wrote this for my own convenience, and I'm only
  putting it on GitHub to share with a few fellow comp-sci markers (who no doubt
  will be able to compile it for themselves), it might be nice to be able to
  provide Windows, Linux, and Apple Silicon binaries so that other, less techy
  educators can make use of this application without having to install the Rust
  toolchain. Just on the off-chance that it may be helpful to someone else :)
