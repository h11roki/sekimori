use yamlpath::Route;

use super::secrets::{EVERY_SECRET, shell_secrets};
use super::tools::{WRAPPERS, claude_may_edit, gemini_may_edit};
use super::{input, is_assignment, nearest_env, program_name, running_later_steps, shell_programs};
use crate::agent::{Agent, AgentStep, JobStep};
use crate::workflow::Workflow;

const RUN_KEY: &str = "run";
const CODEX_READ_ONLY: [(&str, &str); 3] = [
    ("sandbox", "read-only"),
    ("permission-profile", ":read-only"),
    ("safety-strategy", "read-only"),
];
const WORKSPACE_PATHS: [&str; 6] = [
    "./",
    "$GITHUB_WORKSPACE/",
    "${GITHUB_WORKSPACE}/",
    "${{github.workspace}}/",
    "$PWD/",
    "${PWD}/",
];
const LOOKUP_WRAPPER: &str = "command";
const LOOKUP_FLAGS: [&str; 2] = ["-v", "-V"];
const DURATION_WRAPPER: &str = "timeout";
const WRAPPER_VALUE_FLAGS: [(&str, &[&str]); 6] = [
    ("sudo", &["-u", "-g", "-h", "-p", "-C", "-D", "-T", "-U"]),
    ("doas", &["-u", "-C"]),
    ("env", &["-u", "-C", "-S"]),
    ("nice", &["-n"]),
    ("timeout", &["-k", "-s"]),
    ("xargs", &["-n", "-I", "-L", "-P", "-d", "-E", "-s", "-a"]),
];
const SUBCOMMAND_VALUE_FLAGS: [&str; 9] = [
    "--prefix",
    "-C",
    "--workspace",
    "-w",
    "--manifest-path",
    "--project",
    "--directory",
    "--cwd",
    "--filter",
];
const TOOLCHAIN_PREFIX: char = '+';
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
}

