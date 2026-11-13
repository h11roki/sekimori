use github_actions_expressions::context::Context;
use github_actions_expressions::{Expr, SpannedExpr};
use yamlpath::Route;

use super::{
    ENV_CONTEXT, EnvVariable, Input, NearestEnv, SETTINGS_INPUT, agent_env, expression_spans,
    input, input_route, lookup, parse_expression, references_variable,
};
use crate::agent::{Agent, AgentStep};
use crate::workflow::Workflow;

const SECRETS_CONTEXT: &str = "secrets";
const SHORT_LIVED_SECRET: &str = "GITHUB_TOKEN";
const EVERY_SECRET: &str = "every secret";
const CLAUDE_TOKEN_INPUT: &str = "github_token";
const TOKEN_VARIABLES: [&str; 2] = ["GITHUB_TOKEN", "GH_TOKEN"];
const CLAUDE_AGENT_INPUTS: [&str; 3] = ["prompt", "settings", "claude_args"];
const CODEX_AGENT_INPUTS: [&str; 2] = ["prompt", "codex-args"];
const GEMINI_AGENT_INPUTS: [&str; 2] = ["prompt", "settings"];
const CLAUDE_CREDENTIAL_VARIABLES: [&str; 4] = [
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "ANTHROPIC_CUSTOM_HEADERS",
];
const CLAUDE_CREDENTIAL_PREFIXES: [&str; 2] = ["AWS_", "ANTHROPIC_FOUNDRY_"];
const CODEX_CREDENTIAL_VARIABLES: [&str; 1] = ["OPENAI_API_KEY"];
const GEMINI_CREDENTIAL_VARIABLES: [&str; 2] = ["GEMINI_API_KEY", "GOOGLE_API_KEY"];
const LONG_LIVED: &str = "a stored token rather than GITHUB_TOKEN or an app token";

#[derive(Debug)]
pub struct SecretUse<'w> {
    pub setting: String,
    pub route: Route<'w>,
}

pub struct SecretEnv<'w> {
    env: NearestEnv<'w>,
    secrets: Vec<Option<String>>,
}

impl<'w> SecretEnv<'w> {
    fn variables(&self) -> impl Iterator<Item = (&EnvVariable<'w>, Option<&String>)> {
        self.env
            .variables
            .iter()
            .zip(self.secrets.iter().map(Option::as_ref))
    }
}

pub fn secret_env<'w>(workflow: &'w Workflow, step: &AgentStep<'w>) -> SecretEnv<'w> {
    let env = agent_env(workflow, step);
    let direct = SecretEnv {
        secrets: env
            .variables
            .iter()
            .map(|variable| first_secret(&variable.value, None))
            .collect(),
        env,
    };
    let secrets = direct
        .variables()
        .map(|(variable, secret)| {
            secret
                .cloned()
                .or_else(|| first_secret(&variable.value, Some(&direct)))
        })
        .collect();
    SecretEnv {
        env: direct.env,
        secrets,
    }
}

pub fn long_lived_token<'w>(step: &AgentStep<'w>, env: &SecretEnv<'w>) -> Option<SecretUse<'w>> {
    let (setting, secret, route) = match step.agent {
        Agent::ClaudeCode => {
            let (key, value) = input(step, CLAUDE_TOKEN_INPUT)?;
            let secret = first_secret(value, Some(env))?;
            (key.to_string(), secret, input_route(step, key))
        }
        Agent::Codex | Agent::GeminiCli => env
            .variables()
            .filter(|(variable, _)| TOKEN_VARIABLES.contains(&variable.name))
            .find_map(|(variable, secret)| {
                Some((
                    format!("env {}", variable.name),
                    secret?.clone(),
                    variable.route.clone(),
                ))
            })?,
    };
    Some(SecretUse {
        setting: format!("{setting} uses {secret}, {LONG_LIVED}"),
        route,
    })
}

pub fn other_secret<'w>(step: &AgentStep<'w>, env: &SecretEnv<'w>) -> Option<SecretUse<'w>> {
    let agent_inputs: &[&str] = match step.agent {
        Agent::ClaudeCode => &CLAUDE_AGENT_INPUTS,
        Agent::Codex => &CODEX_AGENT_INPUTS,
        Agent::GeminiCli => &GEMINI_AGENT_INPUTS,
    };
    let from_input = agent_inputs.iter().find_map(|name| {
        let (key, value) = input(step, name)?;
        Some(SecretUse {
            setting: format!(
                "{key} passes {} to the agent",
                first_secret(value, Some(env))?
            ),
            route: input_route(step, key),
        })
    });
    let gemini_settings = match lookup(step, SETTINGS_INPUT) {
        Input::Text { value, .. } => value,
        Input::Missing | Input::Unreadable => "",
    };
    let from_variable = || {
        env.variables()
            .filter(|(variable, _)| reaches_agent(step.agent, variable.name, gemini_settings))
            .find_map(|(variable, secret)| {
                Some(SecretUse {
                    setting: format!("env {} passes {} to the agent", variable.name, secret?),
                    route: variable.route.clone(),
                })
            })
    };
    let from_expression = || {
        if step.agent == Agent::GeminiCli {
            return None;
        }
        let (text, route) = env.env.expression.as_ref()?;
        Some(SecretUse {
            setting: format!("env passes {} to the agent", first_secret(text, None)?),
            route: route.clone(),
        })
    };
    from_input.or_else(from_variable).or_else(from_expression)
}

