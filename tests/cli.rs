use std::path::Path;
use std::process::{Command, Output};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/workflows");

fn sekimori(paths: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sekimori"))
        .args(paths.iter().map(|path| Path::new(FIXTURES).join(path)))
        .output()
        .unwrap()
}

#[test]
fn readable_workflows_exit_with_zero() {
    let output = sekimori(&["ci.yml", "release.yaml"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
}

#[test]
fn an_unreadable_workflow_is_reported_and_exits_with_two() {
    let output = sekimori(&["broken.yml", "missing.yml", "ci.yml"]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("broken.yml is not a valid workflow"));
    assert!(stderr.contains("failed to read"));
    assert!(stderr.contains("missing.yml"));
    assert_eq!(stderr.lines().count(), 2);
}

#[test]
fn a_directory_is_checked_file_by_file() {
    let output = sekimori(&["."]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(stderr.lines().count(), 1);
    assert!(stderr.contains("broken.yml"));
}

#[test]
fn a_file_reached_twice_is_checked_once() {
    let output = sekimori(&["broken.yml", ".", "nested/../broken.yml"]);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(stderr.lines().count(), 1);
}

#[test]
fn errors_follow_the_order_of_the_arguments() {
    let output = sekimori(&["broken.yml", "../no-workflows"]);
    let stderr = String::from_utf8(output.stderr).unwrap();
    let lines: Vec<_> = stderr.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("broken.yml"));
    assert!(lines[1].contains("no-workflows"));
}

#[test]
fn a_finding_is_printed_to_stdout_and_exits_with_one() {
    let output = sekimori(&["../findings/open-claude.yml"]);
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("open-claude.yml:9:11: warning[unrestricted-trigger]"));
    assert!(output.stderr.is_empty());
}

#[test]
fn an_unreadable_workflow_wins_over_findings_and_exits_with_two() {
    let output = sekimori(&["../findings/open-claude.yml", "broken.yml"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(!output.stdout.is_empty());
}

#[test]
fn a_directory_without_workflows_exits_with_two() {
    let output = sekimori(&["../no-workflows"]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("no .yml or .yaml files directly inside"));
}
