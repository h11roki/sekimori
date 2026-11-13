use github_actions_models::workflow::job::{RunStep, UsesStep};
use yamlpath::{Component, Route};

use super::{
    ENV_CONTEXT, EXPRESSION_START, ExternalEvent, Input, NearestEnv, SETTINGS_INPUT, agent_env,
    before_comment, ends_word, expression_spans, lookup, lookup_in, nearest_env, program_name,
    references_variable,
};
use crate::agent::{Agent, AgentStep, JobStep, is_root_action, job_steps};
use crate::workflow::Workflow;

const CHECKOUT_ACTION: &str = "actions/checkout";
const WORKSPACE_EXPRESSION: &str = "${{github.workspace}}";
const BASE_REPOSITORY_EXPRESSION: &str = "${{github.repository}}";
const BASE_REFERENCES: [&str; 8] = [
    "github.sha",
    "github.ref",
    "github.base_ref",
    "github.head_ref",
    "github.event.pull_request.base.",
    "github.event.pull_request.head.ref",
    "github.event.workflow_run.head_branch",
    "github.event.repository.default_branch",
];
const PROMPT_INPUT: &str = "prompt";
const BASE_WORKSPACE_EVENTS: [ExternalEvent; 4] = [
    ExternalEvent::Issues,
    ExternalEvent::IssueComment,
    ExternalEvent::Discussion,
    ExternalEvent::DiscussionComment,
];
const LINE_CONTINUATION: &str = "\\\n";
const SHELL_COMMAND_FLAG: &str = "-c";
const WILDCARD: &str = "*";
const HEAD_COMMITS: [&str; 3] = [
    "github.event.pull_request.head.sha",
    "github.event.pull_request.merge_commit_sha",
    "github.event.workflow_run.head_sha",
];
const HEAD_REPOSITORIES: [&str; 2] = [
    "github.event.pull_request.head.repo.",
    "github.event.workflow_run.head_repository.",
];
const WORKFLOW_RUN_PULL_REQUESTS: &str = "github.event.workflow_run.pull_requests[";
const PULL_REQUEST_HEAD_COMMIT: &str = ".head.sha";
const PULL_REQUEST_HEAD_REPOSITORY: &str = ".head.repo.";
const PULL_REFSPEC: &str = "pull/";
const PULL_REFSPEC_HEADS: [&str; 2] = ["head", "merge"];
const GIT: &str = "git";
const GIT_OPTIONS_WITH_VALUE: [&str; 4] = ["-C", "-c", "--git-dir", "--work-tree"];
const GIT_FETCH: &str = "fetch";
const GIT_WORKTREE_COMMANDS: [&str; 8] = [
    "checkout",
    "switch",
    "reset",
    "pull",
    "merge",
    "rebase",
    "cherry-pick",
    "restore",
];
const GH_PR_CHECKOUT: [&str; 3] = ["gh", "pr", "checkout"];
const COMMAND_SEPARATORS: &str = ";&|()`";
const WORD_SEPARATORS: &str = "\"'";
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
    let loads = match step.agent {
        Agent::ClaudeCode => events.contains(&ExternalEvent::WorkflowRun).then_some(
            "on workflow_run claude-code-action keeps the PR's .claude/ and CLAUDE.md",
        ),
        Agent::GeminiCli => (privileged && gemini_loads_workspace_settings(workflow, step)).then_some(
            "with no `settings` and GEMINI_CLI_TRUST_WORKSPACE=true, Gemini CLI loads the PR's .gemini/settings.json",
        ),
        Agent::Codex => None,
    }?;
    let mut head = None;
    for (index, route, earlier) in job_steps(step.job_id, step.job) {
        if index >= step.index {
            break;
        }
        match earlier {
            JobStep::Uses(uses) => match root_checkout(uses) {
                RootCheckout::Untouched => {}
                RootCheckout::Base => head = None,
                RootCheckout::Head(key, value) => {
                    head = head.or(Some((
                        format!(
                            "{} checks out the PR head with `{key}: {value}`",
                            uses.uses.raw()
                        ),
                        route.with_keys([Component::from("with"), Component::from(key)]),
                    )));
                }
            },
            JobStep::Run(run) => {
                if head.is_none()
                    && checks_out_head(run, &run_head_variables(workflow, step, run, &route))
                {
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

#[derive(Debug, PartialEq, Eq)]
pub enum OutsideText {
    MayReach,
    NeverReaches,
    ThroughCheckout(String),
}

pub fn outside_text(
    workflow: &Workflow,
    step: &AgentStep,
    events: &[ExternalEvent],
) -> OutsideText {
    if step.agent != Agent::GeminiCli
        || !blank_input(step, PROMPT_INPUT)
        || !events
            .iter()
            .all(|event| BASE_WORKSPACE_EVENTS.contains(event))
    {
        return OutsideText::MayReach;
    }
    let source = job_steps(step.job_id, step.job)
        .into_iter()
        .take_while(|(index, ..)| *index < step.index)
        .find_map(|(_, route, earlier)| match earlier {
            JobStep::Uses(uses) => (is_root_action(&uses.uses, CHECKOUT_ACTION)
                && (matches!(lookup_in(&uses.with, "ref"), Input::Unreadable)
                    || !matches!(checkout_of(uses), RootCheckout::Base)))
            .then(|| format!("`{}`", uses.uses.raw())),
            JobStep::Run(run) => {
                (checks_out_head(run, &run_head_variables(workflow, step, run, &route))
                    || names_head_commit(&normalized(&run.run)))
                .then(|| "a `run` step".to_string())
            }
        });
    match source {
        Some(source) => OutsideText::ThroughCheckout(format!(
            "with no `prompt`, Gemini CLI still reads GEMINI.md files that {source} may bring from a PR"
        )),
        None => OutsideText::NeverReaches,
    }
}

enum RootCheckout<'w> {
    Untouched,
    Base,
    Head(&'w str, &'w str),
}

fn root_checkout(step: &UsesStep) -> RootCheckout<'_> {
    let into_root = match lookup_in(&step.with, "path") {
        Input::Missing => true,
        Input::Text { value, .. } => is_workspace_root(value),
        Input::Unreadable => false,
    };
    if into_root {
        checkout_of(step)
    } else {
        RootCheckout::Untouched
    }
}

fn checkout_of(step: &UsesStep) -> RootCheckout<'_> {
    if !is_root_action(&step.uses, CHECKOUT_ACTION) {
        return RootCheckout::Untouched;
    }
    let text = |name| match lookup_in(&step.with, name) {
        Input::Text { key, value } => Some((key, value)),
        Input::Missing | Input::Unreadable => None,
    };
    let repository = text("repository");
    let reference = text("ref");
    if let Some((key, value)) = repository
        .filter(|(_, value)| names_head_repository(&normalized(value)))
        .or(reference.filter(|(_, value)| names_head_commit(&normalized(value))))
    {
        return RootCheckout::Head(key, value);
    }
    let base_repository =
        repository.is_none_or(|(_, value)| normalized(value) == BASE_REPOSITORY_EXPRESSION);
    let base_reference = reference.is_none_or(|(_, value)| {
        let value = normalized(value);
        !value.contains(EXPRESSION_START)
            || BASE_REFERENCES
                .iter()
                .any(|context| value.contains(context))
    });
    if base_repository && base_reference {
        RootCheckout::Base
    } else {
        RootCheckout::Untouched
    }
}

fn is_workspace_root(path: &str) -> bool {
    normalized(path)
        .replace(WORKSPACE_EXPRESSION, ".")
        .split('/')
        .all(|component| component.is_empty() || component == ".")
}

fn run_head_variables<'w>(
    workflow: &'w Workflow,
    step: &AgentStep<'w>,
    run: &'w RunStep,
    route: &Route<'w>,
) -> Vec<&'w str> {
    head_variables(&nearest_env(
        workflow,
        step.job_id,
        step.job,
        &run.shared.env,
        route,
    ))
}

