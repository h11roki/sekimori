use github_actions_models::common::expr::LoE;
use github_actions_models::workflow::job::{ParallelStep, RunStep, Step, UsesStep};
use yamlpath::{Component, Route, route};

use super::{ExternalEvent, Input, lookup, lookup_in};
use crate::agent::{Agent, AgentStep, is_root_action};
use crate::workflow::Workflow;

const CHECKOUT_ACTION: &str = "actions/checkout";
const CHECKOUT_TARGETS: [&str; 2] = ["ref", "repository"];
const ROOT_PATHS: [&str; 3] = ["", ".", "./"];
const HEAD_CONTEXTS: [&str; 3] = [
    "github.event.pull_request.head.",
    "github.head_ref",
    "github.event.workflow_run.head_",
];
const PULL_REFSPEC: &str = "pull/";
const PULL_REFSPEC_HEADS: [&str; 2] = ["head", "merge"];
const GH_PR_CHECKOUT: &str = "gh pr checkout";
const GIT_COMMANDS: [&str; 5] = [
    "git checkout",
    "git switch",
    "git fetch",
    "git pull",
    "git reset",
];
const GEMINI_TRUST_VARIABLE: &str = "GEMINI_CLI_TRUST_WORKSPACE";

#[derive(Debug)]
pub struct HeadCheckout<'w> {
    pub setting: String,
    pub route: Route<'w>,
}

pub fn head_checkout<'w>(
    workflow: &'w Workflow,
    step: &AgentStep<'w>,
    events: &[ExternalEvent],
) -> Option<HeadCheckout<'w>> {
    let privileged = events.iter().any(|event| {
        matches!(
            event,
            ExternalEvent::PullRequestTarget
                | ExternalEvent::WorkflowRun
                | ExternalEvent::IssueComment
        )
    });
    if !privileged {
        return None;
    }
    let loads = match step.agent {
        Agent::ClaudeCode => events.contains(&ExternalEvent::WorkflowRun).then_some(
            "on workflow_run claude-code-action keeps the PR's .claude/ and CLAUDE.md",
        ),
        Agent::GeminiCli => gemini_loads_workspace_settings(workflow, step).then_some(
            "with no `settings` and GEMINI_CLI_TRUST_WORKSPACE=true, Gemini CLI loads the PR's .gemini/settings.json",
        ),
        Agent::Codex => None,
    }?;
    earlier_steps(step)
        .into_iter()
        .find_map(|(route, earlier)| match earlier {
            Earlier::Uses(uses) => checkout_action(route, uses),
            Earlier::Run(run) => checkout_command(route, run),
        })
        .map(|(checkout, route)| HeadCheckout {
            setting: format!("{checkout}, and {loads}"),
            route,
        })
}

enum Earlier<'w> {
    Uses(&'w UsesStep),
    Run(&'w RunStep),
}

fn earlier_steps<'w>(agent: &AgentStep<'w>) -> Vec<(Route<'w>, Earlier<'w>)> {
    let steps = route!["jobs", agent.job_id, "steps"];
    let mut found = Vec::new();
    for (index, step) in agent.job.steps[..agent.index].iter().enumerate() {
        match step {
            Step::Uses(uses) => found.push((steps.with_key(index), Earlier::Uses(uses))),
            Step::Run(run) => found.push((steps.with_key(index), Earlier::Run(run))),
            Step::Parallel { parallel } => {
                for (inner, step) in parallel.iter().enumerate() {
                    let route = steps.with_keys([
                        Component::from(index),
                        Component::from("parallel"),
                        Component::from(inner),
                    ]);
                    found.push(match step {
                        ParallelStep::Uses(uses) => (route, Earlier::Uses(uses)),
                        ParallelStep::Run(run) => (route, Earlier::Run(run)),
                    });
                }
            }
            Step::Wait { .. } | Step::WaitAll { .. } | Step::Cancel { .. } => {}
        }
    }
    found
}

fn checkout_action<'w>(route: Route<'w>, step: &'w UsesStep) -> Option<(String, Route<'w>)> {
    if !is_root_action(&step.uses, CHECKOUT_ACTION) {
        return None;
    }
    let into_root = match lookup_in(&step.with, "path") {
        Input::Missing => true,
        Input::Text { value, .. } => ROOT_PATHS.contains(&value.trim()),
        Input::Unreadable => false,
    };
    if !into_root {
        return None;
    }
    CHECKOUT_TARGETS.into_iter().find_map(|name| {
        let Input::Text { key, value } = lookup_in(&step.with, name) else {
            return None;
        };
        names_head(value).then(|| {
            (
                format!(
                    "{} checks out the PR head with `{key}: {value}`",
                    step.uses.raw()
                ),
                route.with_keys([Component::from("with"), Component::from(key)]),
            )
        })
    })
}

