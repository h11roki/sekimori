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
    let mut failed = false;
    for path in workflow_paths(&cli.paths) {
        if let Err(error) = path.and_then(|path| Workflow::load(&path)) {
            eprintln!("error: {error}");
            failed = true;
        }
    }
    if failed {
        ExitCode::from(EXIT_LOAD_FAILURE)
    } else {
        ExitCode::SUCCESS
    }
}