fn reaches_agent(agent: Agent, name: &str, gemini_settings: &str) -> bool {
    if TOKEN_VARIABLES.contains(&name) {
        return false;
    }
    match agent {
        Agent::ClaudeCode => {
            !CLAUDE_CREDENTIAL_VARIABLES.contains(&name)
                && !CLAUDE_CREDENTIAL_PREFIXES
                    .iter()
                    .any(|prefix| name.starts_with(prefix))
        }
        Agent::Codex => !CODEX_CREDENTIAL_VARIABLES.contains(&name),
        Agent::GeminiCli => {
            !GEMINI_CREDENTIAL_VARIABLES.contains(&name)
                && references_variable(gemini_settings, name)
        }
    }
}

fn first_secret(text: &str, env: Option<&SecretEnv>) -> Option<String> {
    expression_spans(text)
        .into_iter()
        .find_map(|span| secret_in(&parse_expression(&text[span])?, env))
}

fn secret_in(expression: &SpannedExpr, env: Option<&SecretEnv>) -> Option<String> {
    expression
        .dataflow_contexts()
        .into_iter()
        .find_map(|(context, _)| {
            secret_of(context, env).or_else(|| {
                let head = context.parts.first()?;
                match head.inner {
                    Expr::Call(_) => secret_in(head, env),
                    _ => None,
                }
            })
        })
}

