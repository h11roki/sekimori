use github_actions_expressions::call::Function;
use github_actions_expressions::literal::Literal;
use github_actions_expressions::op::{BinExpr, BinOp};
use github_actions_expressions::{Expr, SpannedExpr};
use github_actions_models::common::expr::LoE;
use github_actions_models::common::{EnvValue, If};
use github_actions_models::workflow::event::Events;
use yamlpath::{Component, Route};

use crate::agent::{Agent, AgentStep};

const ACTOR_CONTEXTS: [&str; 3] = [
    "github.actor",
    "github.triggering_actor",
    "github.event.sender.login",
];
const EVENT_NAME_CONTEXT: &str = "github.event_name";
const EVENT_PAYLOAD: &str = "github.event";
const EVENT_SENDER: &str = "github.event.sender";
const OUTSIDER_ASSOCIATIONS: [&str; 5] = [
    "NONE",
    "FIRST_TIMER",
    "FIRST_TIME_CONTRIBUTOR",
    "CONTRIBUTOR",
    "MANNEQUIN",
];

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

    fn written_by_trigger(self) -> Option<&'static str> {
        match self {
            ExternalEvent::WorkflowRun => None,
            _ => self.payload_objects().last().copied(),
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
    pub events: Vec<ExternalEvent>,
}

pub fn never_runs(step: &AgentStep) -> bool {
    conditions(step).iter().any(|condition| {
        condition
            .as_ref()
            .and_then(|expression| expression.consteval())
            .is_some_and(|value| !value.as_boolean())
    })
}

pub fn open_trigger<'w>(step: &AgentStep<'w>, events: &[ExternalEvent]) -> Option<OpenTrigger<'w>> {
    let conditions = conditions(step);
    let (setting, route) = match step.agent {
        Agent::ClaudeCode => {
            let token_given =
                input(step, "github_token").is_some_and(|(_, value)| !value.trim().is_empty());
            token_given
                .then(|| wildcard(step, "allowed_non_write_users"))
                .flatten()
                .or_else(|| wildcard(step, "allowed_bots"))?
        }
        Agent::Codex => wildcard(step, "allow-users")?,
        Agent::GeminiCli => {
            let setting = if conditions.is_empty() {
                "no `if:` on the job or step"
            } else {
                "the `if:` does not check who triggered it"
            };
            (setting.to_string(), step.route.clone())
        }
    };
    let open_events: Vec<ExternalEvent> = events
        .iter()
        .copied()
        .filter(|event| {
            !conditions
                .iter()
                .flatten()
                .any(|condition| requires_actor_check(condition, *event))
        })
        .collect();
    (!open_events.is_empty()).then_some(OpenTrigger {
        setting,
        route,
        events: open_events,
    })
}

fn conditions<'w>(step: &AgentStep<'w>) -> Vec<Option<SpannedExpr<'w>>> {
    [step.job.r#if.as_ref(), step.step.shared.r#if.as_ref()]
        .into_iter()
        .flatten()
        .map(|condition| match condition {
            If::Bool(runs) => Expr::parse(if *runs { "true" } else { "false" }).ok(),
            If::Expr(raw) => {
                let trimmed = raw.trim();
                let bare = trimmed
                    .strip_prefix("${{")
                    .and_then(|body| body.strip_suffix("}}"))
                    .map_or(trimmed, str::trim);
                Expr::parse(bare).ok()
            }
        })
        .collect()
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

fn wildcard<'w>(step: &AgentStep<'w>, name: &str) -> Option<(String, Route<'w>)> {
    let (key, value) = input(step, name)?;
    (value.trim() == "*").then(|| {
        (
            format!("{key}: '*'"),
            step.route
                .with_keys([Component::from("with"), Component::from(key)]),
        )
    })
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
            op: op @ (BinOp::Eq | BinOp::Neq),
            lhs,
            rhs,
        }) => {
            let (subject, value) = match (&lhs.inner, &rhs.inner) {
                (_, Expr::Literal(value)) => (lhs, value),
                (Expr::Literal(value), _) => (rhs, value),
                _ => return false,
            };
            if matches!(&subject.inner, Expr::Context(context) if context.matches(EVENT_NAME_CONTEXT))
            {
                let same_event = matches!(value, Literal::String(name) if name.eq_ignore_ascii_case(event.name()));
                return (*op == BinOp::Eq) != same_event;
            }
            *op == BinOp::Eq
                && is_actor_value(subject, event)
                && !matches!(value, Literal::String(text) if admits_outsiders(text))
        }
        Expr::Call(call) if matches!(call.func, Function::Contains) => {
            let [list, needle] = call.args.as_slice() else {
                return false;
            };
            let trusted_list = matches!(&list.inner, Expr::Call(from_json)
                if matches!(from_json.func, Function::FromJSON)
                    && matches!(from_json.args.as_slice(), [json]
                        if matches!(&json.inner, Expr::Literal(Literal::String(text))
                            if text.trim_start().starts_with('[') && !admits_outsiders(text))));
            trusted_list && is_actor_value(needle, event)
        }
        Expr::Literal(Literal::Boolean(runs)) => !runs,
        _ => false,
    }
}

