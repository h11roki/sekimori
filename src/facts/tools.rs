use serde_json::Value;
use yamlpath::Route;

use super::{input, input_route};
use crate::agent::{Agent, AgentStep};

const CLAUDE_ALLOWED_TOOLS_FLAGS: [&str; 2] = ["--allowedTools", "--allowed-tools"];
const CLAUDE_SKIP_PERMISSIONS_FLAG: &str = "--dangerously-skip-permissions";
const CLAUDE_PERMISSION_MODE_FLAG: &str = "--permission-mode";
const CLAUDE_BYPASS_MODE: &str = "bypassPermissions";
const CLAUDE_SHELL_TOOL: &str = "Bash";
const CODEX_BROAD_INPUTS: [(&str, &str); 3] = [
    ("sandbox", "danger-full-access"),
    ("permission-profile", ":danger-full-access"),
    ("safety-strategy", "unsafe"),
];
const GEMINI_SHELL_TOOLS: [&str; 2] = ["run_shell_command", "ShellTool"];

#[derive(Debug)]
pub struct BroadTool<'w> {
    pub setting: String,
    pub route: Route<'w>,
}

pub fn broad_tool<'w>(step: &AgentStep<'w>) -> Option<BroadTool<'w>> {
    match step.agent {
        Agent::ClaudeCode => claude_args(step).or_else(|| claude_settings(step)),
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

fn claude_args<'w>(step: &AgentStep<'w>) -> Option<BroadTool<'w>> {
    let (key, value) = input(step, "claude_args")?;
    let without_comments = value
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    let args = shlex::split(&without_comments)?;
    let mut args = args.iter().peekable();
    while let Some(arg) = args.next() {
        let (flag, inline_value) = match arg.split_once('=') {
            Some((flag, value)) => (flag, Some(value)),
            None => (arg.as_str(), None),
        };
        let setting = if flag == CLAUDE_SKIP_PERMISSIONS_FLAG {
            Some(format!("{key} has {flag}"))
        } else if flag == CLAUDE_PERMISSION_MODE_FLAG {
            let mode = inline_value.or_else(|| args.peek().map(|next| next.as_str()));
            (mode == Some(CLAUDE_BYPASS_MODE))
                .then(|| format!("{key} has {flag} {CLAUDE_BYPASS_MODE}"))
        } else if CLAUDE_ALLOWED_TOOLS_FLAGS.contains(&flag) {
            let mut values: Vec<&str> = inline_value.into_iter().collect();
            while let Some(next) = args.next_if(|next| !next.starts_with("--")) {
                values.push(next);
            }
            values
                .iter()
                .flat_map(|value| value.split(','))
                .map(str::trim)
                .find(|tool| is_unrestricted(tool, CLAUDE_SHELL_TOOL))
                .map(|tool| format!("{key} allows `{tool}` through {flag}"))
        } else {
            None
        };
        if let Some(setting) = setting {
            return Some(BroadTool {
                setting,
                route: input_route(step, key),
            });
        }
    }
    None
}

fn claude_settings<'w>(step: &AgentStep<'w>) -> Option<BroadTool<'w>> {
    let (key, value) = input(step, "settings")?;
    let settings: Value = serde_json::from_str(value).ok()?;
    let permissions = settings.get("permissions")?;
    let setting =
        if permissions.get("defaultMode").and_then(Value::as_str) == Some(CLAUDE_BYPASS_MODE) {
            format!("{key} sets permissions.defaultMode to {CLAUDE_BYPASS_MODE}")
        } else {
            let tool = permissions
                .get("allow")?
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
                .find(|tool| is_unrestricted(tool, CLAUDE_SHELL_TOOL))?;
            format!("{key} allows `{tool}` in permissions.allow")
        };
    Some(BroadTool {
        setting,
        route: input_route(step, key),
    })
}

fn gemini_settings<'w>(step: &AgentStep<'w>) -> Option<BroadTool<'w>> {
    let Some((key, value)) = input(step, "settings").filter(|(_, value)| !value.trim().is_empty())
    else {
        return Some(BroadTool {
            setting:
                "no `settings`, so tools.core is unset and every built-in tool runs under --yolo"
                    .to_string(),
            route: step.route.clone(),
        });
    };
    let settings: Value = serde_json::from_str(value).ok()?;
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
    let tool = core
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .find(|tool| {
            GEMINI_SHELL_TOOLS
                .iter()
                .any(|shell| is_unrestricted(tool, shell))
        })?;
    Some(BroadTool {
        setting: format!("{key} allows `{tool}` in tools.core"),
        route,
    })
}

fn is_unrestricted(entry: &str, tool: &str) -> bool {
    match entry.trim().strip_prefix(tool) {
        Some("") => true,
        Some(rest) => rest
            .strip_prefix('(')
            .and_then(|rest| rest.strip_suffix(')'))
            .is_some_and(|pattern| pattern.chars().all(|c| matches!(c, '*' | ':' | ' '))),
        None => false,
    }
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
}
