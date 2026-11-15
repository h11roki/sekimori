use json_comments::CommentSettings;
use serde_json::Value;
use yamlpath::Route;

use super::{Input, SETTINGS_INPUT, before_comment, input, input_route, lookup, program_name};
use crate::agent::{Agent, AgentStep};

const CLAUDE_ALLOWED_TOOLS_FLAGS: [&str; 2] = ["--allowedTools", "--allowed-tools"];
const CLAUDE_DISALLOWED_TOOLS_FLAGS: [&str; 2] = ["--disallowedTools", "--disallowed-tools"];
const CLAUDE_SKIP_PERMISSIONS_FLAG: &str = "--dangerously-skip-permissions";
const CLAUDE_PERMISSION_MODE_FLAG: &str = "--permission-mode";
const CLAUDE_SETTINGS_FLAG: &str = "--settings";
const CLAUDE_BYPASS_MODE: &str = "bypassPermissions";
const CLAUDE_ACCEPT_EDITS_MODE: &str = "acceptEdits";
const CLAUDE_BYPASS_DISABLED: &str = "disable";
const CLAUDE_SHELL_TOOL: &str = "Bash";
const CLAUDE_ARGS_INPUT: &str = "claude_args";
const CLAUDE_LEGACY_ALLOWED_INPUT: &str = "allowed_tools";
const CLAUDE_LEGACY_DISALLOWED_INPUT: &str = "disallowed_tools";
const CODEX_BROAD_INPUTS: [(&str, &str); 3] = [
    ("sandbox", "danger-full-access"),
    ("permission-profile", ":danger-full-access"),
    ("safety-strategy", "unsafe"),
];
const GEMINI_SHELL_TOOLS: [&str; 2] = ["run_shell_command", "ShellTool"];
const GEMINI_FILE_EDIT_TOOLS: [[&str; 2]; 2] =
    [["write_file", "WriteFileTool"], ["replace", "EditTool"]];
const INTERPRETERS: [&str; 24] = [
    "bash",
    "sh",
    "zsh",
    "dash",
    "ksh",
    "fish",
    "pwsh",
    "powershell",
    "cmd",
    "python",
    "node",
    "nodejs",
    "deno",
    "bun",
    "tsx",
    "ts-node",
    "perl",
    "ruby",
    "php",
    "lua",
    "awk",
    "osascript",
    "eval",
    "exec",
];
const PACKAGE_RUNNERS: [&str; 5] = ["npx", "pnpx", "bunx", "uvx", "pipx"];
const RUN_SUBCOMMANDS: [(&str, &[&str]); 9] = [
    ("npm", &["exec", "x"]),
    ("pnpm", &["dlx", "exec"]),
    ("yarn", &["dlx", "exec"]),
    ("uv", &["run"]),
    ("poetry", &["run"]),
    ("go", &["run"]),
    ("cargo", &["run"]),
    ("dotnet", &["run"]),
    ("bundle", &["exec"]),
];
const PROJECT_CODE_TOOLS: [&str; 31] = [
    "npm",
    "pnpm",
    "yarn",
    "bun",
    "make",
    "gradle",
    "gradlew",
    "mvn",
    "mvnw",
    "cargo",
    "go",
    "pytest",
    "tox",
    "nox",
    "rake",
    "bundle",
    "composer",
    "dotnet",
    "mix",
    "just",
    "task",
    "uv",
    "poetry",
    "pip",
    "swift",
    "xcodebuild",
    "sbt",
    "jest",
    "vitest",
    "phpunit",
    "rspec",
];
const RELATIVE_PATH_PREFIXES: [&str; 2] = ["./", "../"];
const CLAUDE_EDIT_TOOLS: [&str; 3] = ["Edit", "MultiEdit", "Write"];
const EDITABLE_CODE_NOTE: &str = ", and the agent can edit the files that command runs";
pub(super) const WRAPPERS: [&str; 10] = [
    "env", "sudo", "doas", "xargs", "nohup", "timeout", "nice", "command", "busybox", "time",
];

#[derive(Debug)]
pub struct BroadTool<'w> {
    pub setting: String,
    pub route: Route<'w>,
}

