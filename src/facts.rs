use github_actions_models::common::expr::LoE;
use github_actions_models::common::{EnvValue, If};
use yamlpath::{Component, Route};

use crate::agent::{Agent, AgentStep};

const ACTOR_CHECKS: [&str; 3] = [
    "author_association",
    "github.actor",
    "github.triggering_actor",
];

#[derive(Debug)]
pub enum TriggerGate<'w> {
    Open { setting: String, route: Route<'w> },
    Restricted,
}

pub fn trigger_gate<'w>(step: &AgentStep<'w>) -> TriggerGate<'w> {
    match step.agent {
        Agent::ClaudeCode => wildcard_input(step, &["allowed_non_write_users", "allowed_bots"]),
        Agent::Codex => wildcard_input(step, &["allow-users"]),
        Agent::GeminiCli => {
            let conditions = [step.job.r#if.as_ref(), step.step.shared.r#if.as_ref()];
            if conditions.into_iter().flatten().any(limits_who_runs) {
                TriggerGate::Restricted
            } else {
                TriggerGate::Open {
                    setting: "no `if:` on the job or step checks who triggered it".to_string(),
                    route: step.route.clone(),
                }
            }
        }
    }
}

fn wildcard_input<'w>(step: &AgentStep<'w>, inputs: &[&'static str]) -> TriggerGate<'w> {
    let LoE::Literal(with) = &step.step.with else {
        return TriggerGate::Restricted;
    };
    let wildcard = inputs.iter().find(
        |input| matches!(with.get(**input), Some(EnvValue::String(value)) if value.trim() == "*"),
    );
    match wildcard {
        Some(input) => TriggerGate::Open {
            setting: format!("{input}: '*'"),
            route: step
                .route
                .with_keys([Component::from("with"), Component::from(*input)]),
        },
        None => TriggerGate::Restricted,
    }
}

fn limits_who_runs(condition: &If) -> bool {
    match condition {
        If::Bool(runs) => !runs,
        If::Expr(expression) => ACTOR_CHECKS.iter().any(|check| expression.contains(check)),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::agent::agent_steps;
    use crate::workflow::Workflow;

    fn gate_setting(source: &str) -> Option<String> {
        let workflow = Workflow::parse(Path::new("test.yml"), source.to_string()).unwrap();
        let steps = agent_steps(&workflow);
        assert_eq!(steps.len(), 1);
        match trigger_gate(&steps[0]) {
            TriggerGate::Open { setting, route } => {
                let line = workflow
                    .document
                    .query_pretty(&route)
                    .unwrap()
                    .location
                    .point_span
                    .0
                    .0;
                Some(format!("{}: {setting}", line + 1))
            }
            TriggerGate::Restricted => None,
        }
    }

    #[test]
    fn claude_is_open_when_non_write_users_or_bots_are_a_wildcard() {
        let users = gate_setting(
            "on: issue_comment
jobs:
  claude:
    runs-on: ubuntu-latest
    steps:
      - uses: anthropics/claude-code-action@v1
        with:
          allowed_non_write_users: '*'
",
        );
        assert_eq!(users.as_deref(), Some("8: allowed_non_write_users: '*'"));

        let bots = gate_setting(
            "on: issue_comment
jobs:
  claude:
    runs-on: ubuntu-latest
    steps:
      - uses: anthropics/claude-code-action@v1
        with:
          allowed_bots: \" * \"
",
        );
        assert_eq!(bots.as_deref(), Some("8: allowed_bots: '*'"));
    }

    #[test]
    fn claude_with_named_users_or_defaults_is_restricted() {
        let named = gate_setting(
            "on: issue_comment
jobs:
  claude:
    runs-on: ubuntu-latest
    steps:
      - uses: anthropics/claude-code-action@v1
        with:
          allowed_non_write_users: alice,bob
",
        );
        assert_eq!(named, None);

        let defaults = gate_setting(
            "on: issue_comment
jobs:
  claude:
    runs-on: ubuntu-latest
    steps:
      - uses: anthropics/claude-code-action@v1
",
        );
        assert_eq!(defaults, None);
    }

    #[test]
    fn codex_is_open_when_users_are_a_wildcard() {
        let setting = gate_setting(
            "on: issues
jobs:
  codex:
    runs-on: ubuntu-latest
    steps:
      - uses: openai/codex-action@v1
        with:
          allow-users: '*'
",
        );
        assert_eq!(setting.as_deref(), Some("8: allow-users: '*'"));
    }

    #[test]
    fn gemini_without_an_actor_check_is_open_at_the_step() {
        let setting = gate_setting(
            "on: issues
jobs:
  gemini:
    runs-on: ubuntu-latest
    if: github.event.issue.pull_request == null
    steps:
      - uses: google-github-actions/run-gemini-cli@v0
",
        );
        assert_eq!(
            setting.as_deref(),
            Some("7: no `if:` on the job or step checks who triggered it")
        );
    }

    #[test]
    fn gemini_is_restricted_by_an_actor_check_on_the_job_or_the_step_or_by_if_false() {
        let on_job = gate_setting(
            "on: issues
jobs:
  gemini:
    runs-on: ubuntu-latest
    if: contains(fromJSON('[\"OWNER\", \"MEMBER\"]'), github.event.issue.author_association)
    steps:
      - uses: google-github-actions/run-gemini-cli@v0
",
        );
        assert_eq!(on_job, None);

        let on_step = gate_setting(
            "on: issues
jobs:
  gemini:
    runs-on: ubuntu-latest
    steps:
      - uses: google-github-actions/run-gemini-cli@v0
        if: github.actor == 'octocat'
",
        );
        assert_eq!(on_step, None);

        let disabled = gate_setting(
            "on: issues
jobs:
  gemini:
    runs-on: ubuntu-latest
    if: false
    steps:
      - uses: google-github-actions/run-gemini-cli@v0
",
        );
        assert_eq!(disabled, None);
    }
}
