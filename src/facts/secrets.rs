use github_actions_models::common::Env;
use github_actions_models::common::expr::LoE;
use yamlpath::{Component, Route, route};

use super::{Input, input, input_route, lookup, references_variable};
use crate::agent::{Agent, AgentStep};
use crate::workflow::Workflow;

const EXPRESSION_START: &str = "${{";
const EXPRESSION_END: &str = "}}";
const SECRETS_CONTEXT: &str = "secrets";
const SHORT_LIVED_SECRET: &str = "GITHUB_TOKEN";
const EVERY_SECRET: &str = "every secret";
const CLAUDE_TOKEN_INPUT: &str = "github_token";
const TOKEN_VARIABLES: [&str; 2] = ["GITHUB_TOKEN", "GH_TOKEN"];
const CLAUDE_CREDENTIAL_INPUTS: [&str; 4] = [
    "anthropic_api_key",
    "claude_code_oauth_token",
    "github_token",
    "ssh_signing_key",
];
const CODEX_CREDENTIAL_INPUTS: [&str; 1] = ["openai-api-key"];
const GEMINI_CREDENTIAL_INPUTS: [&str; 2] = ["gemini_api_key", "google_api_key"];
const CLAUDE_CREDENTIAL_VARIABLES: [&str; 4] = [
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "ANTHROPIC_CUSTOM_HEADERS",
];
const CLAUDE_CREDENTIAL_PREFIXES: [&str; 2] = ["AWS_", "ANTHROPIC_FOUNDRY_"];
const CODEX_CREDENTIAL_VARIABLES: [&str; 1] = ["OPENAI_API_KEY"];
const GEMINI_CREDENTIAL_VARIABLES: [&str; 2] = ["GEMINI_API_KEY", "GOOGLE_API_KEY"];
const GEMINI_SETTINGS_INPUT: &str = "settings";

#[derive(Debug)]
pub struct SecretUse<'w> {
    pub setting: String,
    pub route: Route<'w>,
}

pub fn long_lived_token<'w>(workflow: &'w Workflow, step: &AgentStep<'w>) -> Option<SecretUse<'w>> {
    match step.agent {
        Agent::ClaudeCode => {
            let (key, value) = input(step, CLAUDE_TOKEN_INPUT)?;
            let secret = stored_secrets(value).into_iter().next()?;
            Some(SecretUse {
                setting: format!("{key} uses {secret}, a stored token rather than GITHUB_TOKEN or an app token"),
                route: input_route(step, key),
            })
        }
        Agent::Codex | Agent::GeminiCli => visible_env(workflow, step)
            .into_iter()
            .filter(|variable| TOKEN_VARIABLES.contains(&variable.name))
            .find_map(|variable| {
                let secret = stored_secrets(&variable.value).into_iter().next()?;
                Some(SecretUse {
                    setting: format!(
                        "env {} uses {secret}, a stored token rather than GITHUB_TOKEN or an app token",
                        variable.name
                    ),
                    route: variable.route,
                })
            }),
    }
}

pub fn other_secret<'w>(workflow: &'w Workflow, step: &AgentStep<'w>) -> Option<SecretUse<'w>> {
    let credential_inputs: &[&str] = match step.agent {
        Agent::ClaudeCode => &CLAUDE_CREDENTIAL_INPUTS,
        Agent::Codex => &CODEX_CREDENTIAL_INPUTS,
        Agent::GeminiCli => &GEMINI_CREDENTIAL_INPUTS,
    };
    if let LoE::Literal(with) = &step.step.with {
        let from_input = with
            .iter()
            .filter(|(key, _)| {
                !credential_inputs
                    .iter()
                    .any(|credential| credential.eq_ignore_ascii_case(key))
            })
            .find_map(|(key, value)| {
                let secret = stored_secrets(&value.to_string()).into_iter().next()?;
                Some(SecretUse {
                    setting: format!("{key} passes {secret} to the agent"),
                    route: input_route(step, key),
                })
            });
        if from_input.is_some() {
            return from_input;
        }
    }
    let gemini_settings = match lookup(step, GEMINI_SETTINGS_INPUT) {
        Input::Text { value, .. } => value,
        Input::Missing | Input::Unreadable => "",
    };
    visible_env(workflow, step)
        .into_iter()
        .filter(|variable| !TOKEN_VARIABLES.contains(&variable.name))
        .filter(|variable| match step.agent {
            Agent::ClaudeCode => {
                !CLAUDE_CREDENTIAL_VARIABLES.contains(&variable.name)
                    && !CLAUDE_CREDENTIAL_PREFIXES
                        .iter()
                        .any(|prefix| variable.name.starts_with(prefix))
            }
            Agent::Codex => !CODEX_CREDENTIAL_VARIABLES.contains(&variable.name),
            Agent::GeminiCli => {
                !GEMINI_CREDENTIAL_VARIABLES.contains(&variable.name)
                    && references_variable(gemini_settings, variable.name)
            }
        })
        .find_map(|variable| {
            let secret = stored_secrets(&variable.value).into_iter().next()?;
            Some(SecretUse {
                setting: format!("env {} passes {secret} to the agent", variable.name),
                route: variable.route,
            })
        })
}

