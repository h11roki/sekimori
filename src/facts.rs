use github_actions_expressions::call::Function;
use github_actions_expressions::context::Context;
use github_actions_expressions::literal::Literal;
use github_actions_expressions::op::{BinExpr, BinOp};
use github_actions_expressions::{Expr, SpannedExpr};
use github_actions_models::common::expr::{ExplicitExpr, LoE};
use github_actions_models::common::{EnvValue, If};
use yamlpath::{Component, Route};

use crate::agent::{Agent, AgentStep};

const ACTOR_CONTEXTS: [&str; 3] = [
    "github.actor",
    "github.triggering_actor",
    "github.event.*.author_association",
];

#[derive(Debug)]
pub struct OpenTrigger<'w> {
    pub setting: String,
    pub route: Route<'w>,
}

pub fn open_trigger<'w>(step: &AgentStep<'w>) -> Option<OpenTrigger<'w>> {
    match step.agent {
        Agent::ClaudeCode => {
            let token_given =
                input(step, "github_token").is_some_and(|(_, value)| !value.trim().is_empty());
            token_given
                .then(|| wildcard(step, "allowed_non_write_users"))
                .flatten()
                .or_else(|| wildcard(step, "allowed_bots"))
        }
        Agent::Codex => wildcard(step, "allow-users"),
        Agent::GeminiCli => {
            let conditions = [step.job.r#if.as_ref(), step.step.shared.r#if.as_ref()];
            (!conditions.into_iter().flatten().any(limits_who_runs)).then(|| OpenTrigger {
                setting: "no `if:` on the job or step checks who triggered it".to_string(),
                route: step.route.clone(),
            })
        }
    }
}

fn input<'w>(step: &AgentStep<'w>, name: &str) -> Option<(&'w str, &'w str)> {
    let LoE::Literal(with) = &step.step.with else {
        return None;
    };
    with.iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .and_then(|(key, value)| match value {
            EnvValue::String(value) => Some((key.as_str(), value.as_str())),
            EnvValue::Number(_) | EnvValue::Boolean(_) => None,
        })
}

fn wildcard<'w>(step: &AgentStep<'w>, name: &str) -> Option<OpenTrigger<'w>> {
    let (key, value) = input(step, name)?;
    (value.trim() == "*").then(|| OpenTrigger {
        setting: format!("{key}: '*'"),
        route: step
            .route
            .with_keys([Component::from("with"), Component::from(key)]),
    })
}

fn limits_who_runs(condition: &If) -> bool {
    match condition {
        If::Bool(runs) => !runs,
        If::Expr(raw) => {
            let curly = ExplicitExpr::from_curly(raw.trim());
            let bare = curly.as_ref().map_or(raw.as_str(), ExplicitExpr::as_bare);
            Expr::parse(bare).is_ok_and(|expression| requires_actor_check(&expression))
        }
    }
}

fn requires_actor_check(expression: &SpannedExpr) -> bool {
    match &expression.inner {
        Expr::BinExpr(BinExpr {
            op: BinOp::And,
            lhs,
            rhs,
        }) => requires_actor_check(lhs) || requires_actor_check(rhs),
        Expr::BinExpr(BinExpr {
            op: BinOp::Or,
            lhs,
            rhs,
        }) => requires_actor_check(lhs) && requires_actor_check(rhs),
        Expr::BinExpr(BinExpr {
            op: BinOp::Eq,
            lhs,
            rhs,
        }) => [lhs, rhs].into_iter().any(|side| match &side.inner {
            Expr::Context(context) => is_actor(context),
            _ => false,
        }),
        Expr::Call(call) if matches!(call.func, Function::Contains) => {
            call.args.get(1).is_some_and(|needle| {
                needle
                    .contexts()
                    .into_iter()
                    .any(|(context, _)| is_actor(context))
            })
        }
        Expr::Literal(Literal::Boolean(runs)) => !runs,
        _ => false,
    }
}

