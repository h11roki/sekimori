use github_actions_expressions::call::Function;
use github_actions_expressions::literal::Literal;
use github_actions_expressions::op::{BinExpr, BinOp};
use github_actions_expressions::{Expr, SpannedExpr};
use github_actions_models::common::If;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use github_actions_models::workflow::event::{Events, OptionalBody};
use yamlpath::Route;

use super::{input, input_route, parse_expression};
use crate::agent::{Agent, AgentStep};
use crate::workflow::Workflow;

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
const WRITER_EVENTS: [&str; 4] = [
    "workflow_dispatch",
    "schedule",
    "repository_dispatch",
    "release",
];
const RESTRICTED_EVENTS: [&str; 4] = ["push", "create", "delete", "merge_group"];
const SAME_REPOSITORY_EVENT: &str = "pull_request";
const LABEL_NAME: &str = "github.event.label.name";
const GLOB_CHARACTERS: [char; 6] = ['*', '?', '+', '!', '[', '\\'];
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

pub fn outside_events(events: &[ExternalEvent], upstream: UpstreamOrigin) -> Vec<ExternalEvent> {
    events
        .iter()
        .copied()
        .filter(|event| {
            !(*event == ExternalEvent::WorkflowRun && upstream == UpstreamOrigin::Writers)
        })
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

pub fn open_trigger<'w>(
    step: &AgentStep<'w>,
    events: &[ExternalEvent],
    upstream: UpstreamOrigin,
) -> Option<OpenTrigger<'w>> {
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
    let upstream_restricted = upstream != UpstreamOrigin::Anyone;
    let open_events: Vec<ExternalEvent> = events
        .iter()
        .copied()
        .filter(|event| !(*event == ExternalEvent::WorkflowRun && upstream_restricted))
        .filter(|event| {
            !conditions
                .iter()
                .flatten()
                .any(|condition| gates_who_starts(condition, *event))
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

fn gates_who_starts(expression: &SpannedExpr, event: ExternalEvent) -> bool {
    match &expression.inner {
        Expr::BinExpr(BinExpr {
            op: BinOp::And,
            lhs,
            rhs,
        }) => gates_who_starts(lhs, event) || gates_who_starts(rhs, event),
        Expr::BinExpr(BinExpr {
            op: BinOp::Or,
            lhs,
            rhs,
        }) => gates_who_starts(lhs, event) && gates_who_starts(rhs, event),
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
            if *op != BinOp::Eq {
                return false;
            }
            let label = matches!(&subject.inner, Expr::Context(context) if context.matches(LABEL_NAME))
                && matches!(value, Literal::String(_));
            label
                || (is_actor_value(subject, event)
                    && !matches!(value, Literal::String(text) if admits_outsiders(text)))
        }
        Expr::Call(call) if matches!(call.func, Function::Contains) => {
            let [list, needle] = call.args.as_slice() else {
                return false;
            };
            let trusted_list = json_string_list(list)
                .is_some_and(|names| !names.iter().any(|name| admits_outsiders(name)));
            trusted_list && is_actor_value(needle, event)
        }
        Expr::Literal(Literal::Boolean(runs)) => !runs,
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum UpstreamOrigin {
    Writers,
    Restricted,
    Anyone,
}

#[derive(Clone)]
struct UpstreamPath {
    events: Option<Vec<String>>,
    same_repository: bool,
    gated: bool,
}

impl UpstreamPath {
    const ANY: UpstreamPath = UpstreamPath {
        events: None,
        same_repository: false,
        gated: false,
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
            gated: self.gated || other.gated,
        }
    }

    fn origin(&self) -> UpstreamOrigin {
        let origin = self
            .events
            .as_ref()
            .map_or(UpstreamOrigin::Anyone, |events| {
                events
                    .iter()
                    .map(|event| match event_origin(event) {
                        UpstreamOrigin::Anyone
                            if self.same_repository && event == SAME_REPOSITORY_EVENT =>
                        {
                            UpstreamOrigin::Restricted
                        }
                        origin => origin,
                    })
                    .max()
                    .unwrap_or(UpstreamOrigin::Writers)
            });
        if self.gated {
            origin.min(UpstreamOrigin::Restricted)
        } else {
            origin
        }
    }
}

#[derive(Debug, Default)]
pub struct Upstreams {
    origins: HashMap<(PathBuf, String), UpstreamOrigin>,
}

impl Upstreams {
    pub fn new<'w>(workflows: impl IntoIterator<Item = &'w Workflow>) -> Upstreams {
        let mut origins: HashMap<(PathBuf, String), UpstreamOrigin> = HashMap::new();
        for workflow in workflows {
            let Some(name) = workflow.model.name.clone() else {
                continue;
            };
            let origin = trigger_origin(&workflow.model.on.events);
            origins
                .entry((directory(workflow), name))
                .and_modify(|known| *known = (*known).max(origin))
                .or_insert(origin);
        }
        Upstreams { origins }
    }

    pub fn origin_of(&self, workflow: &Workflow) -> UpstreamOrigin {
        let OptionalBody::Body(run) = &workflow.model.on.events.workflow_run else {
            return UpstreamOrigin::Anyone;
        };
        let directory = directory(workflow);
        run.workflows
            .iter()
            .map(|name| {
                if name.contains(GLOB_CHARACTERS) {
                    return UpstreamOrigin::Anyone;
                }
                self.origins
                    .get(&(directory.clone(), name.clone()))
                    .copied()
                    .unwrap_or(UpstreamOrigin::Anyone)
            })
            .max()
            .unwrap_or(UpstreamOrigin::Anyone)
    }
}

fn directory(workflow: &Workflow) -> PathBuf {
    let parent = workflow.path.parent().unwrap_or(Path::new(""));
    parent
        .canonicalize()
        .unwrap_or_else(|_| parent.to_path_buf())
}

fn event_origin(event: &str) -> UpstreamOrigin {
    if WRITER_EVENTS.contains(&event) {
        UpstreamOrigin::Writers
    } else if RESTRICTED_EVENTS.contains(&event) {
        UpstreamOrigin::Restricted
    } else {
        UpstreamOrigin::Anyone
    }
}

fn trigger_origin(events: &Events) -> UpstreamOrigin {
    let known = [
        ("workflow_dispatch", events.workflow_dispatch.is_present()),
        ("schedule", events.schedule.is_present()),
        (
            "repository_dispatch",
            events.repository_dispatch.is_present(),
        ),
        ("release", events.release.is_present()),
        ("push", events.push.is_present()),
        ("create", events.create.is_present()),
        ("delete", events.delete.is_present()),
        ("merge_group", events.merge_group.is_present()),
    ];
    let present: Vec<&str> = known
        .into_iter()
        .filter_map(|(name, present)| present.then_some(name))
        .collect();
    if events.count() == 0 || (present.len() as u32) < events.count() {
        return UpstreamOrigin::Anyone;
    }
    present
        .into_iter()
        .map(event_origin)
        .max()
        .unwrap_or(UpstreamOrigin::Anyone)
}

pub fn upstream_origin(step: &AgentStep, from_files: UpstreamOrigin) -> UpstreamOrigin {
    condition_origin(step).min(from_files)
}

fn condition_origin(step: &AgentStep) -> UpstreamOrigin {
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
        _ if excludes_workflow_run(expression) => Vec::new(),
        _ if gates_who_starts(expression, ExternalEvent::WorkflowRun) => vec![UpstreamPath {
            gated: true,
            ..UpstreamPath::ANY
        }],
        _ => vec![upstream_fact(expression)],
    }
}

