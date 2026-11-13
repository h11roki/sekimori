use github_actions_models::common::Env;
use github_actions_models::common::expr::LoE;
use github_actions_models::workflow::job::{RunStep, UsesStep};
use yamlpath::{Component, Route};

use super::{ExternalEvent, Input, lookup, lookup_in};
use crate::agent::{Agent, AgentStep, JobStep, is_root_action, job_steps};
use crate::workflow::Workflow;

const CHECKOUT_ACTION: &str = "actions/checkout";
const CHECKOUT_TARGETS: [&str; 2] = ["ref", "repository"];
const WORKSPACE_EXPRESSION: &str = "${{github.workspace}}";
const HEAD_CONTEXTS: [&str; 4] = [
    "github.event.pull_request.head.",
    "github.event.pull_request.merge_commit_sha",
    "github.head_ref",
    "github.event.workflow_run.head_",
];
const WORKFLOW_RUN_PULL_REQUESTS: &str = "github.event.workflow_run.pull_requests[";
const PULL_REFSPEC: &str = "pull/";
const PULL_REFSPEC_HEADS: [&str; 2] = ["head", "merge"];
const GIT: &str = "git";
const GIT_OPTIONS_WITH_VALUE: [&str; 2] = ["-C", "-c"];
const GIT_SUBCOMMANDS: [&str; 5] = ["checkout", "switch", "fetch", "pull", "reset"];
const GH_PR_CHECKOUT: [&str; 3] = ["gh", "pr", "checkout"];
const SHELL_SEPARATORS: &str = ";&|()`";
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
    let inherited_env = [&step.job.env, &workflow.model.env];
    let mut head = None;
    for (index, route, earlier) in job_steps(step.job_id, step.job) {
        if index >= step.index {
            break;
        }
        match earlier {
            JobStep::Uses(uses) => {
                if let Some(checkout) = root_checkout(route, uses) {
                    head = checkout;
                }
            }
            JobStep::Run(run) => {
                if head.is_none() && fetches_head(run, &inherited_env) {
                    head = Some((
                        "a `run` step checks out the PR head".to_string(),
                        route.with_key("run"),
                    ));
                }
            }
        }
    }
    head.map(|(checkout, route)| HeadCheckout {
        setting: format!("{checkout}, and {loads}"),
        route,
    })
}

fn root_checkout<'w>(route: Route<'w>, step: &'w UsesStep) -> Option<Option<(String, Route<'w>)>> {
    if !is_root_action(&step.uses, CHECKOUT_ACTION) {
        return None;
    }
    let into_root = match lookup_in(&step.with, "path") {
        Input::Missing => true,
        Input::Text { value, .. } => is_workspace_root(value),
        Input::Unreadable => false,
    };
    if !into_root {
        return None;
    }
    Some(CHECKOUT_TARGETS.into_iter().find_map(|name| {
        let Input::Text { key, value } = lookup_in(&step.with, name) else {
            return None;
        };
        names_head(&normalized(value)).then(|| {
            (
                format!(
                    "{} checks out the PR head with `{key}: {value}`",
                    step.uses.raw()
                ),
                route.with_keys([Component::from("with"), Component::from(key)]),
            )
        })
    }))
}

fn is_workspace_root(path: &str) -> bool {
    normalized(path)
        .replace(WORKSPACE_EXPRESSION, ".")
        .split('/')
        .all(|component| component.is_empty() || component == ".")
}

