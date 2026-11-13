use serde_json::{Value, json};

use crate::rules::{Finding, HALVES_PER_POINT, Level, MAX_TOTAL, Rule};
use crate::workflow::{RepositoryPath, Workflow};

const LABEL_WIDTH: usize = 21;
const VALUE_WIDTH: usize = 5;
const SARIF_VERSION: &str = "2.1.0";
const SARIF_SCHEMA: &str = "https://json.schemastore.org/sarif-2.1.0.json";
const SARIF_TAGS: [&str; 1] = ["security"];
const FILE_SCHEME: &str = "file:";
const FILE_URI_SCHEME: &str = "file://";
const UNC_URI_PREFIX: &str = "//";
const DRIVE_SEPARATOR: &str = ":";
const COLUMN_KIND: &str = "utf16CodeUnits";
const URI_UNRESERVED: &str = "-._~/";

pub fn plain(workflow: &Workflow, finding: &Finding) -> String {
    let (line, column) = workflow.position(&finding.hit.route).unwrap_or((1, 1));
    format!(
        "{}:{line}:{column}: {}[{}]: {}\n{}",
        workflow.path.display(),
        finding.level,
        finding.hit.rule.id(),
        headline(finding),
        axis_rows(finding)
            .into_iter()
            .map(|(label, value, reason)| match reason {
                Some(reason) => format!("  {label:<LABEL_WIDTH$}{value:<VALUE_WIDTH$}{reason}\n"),
                None => format!("  {label:<LABEL_WIDTH$}{value}\n"),
            })
            .collect::<String>(),
    )
}

struct SarifPlace {
    rule: Rule,
    position: (usize, usize),
    level: Level,
    breakdowns: Vec<String>,
}

pub fn sarif_results(workflow: &Workflow, uri: &str, findings: &[Finding]) -> Vec<Value> {
    let mut places: Vec<SarifPlace> = Vec::new();
    for finding in findings {
        let position = workflow.position(&finding.hit.route).unwrap_or((1, 1));
        let breakdown = sarif_breakdown(finding);
        match places
            .iter_mut()
            .find(|place| place.rule == finding.hit.rule && place.position == position)
        {
            Some(place) => {
                place.level = place.level.max(finding.level);
                place.breakdowns.push(breakdown);
            }
            None => places.push(SarifPlace {
                rule: finding.hit.rule,
                position,
                level: finding.level,
                breakdowns: vec![breakdown],
            }),
        }
    }
    places
        .into_iter()
        .map(|place| {
            let (line, column) = place.position;
            json!({
                "ruleId": place.rule.id(),
                "level": place.level.to_string(),
                "message": { "text": place.breakdowns.join("\n\n") },
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": { "uri": uri },
                        "region": { "startLine": line, "startColumn": column },
                    },
                }],
            })
        })
        .collect()
}

fn sarif_breakdown(finding: &Finding) -> String {
    let rows: Vec<String> = axis_rows(finding)
        .into_iter()
        .map(|(label, value, reason)| match reason {
            Some(reason) => format!("{label}: {value} ({reason})"),
            None => format!("{label}: {value}"),
        })
        .collect();
    format!("{}\n{}", headline(finding), rows.join("\n"))
}

pub fn sarif_log(results: Vec<Value>, errors: &[String]) -> Value {
    let rules: Vec<Value> = Rule::ALL
        .iter()
        .map(|rule| {
            json!({
                "id": rule.id(),
                "shortDescription": { "text": rule.summary() },
                "fullDescription": { "text": rule.description() },
                "help": { "text": rule.description() },
                "properties": { "tags": SARIF_TAGS },
            })
        })
        .collect();
    let notifications: Vec<Value> = errors
        .iter()
        .map(|error| json!({ "level": "error", "message": { "text": error } }))
        .collect();
    json!({
        "$schema": SARIF_SCHEMA,
        "version": SARIF_VERSION,
        "runs": [{
            "tool": {
                "driver": {
                    "name": env!("CARGO_PKG_NAME"),
                    "version": env!("CARGO_PKG_VERSION"),
                    "semanticVersion": env!("CARGO_PKG_VERSION"),
                    "informationUri": env!("CARGO_PKG_REPOSITORY"),
                    "rules": rules,
                },
            },
            "columnKind": COLUMN_KIND,
            "invocations": [{
                "executionSuccessful": errors.is_empty(),
                "toolExecutionNotifications": notifications,
            }],
            "results": results,
        }],
    })
}