fn secret_of(context: &Context, env: Option<&SecretEnv>) -> Option<String> {
    let Some(Expr::Identifier(head)) = context.parts.first().map(|part| &part.inner) else {
        return None;
    };
    let whole = context.parts.len() == 1 || matches!(context.parts[1].inner, Expr::Star);
    if head.as_str().eq_ignore_ascii_case(SECRETS_CONTEXT) {
        if whole {
            return Some(EVERY_SECRET.to_string());
        }
        let name = context.single_tail()?;
        return (!name.eq_ignore_ascii_case(SHORT_LIVED_SECRET))
            .then(|| format!("{SECRETS_CONTEXT}.{name}"));
    }
    if head.as_str().eq_ignore_ascii_case(ENV_CONTEXT) {
        let mut variables = env?.variables();
        if whole {
            return variables.find_map(|(_, secret)| secret.cloned());
        }
        let name = context.single_tail()?;
        return variables
            .find(|(variable, _)| variable.name.eq_ignore_ascii_case(name))
            .and_then(|(_, secret)| secret.cloned());
    }
    None
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::agent::agent_steps;

    type Finder = for<'w> fn(&AgentStep<'w>, &SecretEnv<'w>) -> Option<SecretUse<'w>>;

    fn found(finder: Finder, source: &str) -> Option<String> {
        let workflow = Workflow::parse(Path::new("test.yml"), source.to_string()).unwrap();
        let steps = agent_steps(&workflow);
        let env = secret_env(&workflow, &steps[0]);
        finder(&steps[0], &env).map(|found| {
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
    #[test]
    fn outputs_string_literals_and_computed_names_are_not_stored_secrets() {
        let values = [
            "${{ steps.import-secrets.outputs.token }}",
            "${{ needs.fetch-secrets.outputs.token }}",
            "${{ format('Never print secrets in {0}', github.repository) }}",
            "${{ secrets[format('{0}_PAT', matrix.org)] }}",
        ];
        for value in values {
            let source = workflow(
                "",
                "anthropics/claude-code-action@v1",
                &format!(
                    "        with:\n          github_token: {value}\n          prompt: {value}\n"
                ),
            );
            assert_eq!(found(long_lived_token, &source), None, "{value}");
            assert_eq!(found(other_secret, &source), None, "{value}");
        }
    }

    #[test]
    fn env_contexts_in_inputs_carry_the_secret_into_the_settings() {
        let source = workflow(
            "env:\n  SENTRY_TOKEN: ${{ secrets.SENTRY_TOKEN }}\n",
            "google-github-actions/run-gemini-cli@v0",
            "        with:
          settings: '{\"mcpServers\": {\"sentry\": {\"env\": {\"TOKEN\": \"${{ env.SENTRY_TOKEN }}\"}}}}'
",
        );
        assert_eq!(
            found(other_secret, &source),
            Some("10: settings passes secrets.SENTRY_TOKEN to the agent".to_string())
        );
    }

    #[test]
    fn an_env_given_as_an_expression_counts_when_it_names_a_secret() {
        let source = workflow(
            "env:\n  GH_TOKEN: ${{ secrets.ORG_PAT }}\n",
            "openai/codex-action@v1",
            "        env: ${{ fromJSON(secrets.AGENT_ENV) }}\n",
        );
        assert_eq!(
            found(other_secret, &source),
            Some("9: env passes secrets.AGENT_ENV to the agent".to_string())
        );
        assert_eq!(found(long_lived_token, &source), None);
    }
    #[test]
    fn nearer_variables_survive_an_outer_env_expression() {
        let source = "on: issues
jobs:
  agent:
    runs-on: ubuntu-latest
    env: ${{ fromJSON(vars.EXTRA) }}
    steps:
      - uses: openai/codex-action@v1
        env:
          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}
          GH_TOKEN: ${{ secrets.ORG_PAT }}
";
        assert!(found(other_secret, source).is_some());
        assert!(found(long_lived_token, source).is_some());
    }

    #[test]
    fn every_token_variable_and_env_reference_is_checked() {
        let both = workflow(
            "env:\n  GH_TOKEN: ${{ secrets.ORG_PAT }}\n",
            "openai/codex-action@v1",
            "        env:\n          GITHUB_TOKEN: ${{ github.token }}\n",
        );
        assert!(found(long_lived_token, &both).is_some());
        let through_env = workflow(
            "env:\n  BOT: ${{ secrets.BOT_PAT }}\n",
            "anthropics/claude-code-action@v1",
            "        with:\n          github_token: ${{ env.bot }}\n",
        );
        assert_eq!(
            found(long_lived_token, &through_env),
            Some("10: github_token uses secrets.BOT_PAT, a stored token rather than GITHUB_TOKEN or an app token".to_string())
        );
    }

    #[test]
    fn braces_in_strings_and_object_filters_are_read() {
        let source = workflow(
            "",
            "anthropics/claude-code-action@v1",
            "        with:
          claude_args: ${{ format('--mcp-config {{\"env\":{{\"K\":\"{0}\"}}}}', secrets.SENTRY) }}
",
        );
        assert_eq!(
            found(other_secret, &source),
            Some("8: claude_args passes secrets.SENTRY to the agent".to_string())
        );
        let filter = workflow(
            "",
            "openai/codex-action@v1",
            "        env:\n          ALL: ${{ toJSON(secrets.*) }}\n",
        );
        assert_eq!(
            found(other_secret, &filter),
            Some("8: env ALL passes every secret to the agent".to_string())
        );
    }
    #[test]
    fn step_env_reading_an_outer_secret_through_env_is_traced() {
        let source = workflow(
            "env:\n  ORG_PAT: ${{ secrets.ORG_PAT }}\n",
            "openai/codex-action@v1",
            "        env:\n          GH_TOKEN: ${{ env.ORG_PAT }}\n",
        );
        assert_eq!(
            found(long_lived_token, &source),
            Some("10: env GH_TOKEN uses secrets.ORG_PAT, a stored token rather than GITHUB_TOKEN or an app token".to_string())
        );
    }

    #[test]
    fn secrets_only_compared_or_tested_do_not_flow_into_the_value() {
        let source = workflow(
            "",
            "anthropics/claude-code-action@v1",
            "        with:
          github_token: ${{ secrets.APP_ID != '' && steps.app.outputs.token || github.token }}
          prompt: ${{ contains(secrets.LIST, 'x') && 'a' || 'b' }}
",
        );
        assert_eq!(found(long_lived_token, &source), None);
        assert_eq!(found(other_secret, &source), None);
    }

    #[test]
    fn the_whole_env_in_settings_carries_its_secrets() {
        let source = workflow(
            "env:\n  SENTRY: ${{ secrets.SENTRY }}\n",
            "google-github-actions/run-gemini-cli@v0",
            "        with:\n          settings: '{\"mcpServers\": {\"s\": {\"env\": ${{ toJSON(env) }}}}}'\n",
        );
        assert_eq!(
            found(other_secret, &source),
            Some("10: settings passes secrets.SENTRY to the agent".to_string())
        );
    }

    #[test]
    fn inputs_the_action_keeps_to_itself_do_not_count() {
        let source = workflow(
            "",
            "google-github-actions/run-gemini-cli@v0",
            "        with:\n          gcp_workload_identity_provider: ${{ secrets.WIF_PROVIDER }}\n",
        );
        assert_eq!(found(other_secret, &source), None);
    }
}