fn admits_outsiders(text: &str) -> bool {
    let upper = text.trim().to_ascii_uppercase();
    OUTSIDER_ASSOCIATIONS
        .iter()
        .any(|association| upper == *association || upper.contains(&format!("\"{association}\"")))
}

fn is_actor_value(expression: &SpannedExpr, event: ExternalEvent) -> bool {
    match &expression.inner {
        Expr::Context(context) => {
            ACTOR_CONTEXTS.into_iter().any(|pattern| context.matches(pattern))
                || event.written_by_trigger().is_some_and(|object| {
                    context.child_of(object)
                        && matches!(context.parts.as_slice(), [_, _, _, last]
                            if matches!(&last.inner, Expr::Identifier(name) if name.eq("author_association")))
                })
        }
        Expr::BinExpr(BinExpr { op: BinOp::Or, lhs, rhs }) => {
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
        Expr::Context(context)
            if context.child_of(EVENT_PAYLOAD) && !context.child_of(EVENT_SENDER) =>
        {
            event
                .payload_objects()
                .iter()
                .any(|object| context.child_of(*object))
        }
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
            Some("7: the `if:` does not check who triggered it")
        );
        let without_if = open_setting(
            "on: issues
jobs:
  gemini:
    runs-on: ubuntu-latest
    steps:
      - uses: google-github-actions/run-gemini-cli@v0
",
        );
        assert_eq!(
            without_if.as_deref(),
            Some("6: no `if:` on the job or step")
        );
    }

    #[test]
    fn open_trigger_names_only_the_events_the_if_does_not_check() {
        let workflow = Workflow::parse(
            Path::new("test.yml"),
            "on: [issues, issue_comment]
jobs:
  gemini:
    runs-on: ubuntu-latest
    if: contains(fromJSON('[\"OWNER\"]'), github.event.comment.author_association)
    steps:
      - uses: google-github-actions/run-gemini-cli@v0
"
            .to_string(),
        )
        .unwrap();
        let events = external_events(&workflow.model.on.events);
        let open = open_trigger(&agent_steps(&workflow)[0], &events).unwrap();
        assert_eq!(open.events, [ExternalEvent::Issues]);
    }

    #[test]
    fn a_constant_false_condition_means_the_step_never_runs() {
        let never = |condition: &str| {
            let workflow = Workflow::parse(
                Path::new("test.yml"),
                format!(
                    "on: issues
jobs:
  gemini:
    runs-on: ubuntu-latest
    steps:
      - uses: google-github-actions/run-gemini-cli@v0
        if: {condition}
"
                ),
            )
            .unwrap();
            never_runs(&agent_steps(&workflow)[0])
        };
        assert!(never("false"));
        assert!(never("${{ false }}"));
        assert!(!never("github.actor == 'octocat'"));
        assert!(!never("true"));
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
            ("issue_comment", "github.event.sender.login == 'octocat'"),
            (
                "[issues, issue_comment]",
                "(github.event_name == 'issues' && contains(fromJSON('[\"OWNER\"]'), github.event.issue.author_association)) || (github.event_name == 'issue_comment' && contains(fromJSON('[\"OWNER\"]'), github.event.comment.author_association))",
            ),
            (
                "[issues, issue_comment]",
                "github.event_name == 'issue_comment' && contains(fromJSON('[\"OWNER\"]'), github.event.comment.author_association)",
            ),
            (
                "[issues, issue_comment]",
                "github.event_name != 'issues' && github.actor == 'octocat'",
            ),
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
            (
                "issues",
                "contains(fromJSON('\"alice,bob\"'), github.actor)",
            ),
            (
                "issue_comment",
                "contains(fromJSON('[\"OWNER\", \"NONE\"]'), github.event.comment.author_association)",
            ),
            (
                "issue_comment",
                "github.event.comment.author_association == 'none'",
            ),
            ("issues", "github.event_name == 'issues'"),
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