fn headline(finding: &Finding) -> String {
    let assessment = finding.assessment;
    format!(
        "{} in job `{}` {}",
        assessment.step.step.uses.raw(),
        assessment.step.job_id,
        finding.hit.rule.summary(),
    )
}

fn axis_rows(finding: &Finding) -> Vec<(&'static str, String, Option<String>)> {
    let assessment = finding.assessment;
    let mut rows: Vec<_> = [
        ("who can trigger", &assessment.who_can_trigger),
        ("allowed tools", &assessment.allowed_tools),
        ("permissions/secrets", &assessment.permissions_and_secrets),
    ]
    .into_iter()
    .map(|(label, score)| match score {
        Some(score) => (
            label,
            halves_text(score.weight.halves()),
            Some(score.reason.clone()),
        ),
        None => (label, "0".to_string(), None),
    })
    .collect();
    rows.push((
        "total",
        format!("{} of {MAX_TOTAL}", halves_text(assessment.total_halves())),
        None,
    ));
    rows
}

pub fn artifact_uri(path: &RepositoryPath) -> String {
    match path {
        RepositoryPath::Relative(relative) => percent_encoded(&uri_path(relative), ""),
        RepositoryPath::Absolute(absolute) => {
            let path = percent_encoded(&uri_path(absolute), DRIVE_SEPARATOR);
            if path.starts_with(UNC_URI_PREFIX) {
                format!("{FILE_SCHEME}{path}")
            } else if path.starts_with('/') {
                format!("{FILE_URI_SCHEME}{path}")
            } else {
                format!("{FILE_URI_SCHEME}/{path}")
            }
        }
    }
}

