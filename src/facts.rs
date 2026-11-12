use github_actions_expressions::call::Function;
use github_actions_expressions::literal::Literal;
use github_actions_expressions::op::{BinExpr, BinOp};
use github_actions_expressions::{Expr, SpannedExpr};
use github_actions_models::common::expr::{ExplicitExpr, LoE};
use github_actions_models::common::{EnvValue, If};
use github_actions_models::workflow::event::Events;
use yamlpath::{Component, Route};

use crate::agent::{Agent, AgentStep};

const ACTOR_CONTEXTS: [&str; 2] = ["github.actor", "github.triggering_actor"];
const EVENT_PAYLOAD: &str = "github.event";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalEvent {
    Issues,
    IssueComment,
    PullRequestTarget,
    PullRequestReview,
    PullRequestReviewComment,
    Discussion,
    DiscussionComment,
    WorkflowRun,
}

impl ExternalEvent {
    pub fn name(self) -> &'static str {
        match self {
            ExternalEvent::Issues => "issues",
            ExternalEvent::IssueComment => "issue_comment",
            ExternalEvent::PullRequestTarget => "pull_request_target",
            ExternalEvent::PullRequestReview => "pull_request_review",
            ExternalEvent::PullRequestReviewComment => "pull_request_review_comment",
            ExternalEvent::Discussion => "discussion",
            ExternalEvent::DiscussionComment => "discussion_comment",
            ExternalEvent::WorkflowRun => "workflow_run",
        }
    }

    fn payload_objects(self) -> &'static [&'static str] {
        match self {
            ExternalEvent::Issues => &["github.event.issue"],
            ExternalEvent::IssueComment => &["github.event.issue", "github.event.comment"],
            ExternalEvent::PullRequestTarget => &["github.event.pull_request"],
            ExternalEvent::PullRequestReview => {
                &["github.event.pull_request", "github.event.review"]
            }
            ExternalEvent::PullRequestReviewComment => {
                &["github.event.pull_request", "github.event.comment"]
            }
            ExternalEvent::Discussion => &["github.event.discussion"],
            ExternalEvent::DiscussionComment => {
                &["github.event.discussion", "github.event.comment"]
            }
            ExternalEvent::WorkflowRun => &["github.event.workflow_run"],
        }
    }

    fn author_association(self) -> Option<&'static str> {
        match self {
            ExternalEvent::Issues => Some("github.event.issue.author_association"),
            ExternalEvent::IssueComment
            | ExternalEvent::PullRequestReviewComment
            | ExternalEvent::DiscussionComment => Some("github.event.comment.author_association"),
            ExternalEvent::PullRequestTarget => {
                Some("github.event.pull_request.author_association")
            }
            ExternalEvent::PullRequestReview => Some("github.event.review.author_association"),
            ExternalEvent::Discussion => Some("github.event.discussion.author_association"),
            ExternalEvent::WorkflowRun => None,
        }
    }
}

pub fn external_events(events: &Events) -> Vec<ExternalEvent> {
    [
        (ExternalEvent::Issues, events.issues.is_present()),
        (
            ExternalEvent::IssueComment,
            events.issue_comment.is_present(),
        ),
        (
            ExternalEvent::PullRequestTarget,
            events.pull_request_target.is_present(),
        ),
        (
            ExternalEvent::PullRequestReview,
            events.pull_request_review.is_present(),
        ),
        (
            ExternalEvent::PullRequestReviewComment,
            events.pull_request_review_comment.is_present(),
        ),
        (ExternalEvent::Discussion, events.discussion.is_present()),
        (
            ExternalEvent::DiscussionComment,
            events.discussion_comment.is_present(),
        ),
        (ExternalEvent::WorkflowRun, events.workflow_run.is_present()),
    ]
    .into_iter()
    .filter_map(|(event, present)| present.then_some(event))
    .collect()
}

#[derive(Debug)]
pub struct OpenTrigger<'w> {
    pub setting: String,
    pub route: Route<'w>,
}

