//! Implementation of Gradebook parsing routines.
//!
//! The main entrypoint of this module is the [`parse_gradebook`] function.

mod datafile;
mod gradebook;

use std::io::{Read, Seek};

use self::gradebook::GradebookParser;
use crate::error::GradebookLoadError;
use crate::{GradebookInfo, ZipArchive};

/// Attempts to parse the contents of a [`ZipArchive`] into a [`GradebookInfo`].
pub fn parse_gradebook<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<GradebookInfo, GradebookLoadError> {
    GradebookParser::new().parse(archive)
}
