mod cli;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let args = cli::App::parse();

    println!("{:#?}", args);

    ExitCode::SUCCESS
}
