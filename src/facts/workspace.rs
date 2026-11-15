use yamlpath::Route;

use super::secrets::shell_secret;
use super::tools::gemini_may_edit;
use super::{constant_false, input, nearest_env, parse_condition, program_name, shell_programs};
use crate::agent::{Agent, AgentStep, JobStep, job_steps};
use crate::workflow::Workflow;

const RUN_KEY: &str = "run";
const CODEX_READ_ONLY: [(&str, &str); 2] = [
    ("sandbox", "read-only"),
    ("permission-profile", ":read-only"),
];
const WORKSPACE_PATH: &str = "./";
const SCRIPT_INTERPRETERS: [(&str, &[char]); 8] = [
    ("bash", &['c']),
    ("sh", &['c']),
    ("zsh", &['c']),
    ("python", &['c']),
    ("node", &['e', 'p']),
    ("ruby", &['e']),
    ("perl", &['e', 'E']),
    ("pwsh", &['c']),
];
const INLINE_CODE_LONG_FLAGS: [&str; 3] = ["--eval", "--print", "-Command"];
const MODULE_FLAG: &str = "-m";
const HEREDOC_OR_REDIRECT: char = '<';
const WORKSPACE_VARIABLES: [&str; 3] = [
    "$GITHUB_WORKSPACE/",
    "${GITHUB_WORKSPACE}/",
    "${{github.workspace}}/",
];
const PROJECT_RUNNERS: [&str; 9] = [
    "make",
    "pytest",
    "tox",
    "nox",
    "pre-commit",
    "gradle",
    "mvn",
    "rake",
    "npx",
];
const SUBCOMMAND_RUNNERS: [(&str, &[&str]); 10] = [
    (
        "npm",
        &[
            "run", "test", "install", "ci", "exec", "build", "start", "publish",
        ],
    ),
    (
        "pnpm",
        &[
            "run", "test", "install", "ci", "exec", "build", "start", "publish",
        ],
    ),
    (
        "yarn",
        &[
            "run", "test", "install", "ci", "exec", "build", "start", "publish",
        ],
    ),
    (
        "bun",
        &[
            "run", "test", "install", "ci", "exec", "build", "start", "publish",
        ],
    ),
    ("cargo", &["test", "build", "run", "publish"]),
    ("go", &["test", "build", "run", "generate"]),
    ("dotnet", &["test", "build", "run"]),
    ("bundle", &["exec"]),
    ("uv", &["run"]),
    ("poetry", &["run"]),
];

#[derive(Debug)]
pub struct AgentCode<'w> {
    pub setting: String,
    pub route: Route<'w>,
}

pub fn agent_code_with_secrets<'w>(
    workflow: &'w Workflow,
    step: &AgentStep<'w>,
) -> Option<AgentCode<'w>> {
    let edits_workspace = match step.agent {
        Agent::ClaudeCode => true,
        Agent::Codex => !CODEX_READ_ONLY.iter().any(|(name, read_only)| {
            input(step, name).is_some_and(|(_, value)| value.trim() == *read_only)
        }),
        Agent::GeminiCli => gemini_may_edit(step),
    };
    if !edits_workspace {
        return None;
    }
    job_steps(step.job_id, step.job)
        .into_iter()
        .filter(|(index, ..)| *index > step.index)
        .find_map(|(_, route, later)| {
            let JobStep::Run(run) = later else {
                return None;
            };
            let never_runs = run
                .shared
                .r#if
                .as_ref()
                .and_then(parse_condition)
                .is_some_and(|condition| constant_false(&condition));
            if never_runs {
                return None;
            }
            let command = shell_programs(&run.run)
                .iter()
                .find_map(|program| project_command(program))?;
            let env = nearest_env(workflow, step.job_id, step.job, &run.shared.env, &route);
            let secret = shell_secret(env, &run.run)?;
            Some(AgentCode {
                setting: format!(
                    "`{command}` in a later step runs files the agent can edit, with {secret}"
                ),
                route: route.with_key(RUN_KEY),
            })
        })
}

