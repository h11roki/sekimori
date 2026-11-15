use yamlpath::Route;

use super::secrets::shell_secret;
use super::tools::{WRAPPERS, gemini_may_edit};
use super::{input, nearest_env, program_name, running_later_steps, shell_programs};
use crate::agent::{Agent, AgentStep, JobStep};
use crate::workflow::Workflow;

const RUN_KEY: &str = "run";
const CODEX_READ_ONLY: [(&str, &str); 3] = [
    ("sandbox", "read-only"),
    ("permission-profile", ":read-only"),
    ("safety-strategy", "read-only"),
];
const WORKSPACE_PATHS: [&str; 4] = [
    "./",
    "$GITHUB_WORKSPACE/",
    "${GITHUB_WORKSPACE}/",
    "${{github.workspace}}/",
];
const OUTSIDE_PATH_STARTS: [char; 3] = ['/', '$', '~'];
const STDIN_REDIRECT: &str = "<";
const SCRIPT_INTERPRETERS: [(&str, &[char], &[&str]); 7] = [
    ("bash", &['c'], &["-o", "-O", "--rcfile", "--init-file"]),
    ("sh", &['c'], &["-o"]),
    ("zsh", &['c'], &["-o"]),
    ("python", &['c'], &["-W", "-X", "-Q"]),
    (
        "node",
        &['e', 'p'],
        &[
            "-r",
            "--require",
            "--import",
            "--loader",
            "-C",
            "--conditions",
        ],
    ),
    ("ruby", &['e'], &["-r", "-I", "-C", "-E"]),
    ("perl", &['e', 'E'], &["-I", "-M", "-m"]),
];
const INLINE_CODE_LONG_FLAGS: [&str; 2] = ["--eval", "--print"];
const MODULE_FLAG: &str = "-m";
const MODULE_INTERPRETER: &str = "python";
const SCRIPT_EXTENSIONS: [&str; 9] = [
    ".sh", ".bash", ".py", ".js", ".mjs", ".cjs", ".ts", ".rb", ".pl",
];
const PROJECT_RUNNERS: [&str; 11] = [
    "make",
    "pytest",
    "tox",
    "nox",
    "pre-commit",
    "gradle",
    "mvn",
    "rake",
    "yarn",
    "pnpm",
    "bun",
];
const SUBCOMMAND_RUNNERS: [(&str, &[&str]); 7] = [
    (
        "npm",
        &[
            "run",
            "run-script",
            "test",
            "t",
            "install",
            "i",
            "ci",
            "it",
            "install-test",
            "exec",
            "build",
            "start",
            "publish",
            "rebuild",
            "pack",
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
    pub secret: String,
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
    running_later_steps(step)
        .into_iter()
        .find_map(|(route, later)| {
            let JobStep::Run(run) = later else {
                return None;
            };
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
                secret,
            })
        })
}

fn in_workspace(path: &str) -> bool {
    WORKSPACE_PATHS.iter().any(|start| path.starts_with(start))
        || !path.starts_with(OUTSIDE_PATH_STARTS)
}

fn project_command(program: &[String]) -> Option<String> {
    let (first, arguments) = program.split_first()?;
    let runs_workspace_file = WORKSPACE_PATHS.iter().any(|start| first.starts_with(start))
        || (in_workspace(first)
            && first.contains('/')
            && SCRIPT_EXTENSIONS
                .iter()
                .any(|extension| first.ends_with(extension)));
    if runs_workspace_file {
        return Some(first.clone());
    }
    let name = program_name(first).trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    if WRAPPERS.contains(&name) {
        return (1..program.len()).find_map(|start| project_command(&program[start..]));
    }
    if let Some((_, inline_letters, value_flags)) = SCRIPT_INTERPRETERS
        .iter()
        .find(|(interpreter, ..)| *interpreter == name)
    {
        let mut words = arguments.iter();
        while let Some(word) = words.next() {
            if word == STDIN_REDIRECT {
                let script = words.next()?;
                return in_workspace(script).then(|| format!("{name} < {script}"));
            }
            if INLINE_CODE_LONG_FLAGS.contains(&word.as_str()) {
                return None;
            }
            if name == MODULE_INTERPRETER && word == MODULE_FLAG {
                return Some(format!("{name} {MODULE_FLAG}"));
            }
            if value_flags.contains(&word.as_str()) {
                words.next();
                continue;
            }
            if let Some(letters) = word.strip_prefix('-') {
                let short_cluster =
                    !letters.starts_with('-') && letters.chars().all(|c| c.is_ascii_alphanumeric());
                if short_cluster && letters.contains(*inline_letters) {
                    return None;
                }
                continue;
            }
            return (in_workspace(word) && !word.starts_with(STDIN_REDIRECT))
                .then(|| format!("{name} {word}"));
        }
        return None;
    }
    if PROJECT_RUNNERS.contains(&name) {
        return Some(name.to_string());
    }
    let (_, subcommands) = SUBCOMMAND_RUNNERS
        .iter()
        .find(|(runner, _)| *runner == name)?;
    let subcommand = arguments
        .iter()
        .find(|argument| !argument.starts_with('-'))
        .filter(|subcommand| subcommands.contains(&subcommand.as_str()))?;
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
    fn the_command_is_read_past_wrappers_flags_and_shorthands() {
        for (run, expected) in [
            (
                "python scripts/deploy.py -c prod.yaml",
                "python scripts/deploy.py",
            ),
            ("python -W ignore x.py", "python x.py"),
            ("bash -o pipefail ./ci.sh", "bash ./ci.sh"),
            ("bash < ./install.sh", "bash < ./install.sh"),
            ("ruby -rbundler/setup test.rb", "ruby test.rb"),
            ("scripts/release.sh", "scripts/release.sh"),
            (
                "$GITHUB_WORKSPACE/ci/deploy.sh",
                "$GITHUB_WORKSPACE/ci/deploy.sh",
            ),
            ("timeout 20m npm test", "npm test"),
            ("sudo -E make install", "make"),
            ("/usr/bin/env CI=1 ./test.sh", "./test.sh"),
            ("yarn", "yarn"),
            ("npm i", "npm i"),
        ] {
            let program: Vec<String> = shell_programs(run).remove(0);
            assert_eq!(
                project_command(&program).as_deref(),
                Some(expected),
                "{run}"
            );
        }
        for run in [
            "https://example.com/a/b",
            "s/[-_]+$//",
            ".artifacts/report",
            "npx prettier --check .",
            "npm view react",
            "python -c 'print(1)'",
            "node -pe 1",
            "bash ~/setup.sh",
            "curl https://example.com",
        ] {
            assert_eq!(
                project_command(&shell_programs(run).remove(0)),
                None,
                "{run}"
            );
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
          safety-strategy: read-only",
                "      - run: npm test
        env:
          TOKEN: ${{ secrets.TOKEN }}",
            ),
            (
                "google-github-actions/run-gemini-cli@v0
        with:
          settings: '{\"tools\": {\"exclude\": [\"write_file\", \"replace\", \"ShellTool\"]}}'",
                "      - run: npm test
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
