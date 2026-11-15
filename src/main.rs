use std::cell::LazyCell;
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use sekimori::facts::Upstreams;
use sekimori::report::{
    artifact_uri, github, github_error, github_warning, plain, sarif_log, sarif_results, summary,
    warning,
};
use sekimori::rules::{Assessment, assess};
use sekimori::workflow::{Workflow, repository_path, workflow_paths};

const EXIT_FINDINGS: u8 = 1;
const EXIT_FAILURE: u8 = 2;

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Plain,
    Sarif,
    Github,
}

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[arg(default_value = ".github/workflows")]
    paths: Vec<PathBuf>,
    #[arg(long, value_enum, default_value_t = Format::Plain)]
    format: Format,
    #[arg(long, value_name = "PATH")]
    sarif_file: Option<PathBuf>,
    #[arg(long, value_name = "PATH")]
    summary_file: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let wants_sarif = matches!(cli.format, Format::Sarif) || cli.sarif_file.is_some();
    let mut stdout = io::stdout().lock();
    let mut errors = Vec::new();
    let mut results = Vec::new();
    let mut plain_text = String::new();
    let mut found = false;
    let mut workflows = Vec::new();
    let mut unreadable = Vec::new();
    for path in workflow_paths(&cli.paths) {
        let loaded = path
            .and_then(|path| Workflow::load(&path).inspect_err(|_| unreadable.push(path.clone())));
        match loaded {
            Ok(workflow) => workflows.push(workflow),
            Err(error) => {
                let line = format!("error: {error}");
                match cli.format {
                    Format::Github => {
                        if let Err(error) = stdout.write_all(
                            github_error(
                                error.workflow_path().map(repository_path).as_ref(),
                                &error.to_string(),
                            )
                            .as_bytes(),
                        ) {
                            return write_failure(&error);
                        }
                    }
                    Format::Plain | Format::Sarif => eprintln!("{line}"),
                }
                plain_text.push_str(&line);
                plain_text.push('\n');
                errors.push(error.to_string());
            }
        }
    }
    let upstreams = Upstreams::new(&workflows, unreadable.iter().map(PathBuf::as_path));
    for workflow in &workflows {
        let assessments = assess(workflow, &upstreams);
        let path = LazyCell::new(|| repository_path(&workflow.path));
        let mut warned = HashSet::new();
        for assessment in &assessments {
            for message in assessment.warnings() {
                let route = assessment.directive_route();
                let line = warning(workflow, &route, message);
                if !warned.insert(line.clone()) {
                    continue;
                }
                match cli.format {
                    Format::Plain => eprintln!("{line}"),
                    Format::Github => {
                        if let Err(error) = stdout
                            .write_all(github_warning(workflow, &path, &route, message).as_bytes())
                        {
                            return write_failure(&error);
                        }
                    }
                    Format::Sarif => {}
                }
                plain_text.push_str(&line);
                plain_text.push('\n');
            }
        }
        let findings: Vec<_> = assessments.iter().flat_map(Assessment::findings).collect();
        if findings.is_empty() {
            continue;
        }
        found = true;
        for finding in &findings {
            let text = plain(workflow, finding);
            let written = match cli.format {
                Format::Plain => stdout.write_all(text.as_bytes()),
                Format::Github => stdout.write_all(github(workflow, &path, finding).as_bytes()),
                Format::Sarif => Ok(()),
            };
            if let Err(error) = written {
                return write_failure(&error);
            }
            plain_text.push_str(&text);
        }
        if wants_sarif {
            results.extend(sarif_results(workflow, &artifact_uri(&path), &findings));
        }
    }
    if wants_sarif {
        let document = serde_json::to_vec_pretty(&sarif_log(results, &errors))
            .map(|mut document| {
                document.push(b'\n');
                document
            })
            .map_err(io::Error::from);
        let written = document.and_then(|document| {
            if let Format::Sarif = cli.format {
                stdout.write_all(&document)?;
            }
            match &cli.sarif_file {
                Some(file) => fs::write(file, &document),
                None => Ok(()),
            }
        });
        if let Err(error) = written {
            return write_failure(&error);
        }
    }
    if let Some(file) = &cli.summary_file
        && let Some(markdown) = summary(&plain_text)
    {
        let appended = OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
            .and_then(|mut opened| opened.write_all(markdown.as_bytes()));
        if let Err(error) = appended {
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