fn is_actor(context: &Context) -> bool {
    ACTOR_CONTEXTS
        .into_iter()
        .any(|pattern| context.matches(pattern))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::agent::agent_steps;
    use crate::workflow::Workflow;

    fn open_setting(source: &str) -> Option<String> {
        let workflow = Workflow::parse(Path::new("test.yml"), source.to_string()).unwrap();
        let steps = agent_steps(&workflow);
        assert_eq!(steps.len(), 1);
        open_trigger(&steps[0]).map(|open| {
            let (line, _) = workflow.position(&open.route).unwrap();
            format!("{line}: {}", open.setting)
        })
    }

    fn claude_with(inputs: &str) -> Option<String> {
        open_setting(&format!(
            "on: issue_comment
jobs:
  claude:
    runs-on: ubuntu-latest
    steps:
      - uses: anthropics/claude-code-action@v1
        with:
{inputs}"
        ))
    }

    fn gemini_with_job_if(condition: &str) -> Option<String> {
        open_setting(&format!(
            "on: issues
jobs:
  gemini:
    runs-on: ubuntu-latest
    if: {condition}
    steps:
      - uses: google-github-actions/run-gemini-cli@v0
"
        ))
    }

    #[test]
    fn claude_non_write_users_wildcard_opens_only_with_a_github_token() {
        let with_token = claude_with(
            "          github_token: ${{ secrets.GITHUB_TOKEN }}
          allowed_non_write_users: '*'
",
        );
        assert_eq!(
            with_token.as_deref(),
            Some("9: allowed_non_write_users: '*'")
        );

        let without_token = claude_with("          allowed_non_write_users: '*'\n");
        assert_eq!(without_token, None);
    }

    #[test]
    fn claude_bots_wildcard_opens_only_as_the_whole_value() {
        let whole = claude_with("          allowed_bots: \" * \"\n");
        assert_eq!(whole.as_deref(), Some("8: allowed_bots: '*'"));

        let in_a_list = claude_with("          allowed_bots: \"dependabot[bot],*\"\n");
        assert_eq!(in_a_list, None);
    }

    #[test]
    fn claude_with_named_users_is_restricted() {
        let named = claude_with(
            "          github_token: ${{ secrets.GITHUB_TOKEN }}
          allowed_non_write_users: alice,bob
",
        );
        assert_eq!(named, None);
    }

    #[test]
    fn input_names_are_matched_ignoring_case() {
        let setting = claude_with("          Allowed_Bots: '*'\n");
        assert_eq!(setting.as_deref(), Some("8: Allowed_Bots: '*'"));
    }

    #[test]
    fn codex_users_wildcard_opens_only_as_the_whole_value() {
        let codex_with = |value: &str| {
            open_setting(&format!(
                "on: issues
jobs:
  codex:
    runs-on: ubuntu-latest
    steps:
      - uses: openai/codex-action@v1
        with:
          allow-users: {value}
"
            ))
        };
        assert_eq!(codex_with("'*'").as_deref(), Some("8: allow-users: '*'"));
        assert_eq!(codex_with("'alice, *'"), None);
    }

    #[test]
    fn gemini_is_open_at_the_step_when_no_condition_checks_the_actor() {
        assert_eq!(
            gemini_with_job_if("github.event.issue.pull_request == null").as_deref(),
            Some("7: no `if:` on the job or step checks who triggered it")
        );
    }

    #[test]
    fn gemini_conditions_that_must_check_the_actor_restrict_it() {
        for condition in [
            "contains(fromJSON('[\"OWNER\", \"MEMBER\"]'), github.event.comment.author_association || github.event.issue.author_association)",
            "github.Actor == 'octocat'",
            "${{ github.triggering_actor == 'octocat' && github.event_name == 'issues' }}",
            "github.event.issue.author_association == 'OWNER' || github.actor == 'octocat'",
            "false",
            "${{ false }}",
        ] {
            assert_eq!(gemini_with_job_if(condition), None, "{condition}");
        }
    }

    #[test]
    fn gemini_conditions_that_can_pass_for_anyone_leave_it_open() {
        for condition in [
            "github.actor != 'github-actions[bot]'",
            "github.event_name == 'issues' || github.actor == 'octocat'",
            "${{ !(github.actor == 'octocat') }}",
            "contains(github.event.comment.body, '@gemini-cli')",
            "github.event.issue.author_association",
            "this is not an expression ((",
        ] {
            assert!(gemini_with_job_if(condition).is_some(), "{condition}");
        }
    }

    #[test]
    fn gemini_step_condition_counts_too() {
        let setting = open_setting(
            "on: issues
jobs:
  gemini:
    runs-on: ubuntu-latest
    steps:
      - uses: google-github-actions/run-gemini-cli@v0
        if: github.actor == 'octocat'
",
        );
        assert_eq!(setting, None);
    }
}
