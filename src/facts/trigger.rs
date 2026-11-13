use github_actions_expressions::call::Function;
use github_actions_expressions::literal::Literal;
use github_actions_expressions::op::{BinExpr, BinOp};
use github_actions_expressions::{Expr, SpannedExpr};
use github_actions_models::common::If;
use github_actions_models::workflow::event::Events;
use yamlpath::Route;

use super::{input, input_route, parse_expression};
use crate::agent::{Agent, AgentStep};

const ACTOR_CONTEXTS: [&str; 3] = [
    "github.actor",
    "github.triggering_actor",
    "github.event.sender.login",
];
const EVENT_NAME_CONTEXT: &str = "github.event_name";
const EVENT_PAYLOAD: &str = "github.event";
const EVENT_SENDER: &str = "github.event.sender";
const UPSTREAM_EVENT: &str = "github.event.workflow_run.event";
const UPSTREAM_REPOSITORY: &str = "github.event.workflow_run.head_repository.full_name";
const THIS_REPOSITORY: &str = "github.repository";
const WRITER_EVENTS: [&str; 3] = ["push", "workflow_dispatch", "schedule"];
const SAME_REPOSITORY_EVENT: &str = "pull_request";
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

pub fn outside_events(step: &AgentStep, events: &[ExternalEvent]) -> Vec<ExternalEvent> {
    let writers_only = events.contains(&ExternalEvent::WorkflowRun)
        && upstream_origin(step) == UpstreamOrigin::Writers;
    events
        .iter()
        .copied()
        .filter(|event| !(*event == ExternalEvent::WorkflowRun && writers_only))
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
    let upstream_restricted = events.contains(&ExternalEvent::WorkflowRun)
        && upstream_origin(step) != UpstreamOrigin::Anyone;
    let open_events: Vec<ExternalEvent> = events
        .iter()
        .copied()
        .filter(|event| !(*event == ExternalEvent::WorkflowRun && upstream_restricted))
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
            If::Expr(raw) => parse_expression(raw),
        })
        .collect()
}

