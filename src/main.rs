use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use sekimori::report::{plain, sarif_log, sarif_result};
use sekimori::rules::{Assessment, assess};
use sekimori::workflow::{Workflow, workflow_paths};

const EXIT_FINDINGS: u8 = 1;
const EXIT_FAILURE: u8 = 2;

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Plain,
    Sarif,
}

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[arg(default_value = ".github/workflows")]
    paths: Vec<PathBuf>,
    #[arg(long, value_enum, default_value_t = Format::Plain)]
    format: Format,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut stdout = io::stdout().lock();
    let mut errors = Vec::new();
    let mut results = Vec::new();
    let mut found = false;
    for path in workflow_paths(&cli.paths) {
        let workflow = match path.and_then(|path| Workflow::load(&path)) {
            Ok(workflow) => workflow,
            Err(error) => {
                eprintln!("error: {error}");
                errors.push(error.to_string());
                continue;
            }
        };
        let assessments = assess(&workflow);
        for finding in assessments.iter().flat_map(Assessment::findings) {
            found = true;
            match cli.format {
                Format::Plain => {
                    if let Err(error) = stdout.write_all(plain(&workflow, &finding).as_bytes()) {
                        return write_failure(&error);
                    }
                }
                Format::Sarif => results.push(sarif_result(&workflow, &finding)),
            }
        }
    }
    if let Format::Sarif = cli.format {
        let log = sarif_log(results, &errors);
        if let Err(error) = serde_json::to_writer_pretty(&mut stdout, &log)
            .map_err(io::Error::from)
            .and_then(|()| writeln!(stdout))
        {
            return write_failure(&error);
        }
    }
    if !errors.is_empty() {
        ExitCode::from(EXIT_FAILURE)
    } else if found {
        ExitCode::from(EXIT_FINDINGS)
    } else {
        ExitCode::SUCCESS
    }
}

fn write_failure(error: &io::Error) -> ExitCode {
    eprintln!("error: failed to write findings: {error}");
    ExitCode::from(EXIT_FAILURE)
}
