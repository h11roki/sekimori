use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use sekimori::agent::agent_steps;
use sekimori::report::plain;
use sekimori::rules::{Assessment, assess};
use sekimori::workflow::{Workflow, workflow_paths};

const EXIT_FINDINGS: u8 = 1;
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
        let assessments: Vec<Assessment> = agent_steps(&workflow)
            .into_iter()
            .map(|step| assess(&workflow, step))
            .collect();
        for finding in assessments.iter().flat_map(Assessment::findings) {
            print!("{}", plain(&workflow, &finding));
            found = true;
        }
    }
    if failed {
        ExitCode::from(EXIT_LOAD_FAILURE)
    } else if found {
        ExitCode::from(EXIT_FINDINGS)
    } else {
        ExitCode::SUCCESS
    }
}
