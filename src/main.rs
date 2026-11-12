use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use sekimori::workflow::{LoadError, Workflow, workflow_paths};

const EXIT_LOAD_FAILURE: u8 = 2;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[arg(default_value = ".github/workflows")]
    paths: Vec<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut failures: Vec<LoadError> = Vec::new();
    for input in &cli.paths {
        match workflow_paths(input) {
            Ok(paths) => {
                failures.extend(paths.iter().filter_map(|path| Workflow::load(path).err()))
            }
            Err(error) => failures.push(error),
        }
    }
    for failure in &failures {
        eprintln!("error: {failure}");
    }
    if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_LOAD_FAILURE)
    }
}