pub fn broad_tool<'w>(step: &AgentStep<'w>) -> Option<BroadTool<'w>> {
    match step.agent {
        Agent::ClaudeCode | Agent::ClaudeBase | Agent::ClaudeBaseBeta => {
            ClaudePermissions::of(step).broad_tool(step.agent == Agent::ClaudeCode)
        }
        Agent::Codex => CODEX_BROAD_INPUTS.into_iter().find_map(|(name, broad)| {
            let (key, value) = input(step, name)?;
            (value.trim() == broad).then(|| BroadTool {
                setting: format!("{key}: {broad}"),
                route: input_route(step, key),
            })
        }),
        Agent::GeminiCli => gemini_settings(step),
    }
}

#[derive(Default)]
struct ClaudePermissions<'w> {
    skip_permissions: Option<BroadTool<'w>>,
    cli_mode: Option<(String, BroadTool<'w>)>,
    settings_mode: Option<(String, BroadTool<'w>)>,
    bypass_disabled: bool,
    shell_allows: Vec<(String, BroadTool<'w>)>,
    project_code_allows: Vec<(String, BroadTool<'w>)>,
    edit_allowed: bool,
    denied: Vec<String>,
}

impl<'w> ClaudePermissions<'w> {
    fn of(step: &AgentStep<'w>) -> Self {
        let mut permissions = Self::default();
        if step.agent != Agent::ClaudeBaseBeta
            && let Some((key, value)) = input(step, CLAUDE_ARGS_INPUT)
        {
            permissions.read_args(key, value, &input_route(step, key));
        }
        if let Some((key, value)) = input(step, SETTINGS_INPUT) {
            permissions.read_settings(key, value, &input_route(step, key));
        }
        if step.agent == Agent::ClaudeBase {
            return permissions;
        }
        if let Some((key, value)) = input(step, CLAUDE_LEGACY_ALLOWED_INPUT) {
            let route = input_route(step, key);
            for tool in tool_list(value) {
                permissions.allow(tool, format!("{key} allows `{tool}`"), &route);
            }
        }
        if let Some((_, value)) = input(step, CLAUDE_LEGACY_DISALLOWED_INPUT) {
            tool_list(value).for_each(|tool| permissions.deny(tool));
        }
        permissions
    }

    fn edits(&self) -> bool {
        self.edit_allowed
            || self
                .cli_mode
                .as_ref()
                .or(self.settings_mode.as_ref())
                .is_some_and(|(mode, _)| mode == CLAUDE_ACCEPT_EDITS_MODE)
    }

    fn broad_tool(self, edits_by_default: bool) -> Option<BroadTool<'w>> {
        let edits = edits_by_default || self.edits();
        if !self.bypass_disabled {
            if let Some(skip) = self.skip_permissions {
                return Some(skip);
            }
            if let Some((mode, setting)) = self.cli_mode.or(self.settings_mode)
                && mode == CLAUDE_BYPASS_MODE
            {
                return Some(setting);
            }
        }
        let denied = self.denied;
        let allowed = |(tool, _): &(String, BroadTool<'w>)| {
            !denied.iter().any(|deny| {
                deny == tool
                    || pattern_of(deny, CLAUDE_SHELL_TOOL).is_some_and(matches_every_command)
            })
        };
        let any_command = self.shell_allows.into_iter().find(allowed);
        let project_code = || {
            self.project_code_allows
                .into_iter()
                .find(allowed)
                .filter(|_| edits)
                .map(|(tool, setting)| {
                    (
                        tool,
                        BroadTool {
                            setting: format!("{}{EDITABLE_CODE_NOTE}", setting.setting),
                            route: setting.route,
                        },
                    )
                })
        };
        any_command
            .or_else(project_code)
            .map(|(_, setting)| setting)
    }

    fn read_args(&mut self, key: &str, value: &str, route: &Route<'w>) {
        let without_comment_lines = value
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        let text = before_comment(&without_comment_lines);
        let args = shlex::split(text)
            .unwrap_or_else(|| text.split_whitespace().map(String::from).collect());
        let mut args = args.iter().peekable();
        while let Some(arg) = args.next() {
            let (flag, inline_value) = match arg.split_once('=') {
                Some((flag, value)) => (flag, Some(value)),
                None => (arg.as_str(), None),
            };
            if flag == CLAUDE_SKIP_PERMISSIONS_FLAG {
                self.skip_permissions.get_or_insert_with(|| BroadTool {
                    setting: format!("{key} has {flag}"),
                    route: route.clone(),
                });
            } else if flag == CLAUDE_PERMISSION_MODE_FLAG {
                let mode = inline_value.or_else(|| args.next().map(String::as_str));
                if let Some(mode) = mode {
                    let setting = BroadTool {
                        setting: format!("{key} has {flag} {mode}"),
                        route: route.clone(),
                    };
                    self.cli_mode = Some((mode.to_string(), setting));
                }
            } else if flag == CLAUDE_SETTINGS_FLAG {
                let settings = inline_value.or_else(|| args.next().map(String::as_str));
                if let Some(settings) = settings {
                    self.read_settings(&format!("{key} {flag}"), settings, route);
                }
            } else if CLAUDE_ALLOWED_TOOLS_FLAGS.contains(&flag) {
                for value in inline_value.into_iter().chain(flag_values(&mut args)) {
                    for tool in tool_list(value) {
                        self.allow(tool, format!("{key} allows `{tool}` through {flag}"), route);
                    }
                }
            } else if CLAUDE_DISALLOWED_TOOLS_FLAGS.contains(&flag) {
                for value in inline_value.into_iter().chain(flag_values(&mut args)) {
                    tool_list(value).for_each(|tool| self.deny(tool));
                }
            }
        }
    }

    fn read_settings(&mut self, origin: &str, text: &str, route: &Route<'w>) {
        let Ok(settings) = serde_json::from_str::<Value>(text) else {
            return;
        };
        let Some(permissions) = settings.get("permissions") else {
            return;
        };
        if let Some(mode) = permissions.get("defaultMode").and_then(Value::as_str) {
            let setting = BroadTool {
                setting: format!("{origin} sets permissions.defaultMode to {mode}"),
                route: route.clone(),
            };
            self.settings_mode = Some((mode.to_string(), setting));
        }
        self.bypass_disabled |= permissions
            .get("disableBypassPermissionsMode")
            .and_then(Value::as_str)
            == Some(CLAUDE_BYPASS_DISABLED);
        for tool in string_array(permissions.get("allow")) {
            self.allow(
                tool,
                format!("{origin} allows `{tool}` in permissions.allow"),
                route,
            );
        }
        string_array(permissions.get("deny"))
            .chain(string_array(permissions.get("ask")))
            .for_each(|tool| self.deny(tool));
    }

    fn allow(&mut self, tool: &str, setting: String, route: &Route<'w>) {
        let tool = tool.trim();
        if CLAUDE_EDIT_TOOLS
            .iter()
            .any(|edit| pattern_of(tool, edit).is_some())
        {
            self.edit_allowed = true;
        }
        let Some(pattern) = pattern_of(tool, CLAUDE_SHELL_TOOL).map(str::trim) else {
            return;
        };
        let reach = match pattern.strip_suffix('*') {
            Some(prefix) => command_reach(prefix.trim_end_matches([':', ' '])),
            None if pattern.is_empty() => Reach::AnyCommand,
            None => match command_reach(pattern) {
                Reach::AnyCommand | Reach::ProjectCode if runs_project_code(pattern) => {
                    Reach::ProjectCode
                }
                Reach::AnyCommand | Reach::ProjectCode | Reach::Fixed => Reach::Fixed,
            },
        };
        let allow = (
            tool.to_string(),
            BroadTool {
                setting,
                route: route.clone(),
            },
        );
        match reach {
            Reach::AnyCommand => self.shell_allows.push(allow),
            Reach::ProjectCode => self.project_code_allows.push(allow),
            Reach::Fixed => {}
        }
    }

    fn deny(&mut self, tool: &str) {
        self.denied.push(tool.trim().to_string());
    }
}

fn flag_values<'a>(args: &mut std::iter::Peekable<std::slice::Iter<'a, String>>) -> Vec<&'a str> {
    let mut values = Vec::new();
    while let Some(next) = args.next_if(|next| !next.starts_with("--")) {
        values.push(next.as_str());
    }
    values
}