fn wildcard<'w>(step: &AgentStep<'w>, name: &str) -> Option<(String, Route<'w>)> {
    let (key, value) = input(step, name)?;
    (value.trim() == "*").then(|| (format!("{key}: '*'"), input_route(step, key)))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum UpstreamOrigin {
    Writers,
    SameRepository,
    Anyone,
}

#[derive(Clone)]
struct UpstreamPath {
    events: Option<Vec<String>>,
    same_repository: bool,
}

impl UpstreamPath {
    const ANY: UpstreamPath = UpstreamPath {
        events: None,
        same_repository: false,
    };

    fn and(&self, other: &UpstreamPath) -> UpstreamPath {
        let events = match (&self.events, &other.events) {
            (Some(left), Some(right)) => Some(
                left.iter()
                    .filter(|event| right.contains(event))
                    .cloned()
                    .collect(),
            ),
            (Some(only), None) | (None, Some(only)) => Some(only.clone()),
            (None, None) => None,
        };
        UpstreamPath {
            events,
            same_repository: self.same_repository || other.same_repository,
        }
    }

    fn origin(&self) -> UpstreamOrigin {
        let Some(events) = &self.events else {
            return UpstreamOrigin::Anyone;
        };
        if events
            .iter()
            .all(|event| WRITER_EVENTS.contains(&event.as_str()))
        {
            UpstreamOrigin::Writers
        } else if self.same_repository
            && events.iter().all(|event| {
                WRITER_EVENTS.contains(&event.as_str()) || event == SAME_REPOSITORY_EVENT
            })
        {
            UpstreamOrigin::SameRepository
        } else {
            UpstreamOrigin::Anyone
        }
    }
}

fn upstream_origin(step: &AgentStep) -> UpstreamOrigin {
    conditions(step)
        .iter()
        .map(|condition| match condition {
            Some(condition) => upstream_paths(condition),
            None => vec![UpstreamPath::ANY],
        })
        .fold(vec![UpstreamPath::ANY], |paths, condition| {
            paths
                .iter()
                .flat_map(|path| condition.iter().map(|other| path.and(other)))
                .collect()
        })
        .iter()
        .map(UpstreamPath::origin)
        .max()
        .unwrap_or(UpstreamOrigin::Writers)
}

fn upstream_paths(expression: &SpannedExpr) -> Vec<UpstreamPath> {
    match &expression.inner {
        Expr::BinExpr(BinExpr {
            op: BinOp::And,
            lhs,
            rhs,
        }) => {
            let right = upstream_paths(rhs);
            upstream_paths(lhs)
                .iter()
                .flat_map(|left| right.iter().map(|other| left.and(other)))
                .collect()
        }
        Expr::BinExpr(BinExpr {
            op: BinOp::Or,
            lhs,
            rhs,
        }) => {
            let mut paths = upstream_paths(lhs);
            paths.extend(upstream_paths(rhs));
            paths
        }
        _ if requires_actor_check(expression, ExternalEvent::WorkflowRun) => Vec::new(),
        _ => vec![upstream_fact(expression)],
    }
}

fn upstream_fact(expression: &SpannedExpr) -> UpstreamPath {
    let events = |events: Vec<String>| UpstreamPath {
        events: Some(events),
        same_repository: false,
    };
    match &expression.inner {
        Expr::BinExpr(BinExpr {
            op: BinOp::Eq,
            lhs,
            rhs,
        }) => match (&lhs.inner, &rhs.inner) {
            (Expr::Context(context), Expr::Literal(Literal::String(name)))
            | (Expr::Literal(Literal::String(name)), Expr::Context(context))
                if context.matches(UPSTREAM_EVENT) =>
            {
                events(vec![name.to_ascii_lowercase()])
            }
            (Expr::Context(left), Expr::Context(right))
                if (left.matches(UPSTREAM_REPOSITORY) && right.matches(THIS_REPOSITORY))
                    || (left.matches(THIS_REPOSITORY) && right.matches(UPSTREAM_REPOSITORY)) =>
            {
                UpstreamPath {
                    events: None,
                    same_repository: true,
                }
            }
            _ => UpstreamPath::ANY,
        },
        Expr::Call(call) if matches!(call.func, Function::Contains) => match call.args.as_slice() {
            [list, needle] if matches!(&needle.inner, Expr::Context(context) if context.matches(UPSTREAM_EVENT)) => {
                json_string_list(list).map_or(UpstreamPath::ANY, |names| {
                    events(names.iter().map(|name| name.to_ascii_lowercase()).collect())
                })
            }
            _ => UpstreamPath::ANY,
        },
        _ => UpstreamPath::ANY,
    }
}

fn json_string_list(expression: &SpannedExpr) -> Option<Vec<String>> {
    let Expr::Call(from_json) = &expression.inner else {
        return None;
    };
    let (Function::FromJSON, [json]) = (&from_json.func, from_json.args.as_slice()) else {
        return None;
    };
    let Expr::Literal(Literal::String(text)) = &json.inner else {
        return None;
    };
    serde_json::from_str(text).ok()
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

    fn outside_on(events: &str, condition: &str) -> Vec<&'static str> {
        let workflow = Workflow::parse(
            Path::new("test.yml"),
            format!(
                "on: {events}
jobs:
  gemini:
    runs-on: ubuntu-latest
    if: {condition}
    steps:
      - uses: google-github-actions/run-gemini-cli@v0
"
            ),
        )
        .unwrap();
        let steps = agent_steps(&workflow);
        outside_events(&steps[0], &external_events(&workflow.model.on.events))
            .into_iter()
            .map(ExternalEvent::name)
            .collect()
    }

    #[test]
    fn workflow_run_from_writer_only_upstream_events_is_not_an_outside_event() {
        for (events, condition) in [
            ("workflow_run", "github.event.workflow_run.event == 'push'"),
            (
                "workflow_run",
                "github.event.workflow_run.event == 'push' || github.event.workflow_run.event == 'schedule'",
            ),
            (
                "workflow_run",
                "contains(fromJSON('[\"push\", \"Workflow_Dispatch\"]'), github.event.workflow_run.event)",
            ),
            (
                "workflow_run",
                "(github.event.workflow_run.event == 'pull_request_target' && github.actor == 'octocat') || github.event.workflow_run.event == 'push'",
            ),
            (
                "workflow_run",
                "github.event.workflow_run.event == 'push' || false",
            ),
            (
                "[workflow_run, push]",
                "github.event_name != 'workflow_run' || github.event.workflow_run.event == 'push'",
            ),
        ] {
            assert_eq!(
                outside_on(events, condition),
                Vec::<&str>::new(),
                "{condition}"
            );
        }
    }

    #[test]
    fn workflow_run_from_a_same_repository_pull_request_is_restricted_but_still_outside() {
        for condition in [
            "(github.event.workflow_run.event == 'pull_request' || github.event.workflow_run.event == 'workflow_dispatch') && github.event.workflow_run.head_repository.full_name == github.repository",
            "github.repository == github.event.workflow_run.head_repository.full_name && github.event.workflow_run.event == 'Pull_Request'",
            "(github.event.workflow_run.event == 'pull_request' && github.event.workflow_run.head_repository.full_name == github.repository) || github.event.workflow_run.event == 'push'",
            "(github.event.workflow_run.event == 'pull_request' || github.event.workflow_run.event == 'push') && (github.event.workflow_run.head_repository.full_name == github.repository || github.event.workflow_run.event == 'push')",
        ] {
            assert_eq!(
                outside_on("workflow_run", condition),
                ["workflow_run"],
                "{condition}"
            );
            assert_eq!(gemini_on("workflow_run", condition), None, "{condition}");
        }
    }

    #[test]
    fn workflow_run_that_outsiders_can_start_upstream_stays_open() {
        for condition in [
            "github.event.workflow_run.head_repository.full_name == github.repository",
            "github.event.workflow_run.event == 'pull_request'",
            "github.event.workflow_run.event == 'push' || github.event.workflow_run.event == 'pull_request_target'",
            "github.event.workflow_run.event == 'push' || contains(github.event.workflow_run.head_branch, 'x')",
            "github.event.workflow_run.event != 'pull_request_target'",
            "(github.event.workflow_run.event == 'pull_request' && github.event.workflow_run.head_repository.full_name == github.repository) || github.event.workflow_run.event == 'issue_comment'",
            "contains(fromJSON('[\"push\", \"issue_comment\"]'), github.event.workflow_run.event)",
            "github.event.workflow_run.conclusion == 'success'",
        ] {
            assert_eq!(
                outside_on("workflow_run", condition),
                ["workflow_run"],
                "{condition}"
            );
            assert!(
                gemini_on("workflow_run", condition).is_some(),
                "{condition}"
            );
        }
    }

    #[test]
    fn workflow_run_job_and_step_conditions_are_read_together() {
        let setting = open_setting(
            "on: workflow_run
jobs:
  gemini:
    runs-on: ubuntu-latest
    if: github.event.workflow_run.event == 'pull_request'
    steps:
      - uses: google-github-actions/run-gemini-cli@v0
        if: github.event.workflow_run.head_repository.full_name == github.repository
",
        );
        assert_eq!(setting, None);
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