fn project_command(program: &[String]) -> Option<String> {
    let (first, arguments) = program.split_first()?;
    if first.starts_with(WORKSPACE_PATH) {
        return Some(first.clone());
    }
    let name = program_name(first).trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    let operand = arguments.iter().find(|argument| !argument.starts_with('-'));
    if let Some((_, inline_letters)) = SCRIPT_INTERPRETERS
        .iter()
        .find(|(interpreter, _)| *interpreter == name)
    {
        let inline_code = arguments.iter().any(|argument| {
            INLINE_CODE_LONG_FLAGS.contains(&argument.as_str())
                || argument
                    .strip_prefix('-')
                    .filter(|letters| !letters.starts_with('-'))
                    .is_some_and(|letters| letters.contains(*inline_letters))
        });
        if inline_code {
            return None;
        }
        if arguments.iter().any(|argument| argument == MODULE_FLAG) {
            return Some(format!("{name} {MODULE_FLAG}"));
        }
        let script = operand?;
        let in_workspace = !script.starts_with(['/', '$', HEREDOC_OR_REDIRECT])
            || WORKSPACE_VARIABLES
                .iter()
                .any(|variable| script.starts_with(variable));
        return in_workspace.then(|| format!("{name} {script}"));
    }
    if PROJECT_RUNNERS.contains(&name) {
        return Some(name.to_string());
    }
    let (_, subcommands) = SUBCOMMAND_RUNNERS
        .iter()
        .find(|(runner, _)| *runner == name)?;
    let subcommand = operand.filter(|subcommand| subcommands.contains(&subcommand.as_str()))?;
    Some(format!("{name} {subcommand}"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::agent::agent_steps;

    fn found(agent: &str, later: &str) -> Option<String> {
        let workflow = Workflow::parse(
            Path::new("test.yml"),
            format!(
                "on: issue_comment
jobs:
  agent:
    runs-on: ubuntu-latest
    steps:
      - uses: {agent}
{later}"
            ),
        )
        .unwrap();
        agent_code_with_secrets(&workflow, &agent_steps(&workflow)[0]).map(|found| {
            let (line, _) = workflow.position(&found.route).unwrap();
            format!("{line}: {}", found.setting)
        })
    }

    #[test]
    fn project_code_run_with_a_secret_after_an_editing_agent_is_found() {
        for (agent, later, expected) in [
            (
                "anthropics/claude-code-action@v1",
                "      - run: npm ci && npm test
        env:
          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}",
                "7: `npm ci` in a later step runs files the agent can edit, with secrets.NPM_TOKEN",
            ),
            (
                "openai/codex-action@v1",
                "      - run: |
          echo start
          ./scripts/release.sh --token \"${{ secrets.DEPLOY_TOKEN }}\"",
                "7: `./scripts/release.sh` in a later step runs files the agent can edit, with secrets.DEPLOY_TOKEN",
            ),
            (
                "anthropics/claude-code-action@v1",
                "      - run: bash \"$GITHUB_WORKSPACE/ci/run.sh\"
        env:
          TOKEN: ${{ secrets.TOKEN }}",
                "7: `bash $GITHUB_WORKSPACE/ci/run.sh` in a later step runs files the agent can edit, with secrets.TOKEN",
            ),
            (
                "google-github-actions/run-gemini-cli@v0",
                "      - run: python3 -m pytest
        env:
          API_KEY: ${{ secrets.API_KEY }}",
                "7: `python -m` in a later step runs files the agent can edit, with secrets.API_KEY",
            ),
            (
                "google-github-actions/run-gemini-cli@v0
        with:
          settings: '{\"tools\": {\"core\": [\"read_file\", \"replace\"]}}'",
                "      - run: CI=1 make check
        env:
          TOKEN: ${{ secrets.TOKEN }}",
                "9: `make` in a later step runs files the agent can edit, with secrets.TOKEN",
            ),
        ] {
            assert_eq!(found(agent, later).as_deref(), Some(expected), "{later}");
        }
    }

    #[test]
    fn steps_without_project_code_secrets_or_an_editing_agent_are_not_found() {
        for (agent, later) in [
            (
                "anthropics/claude-code-action@v1",
                "      - run: 'curl -H \"Authorization: $TOKEN\" https://example.com'
        env:
          TOKEN: ${{ secrets.TOKEN }}",
            ),
            (
                "anthropics/claude-code-action@v1",
                "      - run: npm test
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}",
            ),
            (
                "anthropics/claude-code-action@v1",
                "      - run: bash -c \"echo $TOKEN\"
        env:
          TOKEN: ${{ secrets.TOKEN }}",
            ),
            (
                "anthropics/claude-code-action@v1",
                "      - run: perl -0pe 's/a/b/' out.txt && node <<'JS' && bash $RUNNER_TEMP/x.sh && python /tmp/y.py
        env:
          TOKEN: ${{ secrets.TOKEN }}",
            ),
            (
                "anthropics/claude-code-action@v1",
                "      - if: false
        run: npm test
        env:
          TOKEN: ${{ secrets.TOKEN }}",
            ),
            (
                "openai/codex-action@v1
        with:
          sandbox: read-only",
                "      - run: npm test
        env:
          TOKEN: ${{ secrets.TOKEN }}",
            ),
            (
                "google-github-actions/run-gemini-cli@v0
        with:
          settings: '{\"tools\": {\"core\": [\"read_file\", \"run_shell_command\"], \"exclude\": [\"run_shell_command\"]}}'",
                "      - run: npm test
        env:
          TOKEN: ${{ secrets.TOKEN }}",
            ),
        ] {
            assert_eq!(found(agent, later), None, "{agent} {later}");
        }
    }

    #[test]
    fn steps_before_the_agent_do_not_count() {
        let workflow = Workflow::parse(
            Path::new("test.yml"),
            "on: issue_comment
jobs:
  agent:
    runs-on: ubuntu-latest
    steps:
      - run: npm test
        env:
          TOKEN: ${{ secrets.TOKEN }}
      - uses: anthropics/claude-code-action@v1
"
            .to_string(),
        )
        .unwrap();
        assert!(agent_code_with_secrets(&workflow, &agent_steps(&workflow)[0]).is_none());
    }
}