fn head_variables<'w>(env: &NearestEnv<'w>) -> Vec<&'w str> {
    env.variables
        .iter()
        .filter(|variable| names_head_commit(&normalized(&variable.value)))
        .map(|variable| variable.name)
        .collect()
}

fn checks_out_head(step: &RunStep, head_variables: &[&str]) -> bool {
    let script = flatten_expressions(&step.run.replace(LINE_CONTINUATION, " "));
    let mut fetched_head = false;
    let commands = script
        .lines()
        .map(before_comment)
        .flat_map(|line| line.split(|c: char| COMMAND_SEPARATORS.contains(c)));
    for command in commands {
        let words: Vec<&str> = command
            .split(|c: char| c.is_whitespace() || WORD_SEPARATORS.contains(c))
            .filter(|word| !word.is_empty())
            .collect();
        for position in program_positions(&words) {
            let program = &words[position..];
            if program.len() >= GH_PR_CHECKOUT.len()
                && program_name(program[0]) == GH_PR_CHECKOUT[0]
                && program[1..GH_PR_CHECKOUT.len()] == GH_PR_CHECKOUT[1..]
            {
                return true;
            }
            let Some((subcommand, arguments)) = git_subcommand(program) else {
                continue;
            };
            let names_head = names_head_commit(&normalized(&arguments.concat()))
                || arguments
                    .iter()
                    .any(|argument| refers_to_head_variable(argument, head_variables));
            if GIT_WORKTREE_COMMANDS.contains(&subcommand) && (names_head || fetched_head) {
                return true;
            }
            fetched_head |= subcommand == GIT_FETCH && names_head;
        }
    }
    false
}