fn tool_list(value: &str) -> impl Iterator<Item = &str> {
    let mut depth = 0usize;
    value
        .split(move |c: char| {
            match c {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ => {}
            }
            depth == 0 && (c == ',' || c.is_whitespace())
        })
        .filter(|tool| !tool.is_empty())
}

fn string_array(value: Option<&Value>) -> impl Iterator<Item = &str> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
}

fn pattern_of<'a>(entry: &'a str, tool: &str) -> Option<&'a str> {
    match entry.trim().strip_prefix(tool)? {
        "" => Some(""),
        rest => rest.strip_prefix('(')?.strip_suffix(')'),
    }
}

fn matches_every_command(pattern: &str) -> bool {
    pattern.chars().all(|c| matches!(c, '*' | ':' | ' '))
}

#[derive(Clone, Copy, PartialEq)]
enum Reach {
    AnyCommand,
    ProjectCode,
    Fixed,
}

fn opens_any_command(prefix: &str) -> bool {
    command_reach(prefix) == Reach::AnyCommand
}

fn command_reach(prefix: &str) -> Reach {
    let mut words = prefix.split_whitespace();
    let Some(program) = words.next() else {
        return Reach::AnyCommand;
    };
    if matches_every_command(program) {
        return Reach::AnyCommand;
    }
    let name = program_name(program).trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    let mut arguments = words.filter(|word| !word.starts_with('-')).peekable();
    let takes_any_code = WRAPPERS.contains(&name)
        || PACKAGE_RUNNERS.contains(&name)
        || (INTERPRETERS.contains(&name) && arguments.peek().is_none())
        || RUN_SUBCOMMANDS
            .iter()
            .find(|(tool, _)| *tool == name)
            .is_some_and(|(_, runs)| arguments.peek().is_none_or(|first| runs.contains(first)));
    if takes_any_code {
        Reach::AnyCommand
    } else if runs_project_code(program) {
        Reach::ProjectCode
    } else {
        Reach::Fixed
    }
}