fn excludes_workflow_run(expression: &SpannedExpr) -> bool {
    match &expression.inner {
        Expr::Literal(Literal::Boolean(runs)) => !runs,
        Expr::BinExpr(BinExpr {
            op: op @ (BinOp::Eq | BinOp::Neq),
            lhs,
            rhs,
        }) => match (&lhs.inner, &rhs.inner) {
            (Expr::Context(context), Expr::Literal(Literal::String(name)))
            | (Expr::Literal(Literal::String(name)), Expr::Context(context))
                if context.matches(EVENT_NAME_CONTEXT) =>
            {
                let same_event = name.eq_ignore_ascii_case(ExternalEvent::WorkflowRun.name());
                (*op == BinOp::Eq) != same_event
            }
            (Expr::Context(context), Expr::Literal(Literal::String(_)))
            | (Expr::Literal(Literal::String(_)), Expr::Context(context))
                if context.matches(LABEL_NAME) =>
            {
                *op == BinOp::Eq
            }
            _ => false,
        },
        _ => false,
    }
}

fn upstream_fact(expression: &SpannedExpr) -> UpstreamPath {
    let events = |events: Vec<String>| UpstreamPath {
        events: Some(events),
        ..UpstreamPath::ANY
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
                    same_repository: true,
                    ..UpstreamPath::ANY
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
        let workflow = parsed(source);
        let events = external_events(&workflow.model.on.events);
        let steps = agent_steps(&workflow);
        assert_eq!(steps.len(), 1);
        let upstream = upstream_origin(&steps[0], UpstreamOrigin::Anyone);
        open_trigger(&steps[0], &events, upstream).map(|open| {
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
        let workflow = parsed(
            "on: [issues, issue_comment]
jobs:
  gemini:
    runs-on: ubuntu-latest
    if: contains(fromJSON('[\"OWNER\"]'), github.event.comment.author_association)
    steps:
      - uses: google-github-actions/run-gemini-cli@v0
",
        );
        let events = external_events(&workflow.model.on.events);
        let open =
            open_trigger(&agent_steps(&workflow)[0], &events, UpstreamOrigin::Anyone).unwrap();
        assert_eq!(open.events, [ExternalEvent::Issues]);
    }

    #[test]
    fn a_constant_false_condition_means_the_step_never_runs() {
        let never = |condition: &str| {
            let workflow = parsed(&format!(
                "on: issues
jobs:
  gemini:
    runs-on: ubuntu-latest
    steps:
      - uses: google-github-actions/run-gemini-cli@v0
        if: {condition}
"
            ));
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
    fn adding_a_label_that_needs_triage_restricts_who_can_start_it() {
        for (events, condition) in [
            ("issues", "github.event.label.name == 'agent:gemini'"),
            (
                "pull_request_target",
                "${{ 'safe to test' == github.event.label.name }}",
            ),
        ] {
            assert_eq!(gemini_on(events, condition), None, "{condition}");
        }
        assert!(
            outside_on(
                "[workflow_run, issues]",
                "github.event.label.name == 'agent'"
            )
            .iter()
            .all(|event| *event != "workflow_run")
        );
    }

    #[test]
    fn a_label_that_stays_on_the_issue_or_is_missing_leaves_it_open() {
        for (events, condition) in [
            (
                "[issues, issue_comment]",
                "contains(github.event.issue.labels.*.name, 'gemini')",
            ),
            (
                "pull_request_target",
                "contains(github.event.pull_request.labels.*.name, 'gemini')",
            ),
            ("issues", "github.event.label.name != 'skip'"),
            (
                "issues",
                "github.event.label.name == 'gemini' || github.event.action == 'opened'",
            ),
        ] {
            assert!(gemini_on(events, condition).is_some(), "{condition}");
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
        let workflow = parsed(&format!(
            "on: {events}
jobs:
  gemini:
    runs-on: ubuntu-latest
    if: {condition}
    steps:
      - uses: google-github-actions/run-gemini-cli@v0
"
        ));
        let steps = agent_steps(&workflow);
        outside_events(
            &external_events(&workflow.model.on.events),
            upstream_origin(&steps[0], UpstreamOrigin::Anyone),
        )
        .into_iter()
        .map(ExternalEvent::name)
        .collect()
    }

    #[test]
    fn workflow_run_from_writer_only_upstream_events_is_not_an_outside_event() {
        for (events, condition) in [
            (
                "workflow_run",
                "github.event.workflow_run.event == 'workflow_dispatch'",
            ),
            (
                "workflow_run",
                "github.event.workflow_run.event == 'workflow_dispatch' || github.event.workflow_run.event == 'schedule'",
            ),
            (
                "workflow_run",
                "github.event.workflow_run.event == 'release'",
            ),
            (
                "workflow_run",
                "contains(fromJSON('[\"schedule\", \"Workflow_Dispatch\"]'), github.event.workflow_run.event)",
            ),
            (
                "workflow_run",
                "github.event.workflow_run.event == 'schedule' || false",
            ),
            (
                "[workflow_run, push]",
                "github.event_name != 'workflow_run' || github.event.workflow_run.event == 'workflow_dispatch'",
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
    fn workflow_run_whose_upstream_is_restricted_still_lets_outside_text_in() {
        for condition in [
            "github.event.workflow_run.event == 'push'",
            "github.actor == 'dependabot[bot]'",
            "(github.event.workflow_run.event == 'pull_request_target' && github.actor == 'octocat') || github.event.workflow_run.event == 'push'",
            "contains(fromJSON('[\"push\", \"schedule\"]'), github.event.workflow_run.event)",
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

    fn parsed(source: &str) -> Workflow {
        Workflow::parse(Path::new("test.yml"), source.to_string()).unwrap()
    }

    #[test]
    fn upstream_triggers_decide_who_can_start_the_upstream() {
        for (on, origin) in [
            ("workflow_dispatch", UpstreamOrigin::Writers),
            (
                "[workflow_dispatch, release, repository_dispatch]",
                UpstreamOrigin::Writers,
            ),
            ("{schedule: [{cron: '0 0 * * *'}]}", UpstreamOrigin::Writers),
            ("{push: {tags: ['*']}}", UpstreamOrigin::Restricted),
            ("[push, workflow_dispatch]", UpstreamOrigin::Restricted),
            ("[push, pull_request]", UpstreamOrigin::Anyone),
            ("issues", UpstreamOrigin::Anyone),
            ("{workflow_run: {workflows: [CI]}}", UpstreamOrigin::Anyone),
        ] {
            let upstream = parsed(&format!("name: Up\non: {on}\njobs: {{}}\n"));
            let downstream = parsed("on: {workflow_run: {workflows: [Up]}}\njobs: {}\n");
            assert_eq!(
                Upstreams::new([&upstream]).origin_of(&downstream),
                origin,
                "{on}"
            );
        }
    }

    #[test]
    fn upstream_names_are_matched_exactly_and_the_widest_one_wins() {
        let release = parsed("name: Release Notes\non: {push: {tags: ['*']}}\njobs: {}\n");
        let manual = parsed("name: Manual\non: workflow_dispatch\njobs: {}\n");
        let upstreams = Upstreams::new([&release, &manual]);
        for (workflows, origin) in [
            ("[Manual]", UpstreamOrigin::Writers),
            ("[Manual, Release Notes]", UpstreamOrigin::Restricted),
            ("[Manual, Missing]", UpstreamOrigin::Anyone),
            ("[manual]", UpstreamOrigin::Anyone),
            ("['Release*']", UpstreamOrigin::Anyone),
        ] {
            let downstream = parsed(&format!(
                "on: {{workflow_run: {{workflows: {workflows}}}}}\njobs: {{}}\n"
            ));
            assert_eq!(upstreams.origin_of(&downstream), origin, "{workflows}");
        }
    }

    #[test]
    fn upstreams_sharing_a_name_keep_the_widest_and_stay_in_their_directory() {
        let at = |path: &str, source: &str| {
            Workflow::parse(Path::new(path), source.to_string()).unwrap()
        };
        let manual = at(
            "a/manual.yml",
            "name: CI\non: workflow_dispatch\njobs: {}\n",
        );
        let forks = at("a/forks.yml", "name: CI\non: pull_request\njobs: {}\n");
        let elsewhere = at(
            "b/manual.yml",
            "name: Notes\non: workflow_dispatch\njobs: {}\n",
        );
        let empty = at("a/empty.yml", "name: Empty\non: {}\njobs: {}\n");
        let upstreams = Upstreams::new([&manual, &forks, &elsewhere, &empty]);
        for (workflows, origin) in [
            ("[CI]", UpstreamOrigin::Anyone),
            ("[Notes]", UpstreamOrigin::Anyone),
            ("[Empty]", UpstreamOrigin::Anyone),
        ] {
            let downstream = at(
                "a/down.yml",
                &format!("on: {{workflow_run: {{workflows: {workflows}}}}}\njobs: {{}}\n"),
            );
            assert_eq!(upstreams.origin_of(&downstream), origin, "{workflows}");
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
        let workflow = parsed(
            "on: [push, pull_request, workflow_dispatch, issue_comment, pull_request_target]
jobs: {}
",
        );
        assert_eq!(
            external_events(&workflow.model.on.events),
            [
                ExternalEvent::IssueComment,
                ExternalEvent::PullRequestTarget
            ]
        );
    }
}
