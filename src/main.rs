use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use sekimori::report::plain;
use sekimori::rules::{Assessment, assess};
use sekimori::workflow::{Workflow, workflow_paths};

const EXIT_FINDINGS: u8 = 1;
const EXIT_FAILURE: u8 = 2;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[arg(default_value = ".github/workflows")]
    paths: Vec<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut stdout = io::stdout().lock();
    let mut failed = false;
    let mut found = false;
    for path in workflow_paths(&cli.paths) {
        let workflow = match path.and_then(|path| Workflow::load(&path)) {
            Ok(workflow) => workflow,
            Err(error) => {
                eprintln!("error: {error}");
                failed = true;
                continue;
            }
        };
        let assessments = assess(&workflow);
        for finding in assessments.iter().flat_map(Assessment::findings) {
            found = true;
            if let Err(error) = stdout.write_all(plain(&workflow, &finding).as_bytes()) {
                eprintln!("error: failed to write findings: {error}");
                return ExitCode::from(EXIT_FAILURE);
            }
        }
    }
    if failed {
        ExitCode::from(EXIT_FAILURE)
    } else if found {
        ExitCode::from(EXIT_FINDINGS)
    } else {
        ExitCode::SUCCESS
    }
}
