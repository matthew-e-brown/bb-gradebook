mod cli;

use std::error::Error;
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::Path;
use std::process::ExitCode;

use bb_gradebook::Gradebook;

fn main() -> ExitCode {
    let args = cli::Args::parse();

    println!("{:#?}", args);

    let gradebook = open_gradebook(args.path()).unwrap();
    println!("{:#?}", gradebook);

    ExitCode::SUCCESS
}


fn open_gradebook(path: &Path) -> Result<Gradebook<impl Read + Seek>, Box<dyn Error>> {
    let file = BufReader::new(File::open(path)?);
    let book = Gradebook::from_reader(file)?;
    Ok(book)
}
