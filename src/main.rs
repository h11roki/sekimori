use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use sekimori::report::{artifact_uri, plain, sarif_log, sarif_results};
use sekimori::rules::{Assessment, Upstreams, assess};
use sekimori::workflow::{Workflow, repository_path, workflow_paths};

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
    let mut workflows = Vec::new();
    let mut unreadable = Vec::new();
    for path in workflow_paths(&cli.paths) {
        let loaded = path
            .and_then(|path| Workflow::load(&path).inspect_err(|_| unreadable.push(path.clone())));
        match loaded {
            Ok(workflow) => workflows.push(workflow),
            Err(error) => {
                eprintln!("error: {error}");
                errors.push(error.to_string());
            }
        }
    }
    let upstreams = Upstreams::new(&workflows, unreadable.iter().map(PathBuf::as_path));
    for workflow in &workflows {
        let assessments = assess(workflow, &upstreams);
        let findings: Vec<_> = assessments.iter().flat_map(Assessment::findings).collect();
        if findings.is_empty() {
            continue;
        }
        found = true;
        match cli.format {
            Format::Plain => {
                for finding in &findings {
                    if let Err(error) = stdout.write_all(plain(workflow, finding).as_bytes()) {
                        return write_failure(&error);
                    }
                }
            }
            Format::Sarif => {
                let uri = artifact_uri(&repository_path(&workflow.path));
                results.extend(sarif_results(workflow, &uri, &findings));
            }
        }
    }
    if let Format::Sarif = cli.format {
        let log = sarif_log(results, &errors);
        let written = serde_json::to_vec_pretty(&log)
            .map_err(io::Error::from)
            .and_then(|mut document| {
                document.push(b'\n');
                stdout.write_all(&document)
            });
        if let Err(error) = written {
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