fn flatten_expressions(text: &str) -> String {
    let mut flattened = String::new();
    let mut last = 0;
    for span in expression_spans(text) {
        flattened.push_str(&text[last..span.start]);
        flattened.extend(text[span.clone()].chars().filter(|c| {
            !c.is_whitespace() && !COMMAND_SEPARATORS.contains(*c) && !WORD_SEPARATORS.contains(*c)
        }));
        last = span.end;
    }
    flattened.push_str(&text[last..]);
    flattened
}

fn program_positions(words: &[&str]) -> Vec<usize> {
    let first = words.iter().position(|word| !is_assignment(word));
    let after_shell_flag = words
        .iter()
        .enumerate()
        .filter(|(_, word)| **word == SHELL_COMMAND_FLAG)
        .map(|(index, _)| index + 1)
        .filter(|index| *index < words.len());
    first.into_iter().chain(after_shell_flag).collect()
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

fn git_subcommand<'a>(words: &[&'a str]) -> Option<(&'a str, Vec<&'a str>)> {
    if program_name(words.first()?) != GIT {
        return None;
    }
    let mut next = 1;
    while let Some(word) = words.get(next).filter(|word| word.starts_with('-')) {
        next += if GIT_OPTIONS_WITH_VALUE.contains(word) {
            2
        } else {
            1
        };
    }
    let subcommand = words.get(next)?;
    Some((subcommand, words[next + 1..].to_vec()))
}

fn refers_to_head_variable(argument: &str, head_variables: &[&str]) -> bool {
    head_variables.iter().any(|name| {
        references_variable(argument, name)
            || ends_word(
                &normalized(argument),
                &format!("{ENV_CONTEXT}.{}", name.to_lowercase()),
            )
    })
}

fn names_head_commit(normalized: &str) -> bool {
    HEAD_COMMITS
        .iter()
        .any(|context| normalized.contains(context))
        || followed_by(
            normalized,
            WORKFLOW_RUN_PULL_REQUESTS,
            ']',
            &[PULL_REQUEST_HEAD_COMMIT],
        )
        || followed_by(normalized, PULL_REFSPEC, '/', &PULL_REFSPEC_HEADS)
}

fn names_head_repository(normalized: &str) -> bool {
    HEAD_REPOSITORIES
        .iter()
        .any(|context| normalized.contains(context))
        || followed_by(
            normalized,
            WORKFLOW_RUN_PULL_REQUESTS,
            ']',
            &[PULL_REQUEST_HEAD_REPOSITORY],
        )
}

fn followed_by(text: &str, prefix: &str, separator: char, suffixes: &[&str]) -> bool {
    text.match_indices(prefix).any(|(start, _)| {
        text[start + prefix.len()..]
            .split_once(separator)
            .is_some_and(|(segment, rest)| {
                segment != WILDCARD && suffixes.iter().any(|suffix| rest.starts_with(suffix))
            })
    })
}