fn runs_project_code(command: &str) -> bool {
    let program = command.split_whitespace().next().unwrap_or_default();
    let name = program_name(program);
    PROJECT_CODE_TOOLS.contains(&name)
        || INTERPRETERS.contains(&name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.'))
        || RELATIVE_PATH_PREFIXES
            .iter()
            .any(|prefix| program.starts_with(prefix))
}

fn gemini_settings<'w>(step: &AgentStep<'w>) -> Option<BroadTool<'w>> {
    let (key, value) = match lookup(step, SETTINGS_INPUT) {
        Input::Unreadable => return None,
        Input::Text { key, value } if !value.trim().is_empty() => (key, value),
        Input::Missing | Input::Text { .. } => {
            return Some(BroadTool {
                setting: "no `settings`, so tools.core is unset and every built-in tool runs under --yolo"
                    .to_string(),
                route: step.route.clone(),
            });
        }
    };
    let settings = settings_json(value)?;
    let route = input_route(step, key);
    let Some(core) = settings
        .get("tools")
        .and_then(|tools| tools.get("core"))
        .filter(|core| !core.is_null())
    else {
        return Some(BroadTool {
            setting: format!("{key} sets no tools.core, so every built-in tool runs under --yolo"),
            route,
        });
    };
    let excluded: Vec<&str> =
        string_array(settings.get("tools").and_then(|tools| tools.get("exclude")))
            .map(str::trim)
            .collect();
    let shell_tool = |reaches: fn(&str) -> bool| {
        string_array(Some(core)).find(|tool| {
            GEMINI_SHELL_TOOLS.iter().any(|shell| {
                pattern_of(tool, shell).is_some_and(reaches)
                    && !excluded
                        .iter()
                        .any(|exclusion| exclusion == shell || exclusion == &tool.trim())
            })
        })
    };
    if let Some(tool) = shell_tool(opens_any_command) {
        return Some(BroadTool {
            setting: format!("{key} allows `{tool}` in tools.core"),
            route,
        });
    }
    let tool = shell_tool(runs_project_code).filter(|_| gemini_may_edit(step))?;
    Some(BroadTool {
        setting: format!("{key} allows `{tool}` in tools.core{EDITABLE_CODE_NOTE}"),
        route,
    })
}

pub(super) fn claude_may_edit(step: &AgentStep) -> bool {
    let permissions = ClaudePermissions::of(step);
    permissions.edits() || permissions.broad_tool(false).is_some()
}

pub(super) fn gemini_may_edit(step: &AgentStep) -> bool {
    let settings = match lookup(step, SETTINGS_INPUT) {
        Input::Text { value, .. } => settings_json(value),
        Input::Missing | Input::Unreadable => None,
    };
    let tools = settings.as_ref().and_then(|settings| settings.get("tools"));
    let core: Option<Vec<&str>> = tools
        .and_then(|tools| tools.get("core"))
        .filter(|core| !core.is_null())
        .map(|core| string_array(Some(core)).map(str::trim).collect());
    let excluded: Vec<&str> = string_array(tools.and_then(|tools| tools.get("exclude")))
        .map(str::trim)
        .collect();
    let usable = |names: &[&str], opens: fn(&str) -> bool| {
        let allowed = core.as_ref().is_none_or(|core| {
            core.iter().any(|tool| {
                names
                    .iter()
                    .any(|name| pattern_of(tool, name).is_some_and(opens))
            })
        });
        allowed && !names.iter().any(|name| excluded.contains(name))
    };
    GEMINI_FILE_EDIT_TOOLS
        .iter()
        .any(|names| usable(names, |_| true))
        || usable(&GEMINI_SHELL_TOOLS, opens_any_command)
}

