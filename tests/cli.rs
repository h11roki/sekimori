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
