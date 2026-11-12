use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use sekimori::workflow::{Workflow, workflow_paths};

const EXIT_LOAD_FAILURE: u8 = 2;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[arg(default_value = ".github/workflows")]
    paths: Vec<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut checked = HashSet::new();
    let mut failed = false;
    for input in &cli.paths {
        let paths = match workflow_paths(input) {
            Ok(paths) => paths,
            Err(error) => {
                eprintln!("error: {error}");
                failed = true;
                continue;
            }
        };
        for path in paths {
            let identity = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if !checked.insert(identity) {
                continue;
            }
            if let Err(error) = Workflow::load(&path) {
                eprintln!("error: {error}");
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::from(EXIT_LOAD_FAILURE)
    } else {
        ExitCode::SUCCESS
    }
}