struct Variable<'w> {
    name: &'w str,
    value: String,
    route: Route<'w>,
}

fn visible_env<'w>(workflow: &'w Workflow, step: &AgentStep<'w>) -> Vec<Variable<'w>> {
    let scopes: [(&'w LoE<Env>, Route<'w>); 3] = [
        (&step.step.shared.env, step.route.with_key("env")),
        (&step.job.env, route!["jobs", step.job_id, "env"]),
        (&workflow.model.env, route!["env"]),
    ];
    let mut variables: Vec<Variable<'w>> = Vec::new();
    for (env, scope) in scopes {
        let LoE::Literal(env) = env else {
            continue;
        };
        for (name, value) in env.iter() {
            if variables.iter().any(|variable| variable.name == name) {
                continue;
            }
            variables.push(Variable {
                name,
                value: value.to_string(),
                route: scope.with_key(Component::from(name.as_str())),
            });
        }
    }
    variables
}

fn stored_secrets(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(EXPRESSION_START) {
        let expression = &rest[start + EXPRESSION_START.len()..];
        let end = expression.find(EXPRESSION_END).unwrap_or(expression.len());
        found.extend(secrets_in(&expression[..end]).into_iter().filter(|secret| {
            !secret.eq_ignore_ascii_case(&format!("{SECRETS_CONTEXT}.{SHORT_LIVED_SECRET}"))
        }));
        rest = &expression[end..];
    }
    found
}