pub fn agent_code_with_secrets<'w>(
    workflow: &'w Workflow,
    step: &AgentStep<'w>,
    reached_agent: &[String],
) -> Option<AgentCode<'w>> {
    let edits_workspace = match step.agent {
        Agent::ClaudeCode => true,
        Agent::Codex => !CODEX_READ_ONLY.iter().any(|(name, read_only)| {
            input(step, name).is_some_and(|(_, value)| value.trim() == *read_only)
        }),
        Agent::GeminiCli => gemini_may_edit(step),
        Agent::ClaudeBase => claude_may_edit(step),
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
            let secret = shell_secrets(env, &run.run).into_iter().find(|secret| {
                !reached_agent
                    .iter()
                    .any(|known| known == EVERY_SECRET || known.eq_ignore_ascii_case(secret))
            })?;
            Some(AgentCode {
                setting: format!(
                    "`{command}` in a later step runs files the agent can edit, with {secret}"
                ),
                route: route.with_key(RUN_KEY),
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
        if name == LOOKUP_WRAPPER
            && arguments
                .iter()
                .any(|argument| LOOKUP_FLAGS.contains(&argument.as_str()))
        {
            return None;
        }
        let value_flags = WRAPPER_VALUE_FLAGS
            .iter()
            .find(|(wrapper, _)| *wrapper == name)
            .map_or(&[][..], |(_, flags)| *flags);
        let mut rest = arguments.iter().enumerate();
        let mut start = arguments.len();
        while let Some((index, word)) = rest.next() {
            if value_flags.contains(&word.as_str()) {
                rest.next();
            } else if !word.starts_with('-') && !is_assignment(word) {
                start = index;
                break;
            }
        }
        if name == DURATION_WRAPPER {
            start += 1;
        }
        return project_command(arguments.get(start..)?);
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
            if INLINE_CODE_LONG_FLAGS
                .iter()
                .any(|flag| word.starts_with(flag))
            {
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
                let attached_value = letters.len() > 1
                    && letters
                        .chars()
                        .next()
                        .is_some_and(|first| value_flags.contains(&format!("-{first}").as_str()));
                if attached_value {
                    continue;
                }
                let short_cluster =
                    !letters.starts_with('-') && letters.chars().all(|c| c.is_ascii_alphanumeric());
                if short_cluster && letters.contains(*inline_letters) {
                    return None;
                }
                let takes_value = short_cluster
                    && letters
                        .chars()
                        .last()
                        .is_some_and(|last| value_flags.contains(&format!("-{last}").as_str()));
                if takes_value {
                    words.next();
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
    let mut words = arguments.iter();
    while let Some(word) = words.next() {
        if SUBCOMMAND_VALUE_FLAGS.contains(&word.as_str()) {
            words.next();
            continue;
        }
        if word.starts_with(['-', TOOLCHAIN_PREFIX]) {
            continue;
        }
        return subcommands
            .contains(&word.as_str())
            .then(|| format!("{name} {word}"));
    }
    None
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
        agent_code_with_secrets(&workflow, &agent_steps(&workflow)[0], &[]).map(|found| {
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
            (
                "google-github-actions/run-gemini-cli@v0
        with:
          settings: '{\"tools\": {\"core\": [\"ReadFileTool\", \"WriteFileTool\"]}}'",
                "      - run: npm test
        env:
          TOKEN: ${{ secrets.TOKEN }}",
                "9: `npm test` in a later step runs files the agent can edit, with secrets.TOKEN",
            ),
            (
                "anthropics/claude-code-action/base-action@v1
        with:
          claude_args: --permission-mode acceptEdits",
                "      - run: npm test
        env:
          TOKEN: ${{ secrets.TOKEN }}",
                "9: `npm test` in a later step runs files the agent can edit, with secrets.TOKEN",
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
            ("bash -euo pipefail ./ci.sh", "bash ./ci.sh"),
            ("cargo +nightly build --release", "cargo build"),
            ("npm --prefix web run build", "npm run"),
            ("$PWD/ci/deploy.sh", "$PWD/ci/deploy.sh"),
            (
                "if [ -n \"$X\" ]; then ./scripts/deploy.sh; fi",
                "./scripts/deploy.sh",
            ),
            ("for d in a b; do make -C $d; done", "make"),
            (
                "ruby -Itest test/models/user_test.rb",
                "ruby test/models/user_test.rb",
            ),
            ("sudo -u runner ./scripts/deploy.sh", "./scripts/deploy.sh"),
            ("nice -n 10 make", "make"),
            ("timeout -k 5 10m npm test", "npm test"),
            (
                "cargo --manifest-path crates/x/Cargo.toml test",
                "cargo test",
            ),
        ] {
            let command = shell_programs(run)
                .iter()
                .find_map(|program| project_command(program));
            assert_eq!(command.as_deref(), Some(expected), "{run}");
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
            "sudo apt-get install -y make",
            "command -v pnpm",
            "sudo cp build/out.js /usr/local/",
            "bash -euo pipefail -c \"curl https://example.com\"",
            "node --eval=\"console.log(1)\"",
            "cat > body.md <<EOF\nnpm test passed\nEOF\ngh pr comment --body-file body.md",
        ] {
            let command = shell_programs(run)
                .iter()
                .find_map(|program| project_command(program));
            assert_eq!(command, None, "{run}");
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
                "anthropics/claude-code-base-action@beta
        with:
          allowed_tools: Read",
                "      - run: npm test
        env:
          TOKEN: ${{ secrets.TOKEN }}",
            ),
            (
                "google-github-actions/run-gemini-cli@v0
        with:
          settings: '{\"tools\": {\"core\": [\"read_file\", \"run_shell_command(git diff)\"]}}'",
                "      - run: npm test
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
    fn later_steps_that_cannot_run_with_the_agent_do_not_count() {
        let found_with = |agent_condition: &str, later_condition: &str| {
            let workflow = Workflow::parse(
                Path::new("test.yml"),
                format!(
                    "on: issue_comment
jobs:
  agent:
    runs-on: ubuntu-latest
    steps:
      - if: ${{{{ {agent_condition} }}}}
        uses: anthropics/claude-code-action@v1
      - if: ${{{{ {later_condition} }}}}
        run: npm test
        env:
          TOKEN: ${{{{ secrets.TOKEN }}}}
"
                ),
            )
            .unwrap();
            agent_code_with_secrets(&workflow, &agent_steps(&workflow)[0], &[]).is_some()
        };
        for (agent_condition, later_condition) in [
            (
                "needs.prep.outputs.provider == 'anthropic'",
                "needs.prep.outputs.provider == 'openai' && always()",
            ),
            (
                "contains(fromJSON('[\"a\", \"b\"]'), inputs.mode)",
                "inputs.mode == 'c'",
            ),
            (
                "!contains(fromJSON('[\"a\"]'), inputs.mode)",
                "github.event_name == 'issue_comment' && inputs.mode == 'A'",
            ),
            ("matrix.agent == 'claude'", "matrix.agent != 'claude'"),
            ("true", "inputs.mode == 'a' && inputs.mode == 'b'"),
            ("inputs.m == 'a' && inputs.m == 'b'", "success()"),
        ] {
            assert!(
                !found_with(agent_condition, later_condition),
                "{agent_condition} / {later_condition}"
            );
        }
        for (agent_condition, later_condition) in [
            ("env.PROVIDER == 'anthropic'", "env.PROVIDER == 'openai'"),
            ("inputs.mode == 'a'", "inputs.mode != 'b'"),
            (
                "inputs.mode == 'a' || inputs.mode == 'b'",
                "inputs.mode == 'c'",
            ),
            ("inputs.mode != 'a'", "inputs.mode != 'b'"),
            (
                "github.event.pull_request.labels[0].name == 'claude'",
                "github.event.pull_request.labels[1].name == 'deploy'",
            ),
            ("matrix.python == '3.10'", "matrix.python == '3.1'"),
            (
                "github.action_status == 'success'",
                "github.action_status == 'failure'",
            ),
            ("inputs.lang == 'Ä'", "inputs.lang == 'ä'"),
            ("inputs.lang == 'ς'", "inputs.lang == 'σ'"),
            ("matrix.n == '0x10'", "matrix.n == '0o20'"),
            ("matrix['os.name'] == 'linux'", "matrix.os.name == 'mac'"),
        ] {
            assert!(
                found_with(agent_condition, later_condition),
                "{agent_condition} / {later_condition}"
            );
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
        assert!(agent_code_with_secrets(&workflow, &agent_steps(&workflow)[0], &[]).is_none());
    }
}
