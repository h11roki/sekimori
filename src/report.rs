use crate::rules::{Finding, MAX_TOTAL};
use crate::workflow::Workflow;

const LABEL_WIDTH: usize = 21;
const VALUE_WIDTH: usize = 5;

pub fn plain(workflow: &Workflow, finding: &Finding) -> String {
    let (line, column) = workflow.position(&finding.hit.route).unwrap_or((1, 1));
    let assessment = finding.assessment;
    let mut text = format!(
        "{}:{line}:{column}: {}[{}]: {} in job `{}` {}\n",
        workflow.path.display(),
        finding.level,
        finding.hit.rule.id(),
        assessment.step.step.uses.raw(),
        assessment.step.job_id,
        finding.hit.rule.summary(),
    );
    let axes = [
        ("who can trigger", &assessment.who_can_trigger),
        ("allowed tools", &assessment.allowed_tools),
        ("permissions/secrets", &assessment.permissions_and_secrets),
    ];
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

fn halves_text(halves: u8) -> String {
    if halves.is_multiple_of(2) {
        (halves / 2).to_string()
    } else {
        format!("{}.5", halves / 2)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::agent::agent_steps;
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
        let assessment = assess(&workflow, agent_steps(&workflow).remove(0));
        let findings = assessment.findings();
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
}
