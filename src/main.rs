use std::cell::LazyCell;
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
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
    sarif_file: Option<OsString>,
    #[arg(long, value_name = "PATH")]
    summary_file: Option<OsString>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let sarif_file = given_path(cli.sarif_file.as_deref());
    let summary_file = given_path(cli.summary_file.as_deref());
    let wants_sarif = matches!(cli.format, Format::Sarif) || sarif_file.is_some();
    let mut stdout = io::stdout().lock();
    let mut errors = Vec::new();
    let mut results = Vec::new();
    let mut summary_text = summary_file.as_ref().map(|_| String::new());
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
                if let Some(text) = &mut summary_text {
                    text.push_str(&line);
                    text.push('\n');
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
                if let Some(text) = &mut summary_text {
                    text.push_str(&line);
                    text.push('\n');
                }
            }
        }
        let findings: Vec<_> = assessments.iter().flat_map(Assessment::findings).collect();
        if findings.is_empty() {
            continue;
        }
        found = true;
        for finding in &findings {
            let written = match cli.format {
                Format::Plain => stdout.write_all(plain(workflow, finding).as_bytes()),
                Format::Github => stdout.write_all(github(workflow, &path, finding).as_bytes()),
                Format::Sarif => Ok(()),
            };
            if let Err(error) = written {
                return write_failure(&error);
            }
            if let Some(text) = &mut summary_text {
                text.push_str(&plain(workflow, finding));
            }
        }
        if wants_sarif {
            results.extend(sarif_results(workflow, &artifact_uri(&path), &findings));
        }
    }
    let mut failed = false;
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
        if let Some(file) = &sarif_file {
            let mut messages = Vec::new();
            match File::create(file) {
                Err(error) => messages.push(format!("failed to write {}: {error}", file.display())),
                Ok(mut opened) => {
                    let written = opened.write_all(&document).and_then(|()| {
                        match opened.metadata()?.is_file() {
                            true => opened.sync_all(),
                            false => Ok(()),
                        }
                    });
                    if let Err(error) = written {
                        messages.push(format!("failed to write {}: {error}", file.display()));
                        if file.is_file()
                            && let Err(error) = fs::remove_file(file)
                        {
                            messages.push(format!(
                                "failed to remove the partly written {}: {error}",
                                file.display()
                            ));
                        }
                    }
                }
            }
            for message in messages {
                failed = true;
                report_failure(&mut stdout, cli.format, &message);
                if let Some(text) = &mut summary_text {
                    text.insert_str(0, &format!("error: {message}\n"));
                }
            }
        }
    }
    if let (Some(file), Some(text)) = (&summary_file, &summary_text) {
        let appended = fs::metadata(file)
            .map(|metadata| metadata.len())
            .or_else(|error| match error.kind() {
                io::ErrorKind::NotFound => Ok(0),
                _ => Err(error),
            })
            .and_then(|already_written| {
                let already_written = usize::try_from(already_written).unwrap_or(usize::MAX);
                match summary(text, already_written) {
                    Some(markdown) => OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(file)
                        .and_then(|mut opened| opened.write_all(markdown.as_bytes())),
                    None => Ok(()),
                }
            });
        if let Err(error) = appended {
            failed = true;
            report_failure(
                &mut stdout,
                cli.format,
                &format!("failed to write {}: {error}", file.display()),
            );
        }
    }
    if failed || !errors.is_empty() {
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

fn report_failure(stdout: &mut impl Write, format: Format, message: &str) {
    if let Format::Github = format {
        match stdout.write_all(github_error(None, message).as_bytes()) {
            Ok(()) => return,
            Err(error) => eprintln!("error: failed to write an annotation: {error}"),
        }
    }
    eprintln!("error: {message}");
}

fn given_path(value: Option<&OsStr>) -> Option<PathBuf> {
    value.filter(|value| !value.is_empty()).map(PathBuf::from)
}
