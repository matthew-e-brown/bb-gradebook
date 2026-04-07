mod cli;

use std::error::Error;
use std::fmt::Debug;
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::Path;
use std::process::ExitCode;

use bb_gradebook::GradebookArchive;

fn main() -> ExitCode {
    let args = cli::Args::parse();

    println!("{:#?}", args);

    let gradebook = open_gradebook(args.path()).unwrap();
    println!("{:#?}", gradebook);

    ExitCode::SUCCESS
}


fn open_gradebook(path: &Path) -> Result<GradebookArchive<impl Read + Seek + Debug>, Box<dyn Error>> {
    let file = BufReader::new(File::open(path)?);
    let book = GradebookArchive::from_reader(file)?;
    Ok(book)
}
