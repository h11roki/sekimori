use json_comments::CommentSettings;
use serde_json::Value;
use yamlpath::Route;

use super::{
    Input, SETTINGS_INPUT, before_comment, enabled, input, input_route, is_assignment, lookup,
    program_name,
};
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
const CLAUDE_LEGACY_INPUTS_REMOVED_IN: u64 = 1;
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
const OPTION_EXEC_COMMANDS: [&str; 5] = ["find", "fd", "rg", "sed", "tar"];
const RUN_SUBCOMMANDS: [(&str, &[&str]); 14] = [
    (
        "npm",
        &["exec", "x", "init", "create", "install", "i", "add"],
    ),
    ("pnpm", &["dlx", "exec", "create", "add"]),
    ("yarn", &["dlx", "exec", "create", "add"]),
    ("bun", &["x", "create", "add", "install", "i"]),
    ("pip", &["install"]),
    ("pipx", &["run", "install"]),
    ("deno", &["run", "eval"]),
    ("git", &["submodule", "rebase", "bisect"]),
    ("uv", &["run"]),
    ("poetry", &["run"]),
    ("go", &["run"]),
    ("cargo", &["run"]),
    ("dotnet", &["run"]),
    ("bundle", &["exec"]),
];
const PROJECT_CODE_TOOLS: [&str; 32] = [
    "pre-commit",
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
const FILE_WRITERS: [&str; 11] = [
    "tee", "cp", "mv", "install", "rsync", "patch", "dd", "truncate", "sed", "curl", "wget",
];
const SUBCOMMAND_FILE_WRITERS: [(&str, &[&[&str]]); 5] = [
    (
        "git",
        &[
            &["apply"],
            &["am"],
            &["checkout"],
            &["switch"],
            &["pull"],
            &["merge"],
            &["cherry-pick"],
            &["clone"],
            &["reset"],
            &["restore"],
            &["stash"],
            &["rebase"],
            &["revert"],
            &["worktree"],
            &["submodule"],
            &["config"],
        ],
    ),
    ("npm", &[&["pkg"], &["config"]]),
    ("pnpm", &[&["pkg"], &["config"]]),
    ("yarn", &[&["config"]]),
    (
        "gh",
        &[
            &["pr", "checkout"],
            &["run", "download"],
            &["release", "download"],
            &["repo", "clone"],
        ],
    ),
];
const OPTION_VALUE_FLAGS: [&str; 13] = [
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--exec-path",
    "-R",
    "--repo",
    "--prefix",
    "-w",
    "--workspace",
    "--cwd",
    "--filter",
];
const CLAUDE_EDIT_TOOLS: [&str; 3] = ["Edit", "MultiEdit", "Write"];
const CLAUDE_EDIT_RULE: &str = "Edit";
const CLAUDE_AUTO_MODE: &str = "auto";
const CLAUDE_PROMPT_INPUT: &str = "prompt";
const CLAUDE_TRACK_PROGRESS_INPUT: &str = "track_progress";
const CLAUDE_NO_AUTO_MODE_INPUTS: [&str; 3] = [
    "allowed_non_write_users",
    "allowed_bots",
    "path_to_claude_code_executable",
];
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
            let permissions = ClaudePermissions::of(step);
            let edits_by_default = claude_code_edits_by_default(step, &permissions);
            permissions.broad_tool(edits_by_default)
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
    file_writers: Vec<String>,
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
        let legacy_inputs_removed = step.agent == Agent::ClaudeCode
            && step
                .major_version
                .is_some_and(|major| major >= CLAUDE_LEGACY_INPUTS_REMOVED_IN);
        if step.agent == Agent::ClaudeBase || legacy_inputs_removed {
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

    fn mode(&self) -> Option<&str> {
        self.cli_mode
            .as_ref()
            .or(self.settings_mode.as_ref())
            .map(|(mode, _)| mode.as_str())
    }

    fn editing(&self, edits_by_default: bool) -> Option<Editing> {
        let edit_mode = self
            .mode()
            .is_some_and(|mode| [CLAUDE_ACCEPT_EDITS_MODE, CLAUDE_AUTO_MODE].contains(&mode));
        let edit_tools = (edits_by_default || self.edit_allowed || edit_mode)
            && !self
                .denied
                .iter()
                .any(|deny| pattern_of(deny, CLAUDE_EDIT_RULE).is_some_and(covers_every_file));
        if edit_tools {
            return Some(Editing::Tools);
        }
        self.file_writers
            .iter()
            .find(|writer| !shell_denied(&self.denied, writer))
            .map(|writer| Editing::Shell(writer.clone()))
    }

    fn broad_tool(self, edits_by_default: bool) -> Option<BroadTool<'w>> {
        let editing = self.editing(edits_by_default);
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
        let denied = &self.denied;
        let allowed = |(tool, _): &(String, BroadTool<'w>)| !shell_denied(denied, tool);
        let any_command = self.shell_allows.into_iter().find(allowed);
        let project_code = || {
            self.project_code_allows
                .into_iter()
                .find(allowed)
                .zip(editing)
                .map(|((tool, setting), editing)| {
                    let through = match editing {
                        Editing::Tools => String::new(),
                        Editing::Shell(writer) => format!(" with `{writer}`"),
                    };
                    (
                        tool,
                        BroadTool {
                            setting: format!("{}{EDITABLE_CODE_NOTE}{through}", setting.setting),
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
            .any(|edit| pattern_of(tool, edit).is_some_and(covers_every_file))
        {
            self.edit_allowed = true;
        }
        let Some(pattern) = pattern_of(tool, CLAUDE_SHELL_TOOL).map(str::trim) else {
            return;
        };
        let prefix = pattern
            .strip_suffix('*')
            .map(|prefix| prefix.trim_end_matches([':', ' ']));
        if prefix.is_some_and(writes_files) {
            self.file_writers.push(tool.to_string());
        }
        let reach = match prefix {
            Some(prefix) => command_reach(prefix),
            None if pattern.is_empty() => Reach::AnyCommand,
            None if runs_project_code(pattern) => Reach::ProjectCode,
            None => Reach::Fixed,
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

enum Editing {
    Tools,
    Shell(String),
}

fn shell_denied(denied: &[String], tool: &str) -> bool {
    let command = |rule: &str| {
        pattern_of(rule, CLAUDE_SHELL_TOOL).map(|pattern| {
            let pattern = pattern.trim();
            match pattern.strip_suffix('*') {
                Some(prefix) => (prefix.trim_end_matches([':', ' ']).to_string(), true),
                None => (pattern.to_string(), false),
            }
        })
    };
    let allowed = command(tool);
    denied.iter().any(|deny| {
        deny == tool
            || pattern_of(deny, CLAUDE_SHELL_TOOL).is_some_and(matches_every_command)
            || command(deny)
                .zip(allowed.as_ref())
                .is_some_and(|((deny, any_after), (allow, _))| {
                    any_after && starts_with_words(allow, &deny)
                })
    })
}

fn starts_with_words(command: &str, prefix: &str) -> bool {
    command
        .trim()
        .strip_prefix(prefix.trim())
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(' '))
}

fn gemini_excluded(excluded: &[&str], tool: &str) -> bool {
    excluded.iter().any(|exclusion| {
        GEMINI_SHELL_TOOLS.contains(exclusion)
            || *exclusion == tool.trim()
            || GEMINI_SHELL_TOOLS.iter().any(|shell| {
                pattern_of(exclusion, shell).is_some_and(|exclusion| {
                    GEMINI_SHELL_TOOLS.iter().any(|name| {
                        pattern_of(tool, name)
                            .is_some_and(|command| starts_with_words(command, exclusion))
                    })
                })
            })
    })
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
    let words: Vec<&str> = prefix
        .split_whitespace()
        .skip_while(|word| is_assignment(word))
        .collect();
    let Some((&program, rest)) = words.split_first() else {
        return Reach::AnyCommand;
    };
    if matches_every_command(program) {
        return Reach::AnyCommand;
    }
    let name = command_name(program);
    let arguments: Vec<&str> = rest
        .iter()
        .copied()
        .filter(|word| !word.starts_with('-'))
        .collect();
    let runs_anything = |arguments: &[&str]| match arguments {
        [] => true,
        [only] => INTERPRETERS.contains(&command_name(only)),
        _ => false,
    };
    let takes_any_code = WRAPPERS.contains(&name)
        || OPTION_EXEC_COMMANDS.contains(&name)
        || (INTERPRETERS.contains(&name) && arguments.is_empty())
        || (PACKAGE_RUNNERS.contains(&name) && runs_anything(&arguments))
        || RUN_SUBCOMMANDS
            .iter()
            .find(|(tool, _)| *tool == name)
            .is_some_and(|(_, runs)| match arguments.split_first() {
                None => true,
                Some((first, after)) => runs.contains(first) && runs_anything(after),
            });
    if takes_any_code {
        Reach::AnyCommand
    } else if runs_project_code(prefix) {
        Reach::ProjectCode
    } else {
        Reach::Fixed
    }
}

fn writes_files(prefix: &str) -> bool {
    let mut words = prefix
        .split_whitespace()
        .skip_while(|word| is_assignment(word));
    let Some(program) = words.next() else {
        return false;
    };
    let name = command_name(program);
    let Some((_, writers)) = SUBCOMMAND_FILE_WRITERS
        .iter()
        .find(|(tool, _)| *tool == name)
    else {
        return FILE_WRITERS.contains(&name);
    };
    let mut subcommands = Vec::new();
    while let Some(word) = words.next() {
        if OPTION_VALUE_FLAGS.contains(&word) {
            words.next();
        } else if !word.starts_with('-') {
            subcommands.push(word);
        }
    }
    writers
        .iter()
        .any(|writer| writer.starts_with(&subcommands) || subcommands.starts_with(writer))
}

fn covers_every_file(pattern: &str) -> bool {
    pattern.trim().chars().all(|c| matches!(c, '*' | '/' | '.'))
}

fn command_name(program: &str) -> &str {
    program_name(program).trim_end_matches(|c: char| c.is_ascii_digit() || c == '.')
}

fn runs_project_code(command: &str) -> bool {
    let program = command
        .split_whitespace()
        .find(|word| !is_assignment(word))
        .unwrap_or_default();
    let name = command_name(program);
    PROJECT_CODE_TOOLS.contains(&name)
        || PACKAGE_RUNNERS.contains(&name)
        || INTERPRETERS.contains(&name)
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
                pattern_of(tool, shell).is_some_and(reaches) && !gemini_excluded(&excluded, tool)
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

fn claude_code_edits_by_default(step: &AgentStep, permissions: &ClaudePermissions) -> bool {
    if step.agent != Agent::ClaudeCode || permissions.cli_mode.is_some() {
        return false;
    }
    let agent_mode = input(step, CLAUDE_PROMPT_INPUT)
        .is_some_and(|(_, value)| !value.trim().is_empty())
        && !enabled(step, CLAUDE_TRACK_PROGRESS_INPUT);
    !agent_mode
        || permissions.settings_mode.is_none()
            && !CLAUDE_NO_AUTO_MODE_INPUTS
                .iter()
                .any(|name| input(step, name).is_some_and(|(_, value)| !value.trim().is_empty()))
}

pub(super) fn claude_may_edit(step: &AgentStep) -> bool {
    let permissions = ClaudePermissions::of(step);
    let edits_by_default = claude_code_edits_by_default(step, &permissions);
    permissions.editing(edits_by_default).is_some()
        || permissions.broad_tool(edits_by_default).is_some()
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
                !gemini_excluded(&excluded, tool)
                    && names
                        .iter()
                        .any(|name| pattern_of(tool, name).is_some_and(opens))
            })
        });
        allowed && !names.iter().any(|name| excluded.contains(name))
    };
    GEMINI_FILE_EDIT_TOOLS
        .iter()
        .any(|names| usable(names, |_| true))
        || usable(&GEMINI_SHELL_TOOLS, |command| {
            opens_any_command(command) || writes_files(command)
        })
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
    fn claude_legacy_inputs_are_read_only_before_v1() {
        let with = "        with:\n          allowed_tools: \"Read,Bash\"\n";
        for uses in [
            "anthropics/claude-code-action@beta",
            "anthropics/claude-code-action@v0.0.63",
            "anthropics/claude-code-action@0123456789abcdef0123456789abcdef01234567",
            "anthropics/claude-code-action@0123456789abcdef0123456789abcdef01234567 # v0.0.63",
            "anthropics/claude-code-base-action@v1",
            "anthropics/claude-code-action@beta # v1",
            "anthropics/claude-code-action@main # v1.0.5",
            "anthropics/claude-code-action@0123456789abcdef0123456789abcdef01234567 # v1 migration TODO",
            "anthropics/claude-code-action@0123456789abcdef0123456789abcdef01234567 # vendored",
            "anthropics/claude-code-action@v+1",
            "anthropics/claude-code-action@0123456789abcdef0123456789abcdef01234567 # TODO: bump to @v1",
            "anthropics/claude-code-action@0123456789abcdef0123456789abcdef01234567 # renovate: depName=v2",
        ] {
            assert_eq!(
                broad_setting(uses, with).as_deref(),
                Some("8: allowed_tools allows `Bash`"),
                "{uses}"
            );
        }
        for uses in [
            "anthropics/claude-code-action@v1",
            "anthropics/claude-code-action@v1.0.5",
            "anthropics/claude-code-action@0123456789abcdef0123456789abcdef01234567 # v1.0.5",
            "anthropics/claude-code-action@0123456789abcdef0123456789abcdef01234567 # v2 # sekimori: ignore[pr-head-checkout]",
            "anthropics/claude-code-action@refs/tags/v1.0.5",
            "anthropics/claude-code-action@V1",
            "anthropics/claude-code-action@0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef # v1.0.5",
            "anthropics/claude-code-action@0123456789abcdef0123456789abcdef01234567 # ratchet:anthropics/claude-code-action@v1",
            "anthropics/claude-code-action@0123456789abcdef0123456789abcdef01234567 # pin@v1.0.5",
            "anthropics/claude-code-action@0123456789abcdef0123456789abcdef01234567 # tag=v1.0.5",
            "anthropics/claude-code-action@v10",
        ] {
            assert_eq!(broad_setting(uses, with), None, "{uses}");
        }
        assert_eq!(
            broad_setting(
                "anthropics/claude-code-action@v1",
                "        with:\n          claude_args: --allowedTools Bash\n          disallowed_tools: Bash\n"
            )
            .as_deref(),
            Some("8: claude_args allows `Bash` through --allowedTools")
        );
    }

    #[test]
    fn claude_shell_denied_everywhere_is_not_broad_but_a_scoped_deny_is() {
        for with in [
            "        with:\n          claude_args: --allowedTools Bash --disallowedTools Bash\n",
            "        with:\n          settings: '{\"permissions\": {\"allow\": [\"Bash\"], \"deny\": [\"Bash\"]}}'\n",
        ] {
            assert_eq!(claude(with), None, "{with}");
        }
        assert_eq!(
            broad_setting(
                "anthropics/claude-code-action@beta",
                "        with:\n          claude_args: --allowedTools Bash\n          disallowed_tools: Bash\n"
            ),
            None
        );
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
    fn tools_that_run_commands_through_an_option_are_broad_when_allowed_by_prefix() {
        for rule in [
            "Bash(find:*)",
            "Bash(find . -name:*)",
            "Bash(fd *)",
            "Bash(rg:*)",
            "Bash(sed -n:*)",
            "Bash(tar xzf:*)",
        ] {
            assert_eq!(
                claude(&format!(
                    "        with:\n          claude_args: --allowedTools \"{rule}\"\n"
                ))
                .as_deref(),
                Some(format!("8: claude_args allows `{rule}` through --allowedTools").as_str()),
                "{rule}"
            );
        }
        assert_eq!(
            base(
                "        with:\n          claude_args: --allowedTools \"Bash(find . -name '*.rs')\"\n"
            ),
            None
        );
        assert!(
            gemini("        with:\n          settings: '{\"tools\": {\"core\": [\"run_shell_command(find)\", \"read_file\"]}}'\n")
                .is_some()
        );
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
    fn claude_code_cannot_edit_without_auto_mode_or_when_edits_are_denied() {
        let args = |rest: &str| {
            format!(
                "        with:\n          prompt: review\n          claude_args: --allowedTools \"Bash(npm test:*)\"{rest}\n"
            )
        };
        assert!(claude(&args("")).is_some());
        assert_eq!(claude(&args(" --disallowedTools Edit")), None);
        assert_eq!(claude(&args(" --permission-mode default")), None);
        assert!(claude(&args(" --permission-mode acceptEdits")).is_some());
        assert_eq!(
            claude(&format!("{}          allowed_bots: dependabot\n", args(""))),
            None
        );
        assert_eq!(
            base(
                "        with:\n          claude_args: --allowedTools \"Edit(docs/**),Bash(npm test:*)\"\n"
            ),
            None
        );
        assert!(
            claude(
                "        with:\n          claude_args: --allowedTools \"Bash(pip3 install:*)\"\n"
            )
            .is_some()
        );
        assert!(claude("        with:\n          claude_args: --allowedTools \"Bash(npx tsx scripts/build.ts)\"\n").is_some());
        assert!(claude("        with:\n          claude_args: --allowedTools \"Bash(CI=true npm test:*)\"\n").is_some());
        assert_eq!(claude(&args(" --disallowedTools \"Edit(**)\"")), None);
        assert_eq!(claude(&args(" --disallowedTools \"Edit(//**)\"")), None);
        let tag_mode = "        with:\n          settings: '{\"permissions\": {\"defaultMode\": \"default\"}}'\n          claude_args: --allowedTools \"Bash(npm test:*)\"\n";
        assert!(claude(tag_mode).is_some());
        assert!(
            claude(&format!(
                "{}          allowed_bots: dependabot\n          track_progress: true\n",
                args("")
            ))
            .is_some()
        );
        assert!(
            base("        with:\n          claude_args: --allowedTools \"Write(**/*),Bash(npm test:*)\"\n")
                .is_some()
        );
        assert!(
            base("        with:\n          claude_args: --allowedTools \"Bash(pipx run:*)\"\n")
                .is_some()
        );
        assert!(
            base("        with:\n          claude_args: --allowedTools \"Edit(/**),Bash(npm test:*)\"\n")
                .is_some()
        );
    }

    #[test]
    fn shell_commands_that_write_files_count_as_edits() {
        for writer in [
            "Bash(sed -i:*)",
            "Bash(sed -n:*)",
            "Bash(tee:*)",
            "Bash(cp:*)",
            "Bash(git apply:*)",
            "Bash(git -C . apply:*)",
            "Bash(git:*)",
            "Bash(git pull:*)",
            "Bash(gh pr:*)",
            "Bash(gh pr checkout:*)",
            "Bash(curl:*)",
            "Bash(git clone:*)",
            "Bash(git --work-tree . apply:*)",
            "Bash(npm pkg:*)",
            "Bash(npm config:*)",
            "Bash(gh -R owner/repo pr checkout:*)",
        ] {
            let with = format!(
                "        with:\n          claude_args: --allowedTools \"{writer},Bash(npm test:*)\"\n"
            );
            assert!(base(&with).is_some(), "{writer}");
        }
        for reader in [
            "Bash(git diff:*)",
            "Bash(cp a.txt b.txt)",
            "Bash(cat:*)",
            "Bash(gh pr view:*)",
        ] {
            let with = format!(
                "        with:\n          claude_args: --allowedTools \"{reader},Bash(npm test:*)\"\n"
            );
            assert_eq!(base(&with), None, "{reader}");
        }
        let settings = "        with:\n          settings: '{\"tools\": {\"core\": [\"run_shell_command(npm test)\", \"run_shell_command(sed -i)\"]}}'\n";
        assert!(gemini(settings).is_some());
        let excluded = "        with:\n          settings: '{\"tools\": {\"core\": [\"run_shell_command(npm test)\", \"run_shell_command(sed -i)\"], \"exclude\": [\"run_shell_command(sed -i)\"]}}'\n";
        assert_eq!(gemini(excluded), None);
        assert_eq!(
            gemini(&excluded.replace(
                "\"exclude\": [\"run_shell_command(sed -i)\"]",
                "\"exclude\": [\"run_shell_command(sed)\"]"
            )),
            None
        );
        assert_eq!(
            base(
                "        with:\n          claude_args: --allowedTools \"Bash(sed -i:*),Bash(npm test:*)\" --disallowedTools \"Bash(sed:*)\"\n"
            ),
            None
        );
        assert_eq!(
            base("        with:\n          claude_args: --allowedTools \"Bash(git apply:*),Bash(npm test:*)\"\n").as_deref(),
            Some("8: claude_args allows `Bash(npm test:*)` through --allowedTools, and the agent can edit the files that command runs with `Bash(git apply:*)`")
        );
        assert_eq!(
            base(
                "        with:\n          claude_args: --allowedTools \"Bash(sed:*),Bash(npm test:*)\" --disallowedTools \"Bash(sed:*)\"\n"
            ),
            None
        );
        assert!(
            base("        with:\n          claude_args: --allowedTools \"Bash(sed:*),Bash(npm test:*)\" --disallowedTools Edit\n")
                .is_some()
        );
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
            "Bash(git:*)",
            "Bash(git submodule:*)",
            "Bash(npm init:*)",
            "Bash(bun x:*)",
            "Bash(deno run:*)",
            "Bash(pip install:*)",
            "Bash(npm install:*)",
            "Bash(uv run python:*)",
        ] {
            let with =
                format!("        with:\n          claude_args: --allowedTools \"{runner}\"\n");
            assert!(base(&with).is_some(), "{runner}");
        }
        for fixed in [
            "Bash(uv run pytest)",
            "Bash(uv run pytest:*)",
            "Bash(bundle exec rspec:*)",
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