fn secrets_in(expression: &str) -> Vec<String> {
    let lowercase = expression.to_ascii_lowercase();
    lowercase
        .match_indices(SECRETS_CONTEXT)
        .filter(|(start, _)| {
            !lowercase[..*start]
                .ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        })
        .filter_map(|(start, _)| {
            let after = &expression[start + SECRETS_CONTEXT.len()..];
            if let Some(name) = after.strip_prefix('.') {
                let length = name
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(name.len());
                return (length > 0).then(|| format!("{SECRETS_CONTEXT}.{}", &name[..length]));
            }
            if let Some(index) = after.trim_start().strip_prefix('[') {
                let name = index
                    .trim_start()
                    .trim_start_matches(['\'', '"'])
                    .split(['\'', '"', ']'])
                    .next()
                    .unwrap_or_default();
                return Some(format!("{SECRETS_CONTEXT}.{name}"));
            }
            (!after.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_'))
                .then(|| EVERY_SECRET.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::agent::agent_steps;

    type Finder = for<'w> fn(&'w Workflow, &AgentStep<'w>) -> Option<SecretUse<'w>>;

    fn found(finder: Finder, source: &str) -> Option<String> {
        let workflow = Workflow::parse(Path::new("test.yml"), source.to_string()).unwrap();
        let steps = agent_steps(&workflow);
        finder(&workflow, &steps[0]).map(|found| {
            let (line, _) = workflow.position(&found.route).unwrap();
            format!("{line}: {}", found.setting)
        })
    }

    fn workflow(env: &str, uses: &str, step: &str) -> String {
        format!(
            "on: issues
{env}jobs:
  agent:
    runs-on: ubuntu-latest
    steps:
      - uses: {uses}
{step}"
        )
    }

    #[test]
    fn claude_token_input_from_a_stored_secret_is_long_lived() {
        let source = workflow(
            "",
            "anthropics/claude-code-action@v1",
            "        with:
          github_token: ${{ secrets.BOT_PAT }}
",
        );
        assert_eq!(
            found(long_lived_token, &source),
            Some("8: github_token uses secrets.BOT_PAT, a stored token rather than GITHUB_TOKEN or an app token".to_string())
        );
    }

    #[test]
    fn short_lived_tokens_are_not_long_lived() {
        let values = [
            "${{ secrets.GITHUB_TOKEN }}",
            "${{ secrets.github_token }}",
            "${{ github.token }}",
            "${{ steps.app-token.outputs.token }}",
            "${{ steps.mint.outputs.token || secrets.GITHUB_TOKEN || github.token }}",
        ];
        for value in values {
            let source = workflow(
                "",
                "anthropics/claude-code-action@v1",
                &format!("        with:\n          github_token: {value}\n"),
            );
            assert_eq!(found(long_lived_token, &source), None, "{value}");
        }
    }

    #[test]
    fn codex_and_gemini_tokens_come_from_the_nearest_env() {
        let codex = workflow(
            "env:\n  GH_TOKEN: ${{ secrets.ORG_PAT }}\n",
            "openai/codex-action@v1",
            "",
        );
        assert_eq!(
            found(long_lived_token, &codex),
            Some("3: env GH_TOKEN uses secrets.ORG_PAT, a stored token rather than GITHUB_TOKEN or an app token".to_string())
        );
        let overridden = workflow(
            "env:\n  GITHUB_TOKEN: ${{ secrets.ORG_PAT }}\n",
            "google-github-actions/run-gemini-cli@v0",
            "        env:\n          GITHUB_TOKEN: ${{ github.token }}\n",
        );
        assert_eq!(found(long_lived_token, &overridden), None);
    }

    #[test]
    fn claude_replaces_token_variables_in_its_env() {
        let source = workflow(
            "",
            "anthropics/claude-code-action@v1",
            "        env:\n          GH_TOKEN: ${{ secrets.ORG_PAT }}\n",
        );
        assert_eq!(found(long_lived_token, &source), None);
        assert_eq!(found(other_secret, &source), None);
    }

    #[test]
    fn secrets_in_inputs_other_than_the_agent_credentials_are_found() {
        let source = workflow(
            "",
            "anthropics/claude-code-action@v1",
            "        with:
          anthropic_api_key: ${{ secrets.ANTHROPIC_API_KEY }}
          claude_args: --mcp-config '{\"env\":{\"KEY\":\"${{ secrets['SENTRY_TOKEN'] }}\"}}'
",
        );
        assert_eq!(
            found(other_secret, &source),
            Some("9: claude_args passes secrets.SENTRY_TOKEN to the agent".to_string())
        );
    }

    #[test]
    fn job_env_secrets_reach_claude_and_codex_but_their_own_keys_do_not_count() {
        let claude = workflow(
            "env:\n  AWS_SECRET_ACCESS_KEY: ${{ secrets.AWS }}\n  ANTHROPIC_API_KEY: ${{ secrets.KEY }}\n  NPM_TOKEN: ${{ secrets.NPM_TOKEN }}\n",
            "anthropics/claude-code-action@v1",
            "",
        );
        assert_eq!(
            found(other_secret, &claude),
            Some("5: env NPM_TOKEN passes secrets.NPM_TOKEN to the agent".to_string())
        );
        let codex = workflow(
            "env:\n  OPENAI_API_KEY: ${{ secrets.OPENAI_API_KEY }}\n  ALL: ${{ toJSON(secrets) }}\n",
            "openai/codex-action@v1",
            "",
        );
        assert_eq!(
            found(other_secret, &codex),
            Some("4: env ALL passes every secret to the agent".to_string())
        );
    }

    #[test]
    fn gemini_sees_only_env_secrets_its_settings_refer_to() {
        let unreferenced = workflow(
            "env:\n  SENTRY_TOKEN: ${{ secrets.SENTRY_TOKEN }}\n",
            "google-github-actions/run-gemini-cli@v0",
            "",
        );
        assert_eq!(found(other_secret, &unreferenced), None);
        let referenced = workflow(
            "env:\n  SENTRY_TOKEN: ${{ secrets.SENTRY_TOKEN }}\n  GEMINI_API_KEY: ${{ secrets.GEMINI_API_KEY }}\n",
            "google-github-actions/run-gemini-cli@v0",
            "        with:
          settings: |-
            {\"mcpServers\": {\"sentry\": {\"env\": {\"TOKEN\": \"${SENTRY_TOKEN}\", \"KEY\": \"$GEMINI_API_KEY\"}}}}
",
        );
        assert_eq!(
            found(other_secret, &referenced),
            Some("3: env SENTRY_TOKEN passes secrets.SENTRY_TOKEN to the agent".to_string())
        );
    }

    #[test]
    fn only_secrets_inside_expressions_and_with_word_boundaries_count() {
        let source = workflow(
            "env:\n  NOTE: see secrets.DOCS\n  OTHER: ${{ vars.mysecrets }}\n  THIRD: ${{ github.event.secrets }}\n",
            "openai/codex-action@v1",
            "",
        );
        assert_eq!(found(other_secret, &source), None);
    }
}