fn fetches_head(step: &RunStep, inherited_env: &[&LoE<Env>]) -> bool {
    let words: Vec<&str> = step
        .run
        .split(|c: char| c.is_whitespace() || SHELL_SEPARATORS.contains(c))
        .filter(|word| !word.is_empty())
        .collect();
    let gh_pr_checkout = words
        .windows(GH_PR_CHECKOUT.len())
        .any(|window| window == GH_PR_CHECKOUT);
    let git_fetch = (0..words.len()).any(|start| {
        if program_name(words[start]) != GIT {
            return false;
        }
        let mut next = start + 1;
        while let Some(word) = words.get(next).filter(|word| word.starts_with('-')) {
            next += if GIT_OPTIONS_WITH_VALUE.contains(word) {
                2
            } else {
                1
            };
        }
        words
            .get(next)
            .is_some_and(|subcommand| GIT_SUBCOMMANDS.contains(subcommand))
    });
    gh_pr_checkout
        || (git_fetch
            && std::iter::once(&step.shared.env)
                .chain(inherited_env.iter().copied())
                .filter_map(|env| match env {
                    LoE::Literal(env) => Some(env),
                    LoE::Expr(_) => None,
                })
                .flat_map(|env| env.values().map(|value| value.to_string()))
                .chain(std::iter::once(step.run.clone()))
                .any(|text| names_head(&normalized(&text))))
}

fn program_name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

fn names_head(normalized: &str) -> bool {
    HEAD_CONTEXTS
        .iter()
        .any(|context| normalized.contains(context))
        || followed_by(normalized, WORKFLOW_RUN_PULL_REQUESTS, ']', &[".head."])
        || followed_by(normalized, PULL_REFSPEC, '/', &PULL_REFSPEC_HEADS)
}

fn followed_by(text: &str, prefix: &str, separator: char, suffixes: &[&str]) -> bool {
    text.match_indices(prefix).any(|(start, _)| {
        text[start + prefix.len()..]
            .split_once(separator)
            .is_some_and(|(_, rest)| suffixes.iter().any(|suffix| rest.starts_with(suffix)))
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
    fn pull_request_lists_merge_commits_and_workspace_paths_name_the_head() {
        let cases = [
            "ref: ${{ github.event.workflow_run.pull_requests[0].head.sha }}",
            "ref: ${{ github.event.pull_request.merge_commit_sha }}",
            "ref: ${{ github.event.workflow_run.head_sha }}\n          path: ${{ github.workspace }}",
            "ref: ${{ github.event.workflow_run.head_sha }}\n          path: ./.",
        ];
        for with in cases {
            let steps =
                format!("      - uses: actions/checkout@v6\n        with:\n          {with}\n");
            assert!(claude_after("workflow_run", &steps).is_some(), "{with}");
        }
        let number_only = "      - uses: actions/checkout@v6
        with:
          ref: ${{ github.event.workflow_run.pull_requests[0].number }}
";
        assert_eq!(claude_after("workflow_run", number_only), None);
    }

    #[test]
    fn a_later_checkout_of_the_base_puts_the_trusted_settings_back() {
        assert_eq!(
            claude_after(
                "workflow_run",
                "      - uses: actions/checkout@v6
        with:
          ref: ${{ github.event.workflow_run.head_sha }}
      - run: make build
      - uses: actions/checkout@v6
",
            ),
            None
        );
    }

    #[test]
    fn the_first_step_that_brings_the_head_is_reported() {
        assert_eq!(
            claude_after(
                "workflow_run",
                "      - uses: actions/checkout@v6
        with:
          ref: ${{ github.event.workflow_run.head_branch }}
      - run: git checkout -b \"fix-$HEAD\"
        env:
          HEAD: ${{ github.event.workflow_run.head_branch }}
",
            )
            .map(|found| found[..2].to_string()),
            Some("8:".to_string())
        );
    }

    #[test]
    fn git_options_and_inherited_env_are_read_but_joined_words_are_not() {
        let job_env = found(
            "on: workflow_run
env:
  HEAD_SHA: ${{ github.event.workflow_run.head_sha }}
jobs:
  agent:
    runs-on: ubuntu-latest
    steps:
      - run: git -C . -c protocol.version=2 fetch origin \"$HEAD_SHA\"
      - uses: anthropics/claude-code-action@v1
",
        );
        assert!(job_env.is_some());
        let joined = "      - run: |
          echo digit
          checkout ${{ github.event.workflow_run.head_sha }}
";
        assert_eq!(claude_after("workflow_run", joined), None);
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
