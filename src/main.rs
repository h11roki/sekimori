use std::cell::LazyCell;
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
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
    sarif_file: Option<OsString>,
    #[arg(long, value_name = "PATH")]
    summary_file: Option<OsString>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let sarif_file = given_path(&cli.sarif_file);
    let summary_file = given_path(&cli.summary_file);
    let wants_sarif = matches!(cli.format, Format::Sarif) || sarif_file.is_some();
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
                if summary_file.is_some() {
                    plain_text.push_str(&format!("{line}\n"));
                }
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
                if summary_file.is_some() {
                    plain_text.push_str(&format!("{line}\n"));
                }
            }
        }
        let findings: Vec<_> = assessments.iter().flat_map(Assessment::findings).collect();
        if findings.is_empty() {
            continue;
        }
        found = true;
        for finding in &findings {
            let text = (matches!(cli.format, Format::Plain) || summary_file.is_some())
                .then(|| plain(workflow, finding));
            let written = match (cli.format, &text) {
                (Format::Plain, Some(text)) => stdout.write_all(text.as_bytes()),
                (Format::Github, _) => {
                    stdout.write_all(github(workflow, &path, finding).as_bytes())
                }
                _ => Ok(()),
            };
            if let Err(error) = written {
                return write_failure(&error);
            }
            if summary_file.is_some()
                && let Some(text) = text
            {
                plain_text.push_str(&text);
            }
        }
        if wants_sarif {
            results.extend(sarif_results(workflow, &artifact_uri(&path), &findings));
        }
    }
    let mut failure = None;
    if wants_sarif {
        let mut document = match serde_json::to_vec_pretty(&sarif_log(results, &errors)) {
            Ok(document) => document,
            Err(error) => return write_failure(&io::Error::from(error)),
        };
        document.push(b'\n');
        if let Format::Sarif = cli.format
            && let Err(error) = stdout.write_all(&document)
        {
            return write_failure(&error);
        }
        if let Some(file) = &sarif_file
            && let Err(error) = fs::write(file, &document)
        {
            failure = Some(file_failure(file, &error));
        }
    }
    if let Some(file) = &summary_file {
        let appended = fs::metadata(file)
            .map(|metadata| metadata.len())
            .or_else(|error| match error.kind() {
                io::ErrorKind::NotFound => Ok(0),
                _ => Err(error),
            })
            .and_then(|already_written| {
                let already_written = usize::try_from(already_written).unwrap_or(usize::MAX);
                match summary(&plain_text, already_written) {
                    Some(markdown) => OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(file)
                        .and_then(|mut opened| opened.write_all(markdown.as_bytes())),
                    None => Ok(()),
                }
            });
        if let Err(error) = appended {
            failure = Some(file_failure(file, &error));
        }
    }
    if let Some(failure) = failure {
        return failure;
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

fn file_failure(file: &Path, error: &io::Error) -> ExitCode {
    eprintln!("error: failed to write {}: {error}", file.display());
    ExitCode::from(EXIT_FAILURE)
}

fn given_path(value: &Option<OsString>) -> Option<PathBuf> {
    value
        .as_ref()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}
