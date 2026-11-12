use json_comments::StripComments;
use serde_json::Value;
use yamlpath::Route;

use super::{Input, input, input_route, lookup};
use crate::agent::{Agent, AgentStep};

const CLAUDE_ALLOWED_TOOLS_FLAGS: [&str; 2] = ["--allowedTools", "--allowed-tools"];
const CLAUDE_DISALLOWED_TOOLS_FLAGS: [&str; 2] = ["--disallowedTools", "--disallowed-tools"];
const CLAUDE_SKIP_PERMISSIONS_FLAG: &str = "--dangerously-skip-permissions";
const CLAUDE_PERMISSION_MODE_FLAG: &str = "--permission-mode";
const CLAUDE_SETTINGS_FLAG: &str = "--settings";
const CLAUDE_BYPASS_MODE: &str = "bypassPermissions";
const CLAUDE_BYPASS_DISABLED: &str = "disable";
const CLAUDE_SHELL_TOOL: &str = "Bash";
const CLAUDE_LEGACY_ALLOWED_INPUT: &str = "allowed_tools";
const CLAUDE_LEGACY_DISALLOWED_INPUT: &str = "disallowed_tools";
const CODEX_BROAD_INPUTS: [(&str, &str); 3] = [
    ("sandbox", "danger-full-access"),
    ("permission-profile", ":danger-full-access"),
    ("safety-strategy", "unsafe"),
];
const GEMINI_SHELL_TOOLS: [&str; 2] = ["run_shell_command", "ShellTool"];
const PROGRAM_RUNNERS: [&str; 25] = [
    "bash", "sh", "zsh", "dash", "ksh", "fish", "pwsh", "python", "node", "deno", "bun", "perl",
    "ruby", "env", "eval", "exec", "sudo", "doas", "xargs", "nohup", "timeout", "nice", "command",
    "busybox", "time",
];

#[derive(Debug)]
pub struct BroadTool<'w> {
    pub setting: String,
    pub route: Route<'w>,
}

pub fn broad_tool<'w>(step: &AgentStep<'w>) -> Option<BroadTool<'w>> {
    match step.agent {
        Agent::ClaudeCode => ClaudePermissions::of(step).broad_tool(),
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
    denied: Vec<String>,
}

impl<'w> ClaudePermissions<'w> {
    fn of(step: &AgentStep<'w>) -> Self {
        let mut permissions = Self::default();
        if let Some((key, value)) = input(step, "claude_args") {
            permissions.read_args(key, value, &input_route(step, key));
        }
        if let Some((key, value)) = input(step, "settings") {
            permissions.read_settings(key, value, &input_route(step, key));
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

    fn broad_tool(self) -> Option<BroadTool<'w>> {
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
        self.shell_allows
            .into_iter()
            .find(|(tool, _)| {
                !denied.iter().any(|deny| {
                    deny == tool
                        || pattern_of(deny, CLAUDE_SHELL_TOOL).is_some_and(matches_every_command)
                })
            })
            .map(|(_, setting)| setting)
    }

    fn read_args(&mut self, key: &str, value: &str, route: &Route<'w>) {
        let without_comment_lines = value
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        let Some(args) = shlex::split(before_comment(&without_comment_lines)) else {
            return;
        };
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
        string_array(permissions.get("deny")).for_each(|tool| self.deny(tool));
    }

    fn allow(&mut self, tool: &str, setting: String, route: &Route<'w>) {
        let any_command = pattern_of(tool, CLAUDE_SHELL_TOOL).is_some_and(|pattern| {
            matches_every_command(pattern)
                || pattern
                    .trim()
                    .strip_suffix('*')
                    .is_some_and(|prefix| runs_any_program(prefix.trim_end_matches([':', ' '])))
        });
        if any_command {
            self.shell_allows.push((
                tool.trim().to_string(),
                BroadTool {
                    setting,
                    route: route.clone(),
                },
            ));
        }
    }

    fn deny(&mut self, tool: &str) {
        self.denied.push(tool.trim().to_string());
    }
}

fn before_comment(text: &str) -> &str {
    let mut quote = None;
    let mut previous = ' ';
    for (index, c) in text.char_indices() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => {}
            None if c == '\'' || c == '"' => quote = Some(c),
            None if c == '#' && previous.is_whitespace() => return &text[..index],
            None => {}
        }
        previous = c;
    }
    text
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

fn runs_any_program(command: &str) -> bool {
    let mut words = command.split_whitespace();
    words.next().is_some_and(|program| {
        let name = program.rsplit('/').next().unwrap_or(program);
        let name = name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
        PROGRAM_RUNNERS.contains(&name)
    }) && words.all(|word| word.starts_with('-'))
}

fn gemini_settings<'w>(step: &AgentStep<'w>) -> Option<BroadTool<'w>> {
    let (key, value) = match lookup(step, "settings") {
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
    let settings: Value = serde_json::from_reader(StripComments::new(value.as_bytes())).ok()?;
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
    let tool = string_array(Some(core)).find(|tool| {
        GEMINI_SHELL_TOOLS.iter().any(|shell| {
            pattern_of(tool, shell)
                .is_some_and(|pattern| matches_every_command(pattern) || runs_any_program(pattern))
        })
    })?;
    Some(BroadTool {
        setting: format!("{key} allows `{tool}` in tools.core"),
        route,
    })
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
            "        with:\n          claude_args: --allowedTools \"Bash(gh issue view:*),Bash(npm test)\"\n",
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
                "        with:\n          claude_args: --allowedTools \"Bash(npm run test) Read\"\n"
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
            claude(
                "        with:\n          claude_args: --allowedTools \"Bash(python scripts/check.py)\"\n"
            ),
            None
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
        ] {
            let with =
                format!("        with:\n          claude_args: --allowedTools \"{scoped}\"\n");
            assert_eq!(claude(&with), None, "{scoped}");
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
}