fn checkout_command<'w>(route: Route<'w>, step: &'w RunStep) -> Option<(String, Route<'w>)> {
    let script = normalized(&step.run);
    let env_values: Vec<String> = match &step.shared.env {
        LoE::Literal(env) => env
            .values()
            .map(|value| normalized(&value.to_string()))
            .collect(),
        LoE::Expr(_) => Vec::new(),
    };
    let fetches_head = script.contains(&normalized(GH_PR_CHECKOUT))
        || (GIT_COMMANDS
            .iter()
            .any(|command| script.contains(&normalized(command)))
            && std::iter::once(&script)
                .chain(&env_values)
                .any(|text| names_head(text)));
    fetches_head.then(|| {
        (
            "a `run` step checks out the PR head".to_string(),
            route.with_key("run"),
        )
    })
}

fn names_head(text: &str) -> bool {
    let text = normalized(text);
    HEAD_CONTEXTS.iter().any(|context| text.contains(context))
        || text.match_indices(PULL_REFSPEC).any(|(start, _)| {
            text[start + PULL_REFSPEC.len()..]
                .split_once('/')
                .is_some_and(|(_, rest)| {
                    PULL_REFSPEC_HEADS.iter().any(|head| rest.starts_with(head))
                })
        })
}

fn normalized(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn gemini_loads_workspace_settings(workflow: &Workflow, step: &AgentStep) -> bool {
    let without_settings = match lookup(step, "settings") {
        Input::Missing => true,
        Input::Text { value, .. } => value.trim().is_empty(),
        Input::Unreadable => false,
    };
    without_settings
        && [&step.step.shared.env, &step.job.env, &workflow.model.env]
            .into_iter()
            .find_map(|env| match env {
                LoE::Literal(env) => env
                    .get(GEMINI_TRUST_VARIABLE)
                    .map(|value| value.to_string() == "true"),
                LoE::Expr(_) => Some(false),
            })
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::agent::agent_steps;
    use crate::facts::external_events;

    fn found(source: &str) -> Option<String> {
        let workflow = Workflow::parse(Path::new("test.yml"), source.to_string()).unwrap();
        let events = external_events(&workflow.model.on.events);
        let steps = agent_steps(&workflow);
        head_checkout(&workflow, &steps[0], &events).map(|checkout| {
            let (line, _) = workflow.position(&checkout.route).unwrap();
            format!("{line}: {}", checkout.setting)
        })
    }

    fn claude_after(on: &str, steps: &str) -> Option<String> {
        found(&format!(
            "on: {on}
jobs:
  agent:
    runs-on: ubuntu-latest
    steps:
{steps}      - uses: anthropics/claude-code-action@v1
"
        ))
    }

    fn gemini_after(env: &str, with: &str) -> Option<String> {
        found(&format!(
            "on: pull_request_target
{env}jobs:
  agent:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
        with:
          ref: ${{{{ github.event.pull_request.head.sha }}}}
      - uses: google-github-actions/run-gemini-cli@v0
{with}"
        ))
    }

    #[test]
    fn claude_on_workflow_run_after_a_head_checkout_is_found() {
        assert_eq!(
            claude_after(
                "workflow_run",
                "      - uses: actions/checkout@v6
        with:
          ref: ${{ github.event.workflow_run.head_branch }}
",
            ),
            Some(
                "8: actions/checkout@v6 checks out the PR head with `ref: ${{ github.event.workflow_run.head_branch }}`, and on workflow_run claude-code-action keeps the PR's .claude/ and CLAUDE.md"
                    .to_string()
            )
        );
    }

    #[test]
    fn claude_restores_its_settings_on_pull_request_events() {
        let checkout = "      - uses: actions/checkout@v6
        with:
          ref: ${{ github.event.pull_request.head.sha }}
";
        assert_eq!(claude_after("pull_request_target", checkout), None);
        assert_eq!(claude_after("issue_comment", checkout), None);
    }

    #[test]
    fn only_privileged_triggers_count() {
        let checkout = "      - uses: actions/checkout@v6
        with:
          ref: ${{ github.event.workflow_run.head_sha }}
";
        assert!(claude_after("[workflow_run, push]", checkout).is_some());
        assert_eq!(claude_after("[pull_request, push]", checkout), None);
    }

    #[test]
    fn checkouts_of_the_base_into_a_subdirectory_or_after_the_agent_are_not_found() {
        assert_eq!(
            claude_after(
                "workflow_run",
                "      - uses: actions/checkout@v6
      - uses: actions/checkout@v6
        with:
          ref: ${{ github.event.workflow_run.head_sha }}
          path: pr
",
            ),
            None
        );
        assert_eq!(
            found(
                "on: workflow_run
jobs:
  agent:
    runs-on: ubuntu-latest
    steps:
      - uses: anthropics/claude-code-action@v1
      - uses: actions/checkout@v6
        with:
          ref: ${{ github.event.workflow_run.head_sha }}
"
            ),
            None
        );
    }

    #[test]
    fn head_repository_merge_refs_and_root_paths_count() {
        let cases = [
            "repository: ${{ github.event.workflow_run.head_repository.full_name }}",
            "ref: refs/pull/${{ github.event.workflow_run.pull_requests[0].number }}/merge",
            "ref: ${{ github.head_ref }}\n          path: ./",
            "REF: ${{ GitHub.Event.Workflow_Run.Head_SHA }}",
        ];
        for with in cases {
            let steps =
                format!("      - uses: Actions/Checkout@v6\n        with:\n          {with}\n");
            assert!(claude_after("workflow_run", &steps).is_some(), "{with}");
        }
        let steps = "      - uses: actions/checkout@v6
        with:
          ref: ${{ steps.pr.outputs.sha }}
";
        assert_eq!(claude_after("workflow_run", steps), None);
    }

    #[test]
    fn run_steps_that_fetch_the_head_are_found() {
        let cases = [
            "run: gh pr checkout ${{ github.event.workflow_run.pull_requests[0].number }}",
            "run: git fetch origin pull/${{ env.PR }}/head && git checkout FETCH_HEAD",
            "run: git checkout \"$HEAD\"\n        env:\n          HEAD: ${{ github.event.workflow_run.head_sha }}",
        ];
        for run in cases {
            let steps = format!("      - {run}\n");
            assert_eq!(
                claude_after("workflow_run", &steps),
                Some("6: a `run` step checks out the PR head, and on workflow_run claude-code-action keeps the PR's .claude/ and CLAUDE.md".to_string()),
                "{run}"
            );
        }
        let unrelated = [
            "run: echo ${{ github.event.workflow_run.head_sha }}",
            "run: git checkout -b fix",
            "run: git pull origin main",
        ];
        for run in unrelated {
            let steps = format!("      - {run}\n");
            assert_eq!(claude_after("workflow_run", &steps), None, "{run}");
        }
    }

    #[test]
    fn steps_in_an_earlier_parallel_block_count() {
        assert!(
            claude_after(
                "workflow_run",
                "      - parallel:
          - run: echo start
          - uses: actions/checkout@v6
            with:
              ref: ${{ github.event.workflow_run.head_sha }}
",
            )
            .is_some()
        );
    }

    #[test]
    fn gemini_loads_the_head_settings_only_when_it_trusts_the_workspace() {
        assert_eq!(
            gemini_after(
                "",
                "        env:
          GEMINI_CLI_TRUST_WORKSPACE: 'true'
"
            ),
            Some("8: actions/checkout@v6 checks out the PR head with `ref: ${{ github.event.pull_request.head.sha }}`, and with no `settings` and GEMINI_CLI_TRUST_WORKSPACE=true, Gemini CLI loads the PR's .gemini/settings.json".to_string())
        );
        assert!(gemini_after("env:\n  GEMINI_CLI_TRUST_WORKSPACE: true\n", "").is_some());
        assert_eq!(gemini_after("", ""), None);
        assert_eq!(
            gemini_after(
                "env:\n  GEMINI_CLI_TRUST_WORKSPACE: 'true'\n",
                "        env:
          GEMINI_CLI_TRUST_WORKSPACE: ${{ vars.TRUST }}
"
            ),
            None
        );
        assert_eq!(
            gemini_after(
                "env:\n  GEMINI_CLI_TRUST_WORKSPACE: 'true'\n",
                "        with:
          settings: '{}'
"
            ),
            None
        );
    }

    #[test]
    fn codex_does_not_load_untrusted_project_settings() {
        assert_eq!(
            found(
                "on: pull_request_target
jobs:
  agent:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
        with:
          ref: ${{ github.event.pull_request.head.sha }}
      - uses: openai/codex-action@v1
"
            ),
            None
        );
    }
}