fn settings_json(value: &str) -> Option<Value> {
    serde_json::from_reader(CommentSettings::c_style().strip_comments(value.as_bytes())).ok()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::agent::agent_steps;
    use crate::workflow::Workflow;

    fn broad_setting(uses: &str, with: &str) -> Option<String> {
        let workflow = Workflow::parse(
            Path::new("test.yml"),
            format!(
                "on: issues
jobs:
  agent:
    runs-on: ubuntu-latest
    steps:
      - uses: {uses}
{with}"
            ),
        )
        .unwrap();
        let steps = agent_steps(&workflow);
        broad_tool(&steps[0]).map(|broad| {
            let (line, _) = workflow.position(&broad.route).unwrap();
            format!("{line}: {}", broad.setting)
        })
    }

    fn claude(with: &str) -> Option<String> {
        broad_setting("anthropics/claude-code-action@v1", with)
    }

    fn base(with: &str) -> Option<String> {
        broad_setting("anthropics/claude-code-action/base-action@v1", with)
    }

    fn codex(with: &str) -> Option<String> {
        broad_setting("openai/codex-action@v1", with)
    }

    fn gemini(with: &str) -> Option<String> {
        broad_setting("google-github-actions/run-gemini-cli@v0", with)
    }

    #[test]
    fn claude_allowed_tools_with_bare_bash_is_broad() {
        let cases = [
            (
                "        with:\n          claude_args: --allowedTools \"Read,Bash,Edit\"\n",
                "8: claude_args allows `Bash` through --allowedTools",
            ),
            (
                "        with:\n          claude_args: |\n            --model claude-opus-5\n            --allowed-tools Read 'Bash(*)'\n",
                "8: claude_args allows `Bash(*)` through --allowed-tools",
            ),
            (
                "        with:\n          claude_args: --allowedTools=Bash\n",
                "8: claude_args allows `Bash` through --allowedTools",
            ),
        ];
        for (with, expected) in cases {
            assert_eq!(claude(with).as_deref(), Some(expected), "{with}");
        }
    }

    #[test]
    fn claude_bypass_flags_are_broad() {
        assert_eq!(
            claude("        with:\n          claude_args: --dangerously-skip-permissions\n")
                .as_deref(),
            Some("8: claude_args has --dangerously-skip-permissions")
        );
        assert_eq!(
            claude("        with:\n          claude_args: --permission-mode bypassPermissions\n")
                .as_deref(),
            Some("8: claude_args has --permission-mode bypassPermissions")
        );
    }

    #[test]
    fn claude_scoped_or_commented_bash_is_not_broad() {
        for with in [
            "        with:\n          claude_args: --allowedTools \"Bash(gh issue view:*),Bash(gh pr diff)\"\n",
            "        with:\n          claude_args: --allowedTools BashOutput\n",
            "        with:\n          claude_args: |\n            # --allowedTools Bash\n            --model claude-opus-5\n",
            "        with:\n          claude_args: --permission-mode auto --disallowedTools Bash\n",
            "        with:\n          claude_args: --model claude-opus-5 --allowedTools\n",
        ] {
            assert_eq!(claude(with), None, "{with}");
        }
        assert_eq!(claude(""), None);
    }

    #[test]
    fn claude_settings_json_is_read_but_a_path_is_not() {
        assert_eq!(
            claude("        with:\n          settings: '{\"permissions\": {\"allow\": [\"Read\", \"Bash\"]}}'\n")
                .as_deref(),
            Some("8: settings allows `Bash` in permissions.allow")
        );
        assert_eq!(
            claude("        with:\n          settings: '{\"permissions\": {\"defaultMode\": \"bypassPermissions\"}}'\n")
                .as_deref(),
            Some("8: settings sets permissions.defaultMode to bypassPermissions")
        );
        assert_eq!(
            claude(
                "        with:\n          settings: '{\"permissions\": {\"allow\": [\"Bash(git diff:*)\"]}}'\n"
            ),
            None
        );
        assert_eq!(
            claude("        with:\n          settings: .claude/ci-settings.json\n"),
            None
        );
    }

    #[test]
    fn codex_full_access_and_unsafe_are_broad() {
        assert_eq!(
            codex("        with:\n          sandbox: danger-full-access\n").as_deref(),
            Some("8: sandbox: danger-full-access")
        );
        assert_eq!(
            codex("        with:\n          permission-profile: \":danger-full-access\"\n")
                .as_deref(),
            Some("8: permission-profile: :danger-full-access")
        );
        assert_eq!(
            codex("        with:\n          safety-strategy: unsafe\n").as_deref(),
            Some("8: safety-strategy: unsafe")
        );
        for with in [
            "",
            "        with:\n          sandbox: workspace-write\n",
            "        with:\n          permission-profile: \":workspace\"\n",
            "        with:\n          safety-strategy: drop-sudo\n",
        ] {
            assert_eq!(codex(with), None, "{with}");
        }
    }

    #[test]
    fn gemini_without_tools_core_is_broad() {
        assert_eq!(
            gemini("").as_deref(),
            Some(
                "6: no `settings`, so tools.core is unset and every built-in tool runs under --yolo"
            )
        );
        assert_eq!(
            gemini(
                "        with:\n          settings: '{\"model\": {\"name\": \"gemini-3-pro\"}}'\n"
            )
            .as_deref(),
            Some("8: settings sets no tools.core, so every built-in tool runs under --yolo")
        );
        assert_eq!(
            gemini("        with:\n          settings: '{\"tools\": {\"core\": [\"read_file\", \"run_shell_command\"]}}'\n")
                .as_deref(),
            Some("8: settings allows `run_shell_command` in tools.core")
        );
    }

    #[test]
    fn gemini_with_a_restricted_tools_core_is_not_broad() {
        for with in [
            "        with:\n          settings: '{\"tools\": {\"core\": []}}'\n",
            "        with:\n          settings: '{\"tools\": {\"core\": [\"run_shell_command(git status)\", \"ShellTool(npm test)\"]}}'\n",
            "        with:\n          settings: ${{ vars.GEMINI_SETTINGS }}\n",
        ] {
            assert_eq!(gemini(with), None, "{with}");
        }
    }

    #[test]
    fn claude_allowed_tools_may_be_separated_by_spaces_but_not_inside_parentheses() {
        assert_eq!(
            claude("        with:\n          claude_args: --allowedTools \"Read Bash Edit\"\n")
                .as_deref(),
            Some("8: claude_args allows `Bash` through --allowedTools")
        );
        assert_eq!(
            claude(
                "        with:\n          claude_args: --allowedTools \"Bash(gh pr diff) Read\"\n"
            ),
            None
        );
    }

    #[test]
    fn claude_settings_given_through_claude_args_are_read() {
        assert_eq!(
            claude("        with:\n          claude_args: --settings '{\"permissions\":{\"defaultMode\":\"bypassPermissions\"}}'\n")
                .as_deref(),
            Some("8: claude_args --settings sets permissions.defaultMode to bypassPermissions")
        );
    }

    #[test]
    fn claude_legacy_allowed_tools_input_is_read() {
        assert_eq!(
            claude("        with:\n          allowed_tools: \"Read,Bash\"\n").as_deref(),
            Some("8: allowed_tools allows `Bash`")
        );
    }

    #[test]
    fn claude_shell_denied_everywhere_is_not_broad_but_a_scoped_deny_is() {
        for with in [
            "        with:\n          claude_args: --allowedTools Bash --disallowedTools Bash\n",
            "        with:\n          settings: '{\"permissions\": {\"allow\": [\"Bash\"], \"deny\": [\"Bash\"]}}'\n",
            "        with:\n          claude_args: --allowedTools Bash\n          disallowed_tools: Bash\n",
        ] {
            assert_eq!(claude(with), None, "{with}");
        }
        assert_eq!(
            claude("        with:\n          claude_args: --allowedTools Bash --disallowedTools \"Bash(rm:*)\"\n")
                .as_deref(),
            Some("8: claude_args allows `Bash` through --allowedTools")
        );
    }

    #[test]
    fn claude_interpreters_with_any_arguments_are_broad() {
        assert_eq!(
            claude("        with:\n          claude_args: --allowedTools \"Bash(bash:*)\"\n")
                .as_deref(),
            Some("8: claude_args allows `Bash(bash:*)` through --allowedTools")
        );
        assert_eq!(
            claude("        with:\n          claude_args: --allowedTools \"Bash(python *)\"\n")
                .as_deref(),
            Some("8: claude_args allows `Bash(python *)` through --allowedTools")
        );

        assert_eq!(
            claude("        with:\n          claude_args: --allowedTools \"Bash(bash -c:*)\"\n")
                .as_deref(),
            Some("8: claude_args allows `Bash(bash -c:*)` through --allowedTools")
        );
        for scoped in [
            "Bash(bun test:*)",
            "Bash(python -m pytest:*)",
            "Bash(bun install)",
            "Bash(python scripts/check.py)",
        ] {
            let with =
                format!("        with:\n          claude_args: --allowedTools \"{scoped}\"\n");
            assert_eq!(base(&with), None, "{scoped}");
        }
    }

    #[test]
    fn build_commands_are_broad_when_the_agent_can_edit_what_they_run() {
        let allow = |tools: &str| {
            format!("        with:\n          claude_args: --allowedTools \"{tools}\"\n")
        };
        for build in [
            "Bash(npm test:*)",
            "Bash(npm run test)",
            "Bash(./gradlew build:*)",
            "Bash(make)",
            "Bash(cargo test:*)",
            "Bash(./scripts/check.sh)",
        ] {
            assert_eq!(
                claude(&allow(build)),
                Some(format!(
                    "8: claude_args allows `{build}` through --allowedTools{EDITABLE_CODE_NOTE}"
                )),
                "{build}"
            );
            assert_eq!(base(&allow(build)), None, "{build}");
            assert!(base(&allow(&format!("{build},Edit"))).is_some(), "{build}");
            assert!(
                base(&format!(
                    "{}            --permission-mode acceptEdits\n",
                    allow(build).replace("claude_args: ", "claude_args: |\n            ")
                ))
                .is_some(),
                "{build}"
            );
        }
        assert_eq!(base(&allow("Bash(npm test:*),Read,Grep")), None);
    }

    #[test]
    fn gemini_build_commands_are_broad_with_file_edits() {
        let settings = |core: &str| {
            format!("        with:\n          settings: '{{\"tools\": {{\"core\": [{core}]}}}}'\n")
        };
        assert_eq!(
            gemini(&settings("\"run_shell_command(npm test)\", \"write_file\"")),
            Some(format!(
                "8: settings allows `run_shell_command(npm test)` in tools.core{EDITABLE_CODE_NOTE}"
            ))
        );
        assert_eq!(
            gemini(&settings("\"run_shell_command(npm test)\", \"read_file\"")),
            None
        );
        assert!(gemini(&settings("\"run_shell_command(npx)\", \"read_file\"")).is_some());
    }

    #[test]
    fn package_runners_are_broad_without_edits() {
        for runner in [
            "Bash(npx:*)",
            "Bash(npx tsx:*)",
            "Bash(uvx:*)",
            "Bash(uv run:*)",
            "Bash(uv:*)",
            "Bash(pnpm dlx:*)",
            "Bash(go run:*)",
            "Bash(npm exec:*)",
            "Bash(npm:*)",
        ] {
            let with =
                format!("        with:\n          claude_args: --allowedTools \"{runner}\"\n");
            assert!(base(&with).is_some(), "{runner}");
        }
        for fixed in [
            "Bash(uv run pytest)",
            "Bash(go test:*)",
            "Bash(gh pr view:*)",
        ] {
            let with =
                format!("        with:\n          claude_args: --allowedTools \"{fixed}\"\n");
            assert_eq!(base(&with), None, "{fixed}");
        }
    }

    #[test]
    fn gemini_shell_limited_to_an_interpreter_is_broad() {
        assert_eq!(
            gemini("        with:\n          settings: '{\"tools\": {\"core\": [\"run_shell_command(bash)\"]}}'\n")
                .as_deref(),
            Some("8: settings allows `run_shell_command(bash)` in tools.core")
        );
    }

    #[test]
    fn gemini_settings_may_contain_comments() {
        assert_eq!(
            gemini("        with:\n          settings: |\n            {\n              // telemetry off\n              \"telemetry\": {\"enabled\": false}\n            }\n")
                .as_deref(),
            Some("8: settings sets no tools.core, so every built-in tool runs under --yolo")
        );
    }

    #[test]
    fn gemini_inputs_given_by_an_expression_are_not_judged() {
        assert_eq!(
            gemini("        with: ${{ fromJSON(vars.GEMINI_INPUTS) }}\n"),
            None
        );
    }

    #[test]
    fn claude_args_after_a_comment_are_ignored_like_shell_quote_does() {
        assert_eq!(
            claude(
                "        with:\n          claude_args: |\n            --model claude-opus-5  # pinned\n            --allowedTools Bash\n"
            ),
            None
        );
        assert_eq!(
            claude("        with:\n          claude_args: |\n            --append-system-prompt 'see #42' --allowedTools Bash\n")
                .as_deref(),
            Some("8: claude_args allows `Bash` through --allowedTools")
        );
    }

    #[test]
    fn claude_program_runners_written_as_paths_versions_or_wrappers_are_broad() {
        for tool in [
            "Bash(/bin/bash:*)",
            "Bash(python3.12:*)",
            "Bash(sudo:*)",
            "Bash(xargs *)",
        ] {
            let with = format!("        with:\n          claude_args: --allowedTools \"{tool}\"\n");
            assert_eq!(
                claude(&with).as_deref(),
                Some(format!("8: claude_args allows `{tool}` through --allowedTools").as_str()),
                "{tool}"
            );
        }
    }

    #[test]
    fn claude_bypass_is_not_broad_when_disabled_or_overridden() {
        for with in [
            "        with:\n          claude_args: --dangerously-skip-permissions\n          settings: '{\"permissions\": {\"disableBypassPermissionsMode\": \"disable\"}}'\n",
            "        with:\n          claude_args: --permission-mode bypassPermissions --model x --permission-mode default\n",
            "        with:\n          claude_args: --permission-mode acceptEdits\n          settings: '{\"permissions\": {\"defaultMode\": \"bypassPermissions\"}}'\n",
        ] {
            assert_eq!(claude(with), None, "{with}");
        }
    }

    #[test]
    fn claude_deny_matching_the_allow_exactly_closes_it() {
        assert_eq!(
            claude(
                "        with:\n          claude_args: --allowedTools \"Bash(bash:*)\" --disallowedTools \"Bash(bash:*)\"\n"
            ),
            None
        );
    }

    #[test]
    fn claude_wrappers_with_their_own_arguments_before_the_wildcard_are_broad() {
        for tool in [
            "Bash(timeout 600:*)",
            "Bash(sudo -u root:*)",
            "Bash(env FOO=1:*)",
            "Bash(powershell:*)",
        ] {
            let with = format!("        with:\n          claude_args: --allowedTools \"{tool}\"\n");
            assert_eq!(
                claude(&with).as_deref(),
                Some(format!("8: claude_args allows `{tool}` through --allowedTools").as_str()),
                "{tool}"
            );
        }
    }

    #[test]
    fn claude_args_with_unbalanced_quotes_are_still_read_word_by_word() {
        assert_eq!(
            claude("        with:\n          claude_args: --dangerously-skip-permissions --append-system-prompt Don't push\n")
                .as_deref(),
            Some("8: claude_args has --dangerously-skip-permissions")
        );
    }

    #[test]
    fn claude_escaped_quotes_do_not_end_a_quoted_value() {
        assert_eq!(
            claude("        with:\n          claude_args: |\n            --append-system-prompt \"Say \\\" # done\\\"\" --allowedTools Bash\n")
                .as_deref(),
            Some("8: claude_args allows `Bash` through --allowedTools")
        );
    }

    #[test]
    fn claude_ask_rules_close_the_shell_in_ci() {
        assert_eq!(
            claude(
                "        with:\n          settings: '{\"permissions\": {\"allow\": [\"Bash\"], \"ask\": [\"Bash\"]}}'\n"
            ),
            None
        );
    }

    #[test]
    fn gemini_excluded_shell_is_not_broad_and_hash_comments_are_not_stripped() {
        assert_eq!(
            gemini(
                "        with:\n          settings: '{\"tools\": {\"core\": [\"run_shell_command\"], \"exclude\": [\"run_shell_command\"]}}'\n"
            ),
            None
        );
        assert_eq!(
            gemini(
                "        with:\n          settings: |\n            {\n              # shell off\n              \"tools\": {\"core\": []}\n            }\n"
            ),
            None
        );
    }
}