fn uri_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn percent_encoded(text: &str, also_kept: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric()
                || URI_UNRESERVED.as_bytes().contains(&byte)
                || also_kept.as_bytes().contains(&byte)
            {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

fn halves_text(halves: u8) -> String {
    let points = halves / HALVES_PER_POINT;
    if halves.is_multiple_of(HALVES_PER_POINT) {
        points.to_string()
    } else {
        format!("{points}.5")
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::rules::{Assessment, assess};

    #[test]
    fn a_finding_names_its_place_rule_step_and_every_axis() {
        let workflow = Workflow::parse(
            Path::new(".github/workflows/claude.yml"),
            "on: issue_comment
jobs:
  claude:
    runs-on: ubuntu-latest
    steps:
      - uses: anthropics/claude-code-action@v1
        with:
          github_token: ${{ secrets.GITHUB_TOKEN }}
          allowed_non_write_users: '*'
"
            .to_string(),
        )
        .unwrap();
        let assessments = assess(&workflow);
        let findings = assessments[0].findings();
        assert_eq!(findings.len(), 1);
        assert_eq!(
            plain(&workflow, &findings[0]),
            "\
.github/workflows/claude.yml:9:11: warning[unrestricted-trigger]: anthropics/claude-code-action@v1 in job `claude` can be started without write access
  who can trigger      1    allowed_non_write_users: '*', on issue_comment
  allowed tools        0
  permissions/secrets  1    no permissions are set, so GITHUB_TOKEN gets the repository default
  total                2 of 3
"
        );
    }

    #[test]
    fn halves_are_shown_in_steps_of_one_half() {
        assert_eq!(
            [0, 1, 2, 5, 6].map(halves_text),
            ["0", "0.5", "1", "2.5", "3"]
        );
    }

    #[test]
    fn sarif_results_carry_the_rule_level_place_and_axes() {
        let workflow = Workflow::parse(
            Path::new("claude.yml"),
            "on: issue_comment
jobs:
  claude:
    runs-on: ubuntu-latest
    steps:
      - uses: anthropics/claude-code-action@v1
        with:
          github_token: ${{ secrets.GITHUB_TOKEN }}
          allowed_non_write_users: '*'
"
            .to_string(),
        )
        .unwrap();
        let assessments = assess(&workflow);
        let findings = assessments[0].findings();
        let results = sarif_results(&workflow, ".github/workflows/claude.yml", &findings);
        assert_eq!(results.len(), 1);
        let result = &results[0];
        assert_eq!(result["ruleId"], "unrestricted-trigger");
        assert_eq!(result["level"], "warning");
        let location = &result["locations"][0]["physicalLocation"];
        assert_eq!(
            location["artifactLocation"]["uri"],
            ".github/workflows/claude.yml"
        );
        assert_eq!(location["region"]["startLine"], 9);
        assert_eq!(location["region"]["startColumn"], 11);
        assert_eq!(
            result["message"]["text"],
            "anthropics/claude-code-action@v1 in job `claude` can be started without write access
who can trigger: 1 (allowed_non_write_users: '*', on issue_comment)
allowed tools: 0
permissions/secrets: 1 (no permissions are set, so GITHUB_TOKEN gets the repository default)
total: 2 of 3"
        );
    }

    #[test]
    fn sarif_results_at_the_same_place_merge_into_the_heaviest_level() {
        let workflow = Workflow::parse(
            Path::new("codex.yml"),
            "on: issue_comment
env:
  GITHUB_TOKEN: ${{ secrets.BOT_TOKEN }}
jobs:
  codex:
    runs-on: ubuntu-latest
    steps:
      - uses: openai/codex-action@v1
        with:
          safety-strategy: unsafe
      - uses: openai/codex-action@v1
        with:
          allow-users: '*'
          safety-strategy: unsafe
"
            .to_string(),
        )
        .unwrap();
        let assessments = assess(&workflow);
        let findings: Vec<Finding> = assessments.iter().flat_map(Assessment::findings).collect();
        assert_eq!(findings.len(), 5);
        let results = sarif_results(&workflow, "codex.yml", &findings);
        let summary: Vec<(&str, &str, u64)> = results
            .iter()
            .map(|result| {
                (
                    result["ruleId"].as_str().unwrap(),
                    result["level"].as_str().unwrap(),
                    result["locations"][0]["physicalLocation"]["region"]["startLine"]
                        .as_u64()
                        .unwrap(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("broad-tools", "warning", 10),
                ("long-lived-token", "error", 3),
                ("unrestricted-trigger", "error", 13),
                ("broad-tools", "error", 14),
            ]
        );
        let merged = results[1]["message"]["text"].as_str().unwrap();
        let breakdowns: Vec<&str> = merged.split("\n\n").collect();
        assert_eq!(breakdowns.len(), 2);
        assert!(breakdowns[0].ends_with("total: 2.5 of 3"));
        assert!(breakdowns[1].ends_with("total: 3 of 3"));
    }

    #[test]
    fn artifact_uris_are_encoded_and_absolute_paths_become_file_uris() {
        assert_eq!(
            artifact_uri(&RepositoryPath::Relative(
                ".github/workflows/a b#1.yml".into()
            )),
            ".github/workflows/a%20b%231.yml"
        );
        assert_eq!(
            artifact_uri(&RepositoryPath::Absolute("/tmp/wf/a.yml".into())),
            "file:///tmp/wf/a.yml"
        );
        assert_eq!(
            artifact_uri(&RepositoryPath::Absolute(r"C:\wf\a.yml".into())),
            "file:///C:/wf/a.yml"
        );
        assert_eq!(
            artifact_uri(&RepositoryPath::Absolute(r"\\server\share\a.yml".into())),
            "file://server/share/a.yml"
        );
    }

    #[test]
    fn the_sarif_log_lists_every_rule_and_reports_load_errors() {
        let log = sarif_log(
            Vec::new(),
            &["broken.yml is not a valid workflow".to_string()],
        );
        assert_eq!(log["version"], "2.1.0");
        let run = &log["runs"][0];
        let ids: Vec<_> = run["tool"]["driver"]["rules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|rule| rule["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            [
                "unrestricted-trigger",
                "broad-tools",
                "long-lived-token",
                "pr-head-checkout",
                "other-secrets"
            ]
        );
        assert_eq!(run["invocations"][0]["executionSuccessful"], false);
        assert_eq!(
            run["invocations"][0]["toolExecutionNotifications"][0]["message"]["text"],
            "broken.yml is not a valid workflow"
        );
        assert_eq!(run["results"].as_array().unwrap().len(), 0);
    }
}