pub fn open_trigger<'w>(step: &AgentStep<'w>, events: &[ExternalEvent]) -> Option<OpenTrigger<'w>> {
    let conditions = [step.job.r#if.as_ref(), step.step.shared.r#if.as_ref()];
    let gated_by_if = events.iter().all(|event| {
        conditions
            .into_iter()
            .flatten()
            .any(|condition| limits_who_runs(condition, *event))
    });
    if gated_by_if {
        return None;
    }
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
        Agent::GeminiCli => Some(OpenTrigger {
            setting: "no `if:` on the job or step checks who triggered it".to_string(),
            route: step.route.clone(),
        }),
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

fn limits_who_runs(condition: &If, event: ExternalEvent) -> bool {
    match condition {
        If::Bool(runs) => !runs,
        If::Expr(raw) => {
            let curly = ExplicitExpr::from_curly(raw.trim());
            let bare = curly.as_ref().map_or(raw.as_str(), ExplicitExpr::as_bare);
            Expr::parse(bare).is_ok_and(|expression| requires_actor_check(&expression, event))
        }
    }
}

fn requires_actor_check(expression: &SpannedExpr, event: ExternalEvent) -> bool {
    match &expression.inner {
        Expr::BinExpr(BinExpr {
            op: BinOp::And,
            lhs,
            rhs,
        }) => requires_actor_check(lhs, event) || requires_actor_check(rhs, event),
        Expr::BinExpr(BinExpr {
            op: BinOp::Or,
            lhs,
            rhs,
        }) => requires_actor_check(lhs, event) && requires_actor_check(rhs, event),
        Expr::BinExpr(BinExpr {
            op: BinOp::Eq,
            lhs,
            rhs,
        }) => {
            (is_actor_value(lhs, event) && rhs.is_literal())
                || (is_actor_value(rhs, event) && lhs.is_literal())
        }
        Expr::Call(call) if matches!(call.func, Function::Contains) => {
            matches!(call.args.as_slice(), [list, needle] if is_json_list(list) && is_actor_value(needle, event))
        }
        Expr::Literal(Literal::Boolean(runs)) => !runs,
        _ => false,
    }
}

fn is_json_list(expression: &SpannedExpr) -> bool {
    match &expression.inner {
        Expr::Call(call) if matches!(call.func, Function::FromJSON) => {
            matches!(call.args.as_slice(), [json] if matches!(json.inner, Expr::Literal(Literal::String(_))))
        }
        _ => false,
    }
}

fn is_actor_value(expression: &SpannedExpr, event: ExternalEvent) -> bool {
    match &expression.inner {
        Expr::Context(context) => {
            ACTOR_CONTEXTS
                .into_iter()
                .any(|pattern| context.matches(pattern))
                || event
                    .author_association()
                    .is_some_and(|pattern| context.matches(pattern))
        }
        Expr::BinExpr(BinExpr {
            op: BinOp::Or,
            lhs,
            rhs,
        }) => {
            if is_present(lhs, event) {
                is_actor_value(lhs, event)
            } else {
                is_actor_value(rhs, event)
            }
        }
        _ => false,
    }
}

fn is_present(expression: &SpannedExpr, event: ExternalEvent) -> bool {
    match &expression.inner {
        Expr::Context(context) if context.child_of(EVENT_PAYLOAD) => event
            .payload_objects()
            .iter()
            .any(|object| context.child_of(*object)),
        Expr::BinExpr(BinExpr {
            op: BinOp::Or,
            lhs,
            rhs,
        }) => is_present(lhs, event) || is_present(rhs, event),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::agent::agent_steps;
    use crate::workflow::Workflow;

    fn open_setting(source: &str) -> Option<String> {
        let workflow = Workflow::parse(Path::new("test.yml"), source.to_string()).unwrap();
        let events = external_events(&workflow.model.on.events);
        let steps = agent_steps(&workflow);
        assert_eq!(steps.len(), 1);
        open_trigger(&steps[0], &events).map(|open| {
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

    fn gemini_on(events: &str, condition: &str) -> Option<String> {
        open_setting(&format!(
            "on: {events}
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
    fn an_actor_check_in_if_closes_a_wildcard_for_every_agent() {
        let setting = open_setting(
            "on: issue_comment
jobs:
  codex:
    runs-on: ubuntu-latest
    if: github.actor == 'octocat'
    steps:
      - uses: openai/codex-action@v1
        with:
          allow-users: '*'
",
        );
        assert_eq!(setting, None);
    }

    #[test]
    fn gemini_is_open_at_the_step_when_no_condition_checks_the_actor() {
        assert_eq!(
            gemini_on("issues", "github.event.issue.pull_request == null").as_deref(),
            Some("7: no `if:` on the job or step checks who triggered it")
        );
    }

    #[test]
    fn gemini_conditions_that_must_check_the_actor_restrict_it() {
        for (events, condition) in [
            (
                "[issues, issue_comment]",
                "contains(fromJSON('[\"OWNER\", \"MEMBER\"]'), github.event.comment.author_association || github.event.review.author_association || github.event.issue.author_association)",
            ),
            (
                "issue_comment",
                "contains(fromJSON('[\"OWNER\"]'), github.event.comment.author_association)",
            ),
            ("issues", "github.Actor == 'octocat'"),
            (
                "issues",
                "${{ github.triggering_actor == 'octocat' && github.event_name == 'issues' }}",
            ),
            (
                "issues",
                "github.event.issue.author_association == 'OWNER' || github.actor == 'octocat'",
            ),
            ("issues", "false"),
            ("issues", "${{ false }}"),
        ] {
            assert_eq!(gemini_on(events, condition), None, "{events} {condition}");
        }
    }

    #[test]
    fn gemini_conditions_that_can_pass_for_anyone_leave_it_open() {
        for (events, condition) in [
            ("issues", "github.actor != 'github-actions[bot]'"),
            (
                "issues",
                "github.event_name == 'issues' || github.actor == 'octocat'",
            ),
            ("issues", "${{ !(github.actor == 'octocat') }}"),
            (
                "issues",
                "contains(github.event.comment.body, '@gemini-cli')",
            ),
            ("issues", "contains('alice,bob', github.actor)"),
            ("issues", "github.actor == github.event.issue.user.login"),
            ("issues", "github.event.issue.author_association"),
            ("issues", "this is not an expression (("),
            (
                "issue_comment",
                "github.event.issue.author_association == 'OWNER'",
            ),
            (
                "issue_comment",
                "contains(fromJSON('[\"OWNER\"]'), github.event.issue.author_association || github.event.comment.author_association)",
            ),
            (
                "[issues, issue_comment]",
                "github.event.issue.author_association == 'OWNER'",
            ),
        ] {
            assert!(
                gemini_on(events, condition).is_some(),
                "{events} {condition}"
            );
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

    #[test]
    fn only_events_that_outside_contributors_can_cause_are_external() {
        let workflow = Workflow::parse(
            Path::new("test.yml"),
            "on: [push, pull_request, workflow_dispatch, issue_comment, pull_request_target]
jobs: {}
"
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            external_events(&workflow.model.on.events),
            [
                ExternalEvent::IssueComment,
                ExternalEvent::PullRequestTarget
            ]
        );
    }
}
