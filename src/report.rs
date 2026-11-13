use std::env;
use std::path::{Component, Path};

use serde_json::{Value, json};

use crate::rules::{Finding, HALVES_PER_POINT, MAX_TOTAL, Rule};
use crate::workflow::Workflow;

const LABEL_WIDTH: usize = 21;
const VALUE_WIDTH: usize = 5;
const SARIF_VERSION: &str = "2.1.0";
const SARIF_SCHEMA: &str = "https://json.schemastore.org/sarif-2.1.0.json";
const SARIF_TAGS: [&str; 1] = ["security"];
const FILE_URI_SCHEME: &str = "file://";

pub fn plain(workflow: &Workflow, finding: &Finding) -> String {
    let (line, column) = position(workflow, finding);
    format!(
        "{}:{line}:{column}: {}[{}]: {}\n{}",
        workflow.path.display(),
        finding.level,
        finding.hit.rule.id(),
        headline(finding),
        axes(finding),
    )
}

pub fn sarif_result(workflow: &Workflow, finding: &Finding) -> Value {
    let (line, column) = position(workflow, finding);
    let rule = finding.hit.rule;
    json!({
        "ruleId": rule.id(),
        "ruleIndex": Rule::ALL.iter().position(|known| *known == rule),
        "level": finding.level.to_string(),
        "message": { "text": format!("{}\n{}", headline(finding), axes(finding)) },
        "locations": [{
            "physicalLocation": {
                "artifactLocation": { "uri": artifact_uri(&workflow.path) },
                "region": { "startLine": line, "startColumn": column },
            },
        }],
    })
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
            "invocations": [{
                "executionSuccessful": errors.is_empty(),
                "toolExecutionNotifications": notifications,
            }],
            "results": results,
        }],
    })
}

fn position(workflow: &Workflow, finding: &Finding) -> (usize, usize) {
    workflow.position(&finding.hit.route).unwrap_or((1, 1))
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

fn axes(finding: &Finding) -> String {
    let assessment = finding.assessment;
    let axes = [
        ("who can trigger", &assessment.who_can_trigger),
        ("allowed tools", &assessment.allowed_tools),
        ("permissions/secrets", &assessment.permissions_and_secrets),
    ];
    let mut text = String::new();
    for (label, score) in axes {
        let row = match score {
            Some(score) => format!(
                "  {label:<LABEL_WIDTH$}{:<VALUE_WIDTH$}{}\n",
                halves_text(score.weight.halves()),
                score.reason
            ),
            None => format!("  {label:<LABEL_WIDTH$}0\n"),
        };
        text.push_str(&row);
    }
    text.push_str(&format!(
        "  {:<LABEL_WIDTH$}{} of {MAX_TOTAL}\n",
        "total",
        halves_text(assessment.total_halves())
    ));
    text
}

fn artifact_uri(path: &Path) -> String {
    let relative = if path.is_absolute() {
        env::current_dir()
            .ok()
            .and_then(|directory| path.strip_prefix(directory).ok())
    } else {
        Some(path)
    };
    match relative {
        Some(relative) => {
            let mut parts: Vec<String> = Vec::new();
            for component in relative.components() {
                match component {
                    Component::CurDir => {}
                    Component::ParentDir if parts.last().is_some_and(|last| last != "..") => {
                        parts.pop();
                    }
                    component => parts.push(component.as_os_str().to_string_lossy().into_owned()),
                }
            }
            parts.join("/")
        }
        None => format!(
            "{FILE_URI_SCHEME}{}",
            path.to_string_lossy().replace('\\', "/")
        ),
    }
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
    use crate::rules::assess;

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
            Path::new("./.github/workflows/../workflows/claude.yml"),
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
        let result = sarif_result(&workflow, &findings[0]);
        assert_eq!(result["ruleId"], "unrestricted-trigger");
        assert_eq!(result["ruleIndex"], 0);
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
            plain(&workflow, &findings[0])
                .split_once(": warning[unrestricted-trigger]: ")
                .unwrap()
                .1
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
        assert_eq!(ids, Rule::ALL.map(Rule::id));
        assert_eq!(run["invocations"][0]["executionSuccessful"], false);
        assert_eq!(
            run["invocations"][0]["toolExecutionNotifications"][0]["message"]["text"],
            "broken.yml is not a valid workflow"
        );
        assert_eq!(run["results"].as_array().unwrap().len(), 0);
    }
}