fn normalized(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn blank_input(step: &AgentStep, name: &str) -> bool {
    match lookup(step, name) {
        Input::Missing => true,
        Input::Text { value, .. } => value.trim().is_empty(),
        Input::Unreadable => false,
    }
}

fn gemini_loads_workspace_settings(workflow: &Workflow, step: &AgentStep) -> bool {
    blank_input(step, SETTINGS_INPUT)
        && agent_env(workflow, step)
            .variables
            .iter()
            .find(|variable| variable.name == GEMINI_TRUST_VARIABLE)
            .is_some_and(|variable| variable.value == "true")
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
          ref: ${{ github.event.workflow_run.head_sha }}
",
            ),
            Some(
                "8: actions/checkout@v6 checks out the PR head with `ref: ${{ github.event.workflow_run.head_sha }}`, and on workflow_run claude-code-action keeps the PR's .claude/ and CLAUDE.md"
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
            "ref: ${{ github.event.pull_request.head.sha }}\n          path: ./",
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
          ref: ${{ github.event.workflow_run.head_sha }}
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
      - run: git -C . -c protocol.version=2 fetch origin \"$HEAD_SHA\" && git checkout FETCH_HEAD
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
    fn a_branch_name_counts_only_with_the_head_repository() {
        let branch_only = "      - uses: actions/checkout@v6
        with:
          ref: ${{ github.event.workflow_run.head_branch }}
";
        assert_eq!(claude_after("workflow_run", branch_only), None);
        let with_repository = "      - uses: actions/checkout@v6
        with:
          repository: ${{ github.event.workflow_run.head_repository.full_name }}
          ref: ${{ github.event.workflow_run.head_branch }}
";
        assert_eq!(
            claude_after("workflow_run", with_repository),
            Some("8: actions/checkout@v6 checks out the PR head with `repository: ${{ github.event.workflow_run.head_repository.full_name }}`, and on workflow_run claude-code-action keeps the PR's .claude/ and CLAUDE.md".to_string())
        );
        let run_branch = "      - run: git checkout \"$BRANCH\"
        env:
          BRANCH: ${{ github.event.workflow_run.head_branch }}
";
        assert_eq!(claude_after("workflow_run", run_branch), None);
    }

    #[test]
    fn fetching_without_changing_the_worktree_or_in_a_comment_is_not_a_checkout() {
        let cases = [
            "run: git fetch origin pull/${{ env.PR }}/head:pr && git diff main...pr",
            "run: |\n          # git checkout ${{ github.event.workflow_run.head_sha }}\n          make",
            "run: echo ${{ github.event.workflow_run.head_sha }}; git fetch --tags",
        ];
        for run in cases {
            let steps = format!("      - {run}\n");
            assert_eq!(claude_after("workflow_run", &steps), None, "{run}");
        }
    }

    #[test]
    fn quoted_commands_and_git_options_with_values_are_read() {
        let cases = [
            "run: bash -c \"gh pr checkout ${{ github.event.issue.number }}\"",
            "run: git --git-dir .git --work-tree . checkout ${{ github.event.workflow_run.head_sha }}",
            "run: git checkout ${{ env.HEAD }}\n        env:\n          HEAD: ${{ github.event.workflow_run.head_sha }}",
        ];
        for run in cases {
            let steps = format!("      - {run}\n");
            assert!(claude_after("workflow_run", &steps).is_some(), "{run}");
        }
        let longer_name = "      - run: git checkout \"$HEAD_OLD\"
        env:
          HEAD: ${{ github.event.workflow_run.head_sha }}
          HEAD_OLD: main
";
        assert_eq!(claude_after("workflow_run", longer_name), None);
    }

    #[test]
    fn merges_fallbacks_and_continued_lines_still_check_out_the_head() {
        let cases = [
            "run: git fetch origin pull/${{ github.event.workflow_run.pull_requests[0].number }}/head && git merge FETCH_HEAD",
            "run: git restore --source=${{ github.event.workflow_run.head_sha }} -- .",
            "run: git checkout ${{ inputs.sha || github.event.workflow_run.head_sha }}",
            "run: |\n          git checkout \\\n            ${{ github.event.workflow_run.head_sha }}",
        ];
        for run in cases {
            let steps = format!("      - {run}\n");
            assert!(claude_after("workflow_run", &steps).is_some(), "{run}");
        }
    }

    #[test]
    fn echoed_commands_wildcard_refspecs_and_overridden_variables_are_not_checkouts() {
        let cases = [
            "run: 'echo \"To test locally: git checkout ${{ github.event.workflow_run.head_sha }}\" >> $GITHUB_STEP_SUMMARY'",
            "run: echo \"run gh pr checkout 1\"",
            "run: git fetch origin '+refs/pull/*/head:refs/remotes/origin/pr/*' && git checkout main",
            "run: git checkout \"$HEAD\"\n        env:\n          HEAD: main",
        ];
        for run in cases {
            let steps = format!("      - {run}\n");
            assert_eq!(
                found(&format!(
                    "on: workflow_run
env:
  HEAD: ${{{{ github.event.workflow_run.head_sha }}}}
jobs:
  agent:
    runs-on: ubuntu-latest
    steps:
{steps}      - uses: anthropics/claude-code-action@v1
"
                )),
                None,
                "{run}"
            );
        }
    }

    #[test]
    fn only_a_readable_checkout_of_the_base_puts_the_settings_back() {
        let after_head = |second: &str| {
            claude_after(
                "workflow_run",
                &format!(
                    "      - uses: actions/checkout@v6
        with:
          ref: ${{{{ github.event.workflow_run.head_sha }}}}
      - uses: actions/checkout@v6
        with:
          {second}
"
                ),
            )
        };
        assert!(after_head("ref: ${{ steps.pr.outputs.sha }}").is_some());
        assert!(after_head("repository: org/tools").is_some());
        assert_eq!(
            after_head("ref: ${{ github.event.repository.default_branch }}"),
            None
        );
        assert_eq!(after_head("ref: main"), None);
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
    #[test]
    fn a_stray_quote_in_an_expression_does_not_swallow_later_lines() {
        let steps = "      - run: |
          echo ${{ github.event.pull_request.title || 'none }}
          git checkout ${{ github.event.workflow_run.head_sha }}
";
        assert!(claude_after("workflow_run", steps).is_some());
    }
}
