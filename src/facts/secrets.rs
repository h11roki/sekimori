use github_actions_expressions::call::Call;
use github_actions_expressions::context::Context;
use github_actions_expressions::op::BinExpr;
use github_actions_expressions::{Expr, SpannedExpr};
use github_actions_models::common::expr::LoE;
use yamlpath::Route;

use super::{
    ENV_CONTEXT, EXPRESSION_END, EXPRESSION_START, Input, SETTINGS_INPUT, agent_env,
    expression_spans, input, input_route, lookup, references_variable,
};
use crate::agent::{Agent, AgentStep};
use crate::workflow::Workflow;

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
const LONG_LIVED: &str = "a stored token rather than GITHUB_TOKEN or an app token";

#[derive(Debug)]
pub struct SecretUse<'w> {
    pub setting: String,
    pub route: Route<'w>,
}

pub struct SecretEnv<'w> {
    variables: Vec<SecretVariable<'w>>,
    expression: Option<(&'w str, Route<'w>)>,
}

struct SecretVariable<'w> {
    name: &'w str,
    secret: Option<String>,
    route: Route<'w>,
}

pub fn secret_env<'w>(workflow: &'w Workflow, step: &AgentStep<'w>) -> SecretEnv<'w> {
    let env = agent_env(workflow, step);
    SecretEnv {
        variables: env
            .variables
            .into_iter()
            .map(|variable| SecretVariable {
                name: variable.name,
                secret: first_secret(&variable.value, &[]),
                route: variable.route,
            })
            .collect(),
        expression: env.expression,
    }
}

pub fn long_lived_token<'w>(step: &AgentStep<'w>, env: &SecretEnv<'w>) -> Option<SecretUse<'w>> {
    let (setting, secret, route) = match step.agent {
        Agent::ClaudeCode => {
            let (key, value) = input(step, CLAUDE_TOKEN_INPUT)?;
            let secret = first_secret(value, &env.variables)?;
            (key.to_string(), secret, input_route(step, key))
        }
        Agent::Codex | Agent::GeminiCli => env
            .variables
            .iter()
            .filter(|variable| TOKEN_VARIABLES.contains(&variable.name))
            .find_map(|variable| {
                Some((
                    format!("env {}", variable.name),
                    variable.secret.clone()?,
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
    let credential_inputs: &[&str] = match step.agent {
        Agent::ClaudeCode => &CLAUDE_CREDENTIAL_INPUTS,
        Agent::Codex => &CODEX_CREDENTIAL_INPUTS,
        Agent::GeminiCli => &GEMINI_CREDENTIAL_INPUTS,
    };
    let from_input = match &step.step.with {
        LoE::Literal(with) => with
            .iter()
            .filter(|(key, _)| {
                !credential_inputs
                    .iter()
                    .any(|credential| credential.eq_ignore_ascii_case(key))
            })
            .find_map(|(key, value)| {
                Some(SecretUse {
                    setting: format!(
                        "{key} passes {} to the agent",
                        first_secret(&value.to_string(), &env.variables)?
                    ),
                    route: input_route(step, key),
                })
            }),
        LoE::Expr(_) => None,
    };
    let gemini_settings = match lookup(step, SETTINGS_INPUT) {
        Input::Text { value, .. } => value,
        Input::Missing | Input::Unreadable => "",
    };
    let from_variable = || {
        env.variables
            .iter()
            .filter(|variable| reaches_agent(step.agent, variable.name, gemini_settings))
            .find_map(|variable| {
                Some(SecretUse {
                    setting: format!(
                        "env {} passes {} to the agent",
                        variable.name,
                        variable.secret.as_ref()?
                    ),
                    route: variable.route.clone(),
                })
            })
    };
    let from_expression = || {
        let (text, route) = env.expression.as_ref()?;
        (step.agent != Agent::GeminiCli).then_some(())?;
        Some(SecretUse {
            setting: format!("env passes {} to the agent", first_secret(text, &[])?),
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

fn first_secret(text: &str, variables: &[SecretVariable]) -> Option<String> {
    expression_spans(text).into_iter().find_map(|span| {
        let body = text[span].strip_prefix(EXPRESSION_START)?;
        let body = body.strip_suffix(EXPRESSION_END).unwrap_or(body);
        secret_in(&Expr::parse(body.trim()).ok()?, variables)
    })
}

fn secret_in(expression: &SpannedExpr, variables: &[SecretVariable]) -> Option<String> {
    match &expression.inner {
        Expr::Context(context) => secret_of(context, variables).or_else(|| {
            context
                .parts
                .iter()
                .filter(|part| !matches!(part.inner, Expr::Identifier(_)))
                .find_map(|part| secret_in(part, variables))
        }),
        Expr::Index(index) => secret_in(index, variables),
        Expr::Call(Call { args, .. }) => args
            .iter()
            .find_map(|argument| secret_in(argument, variables)),
        Expr::BinExpr(BinExpr { lhs, rhs, .. }) => {
            secret_in(lhs, variables).or_else(|| secret_in(rhs, variables))
        }
        Expr::UnExpr { expr, .. } => secret_in(expr, variables),
        Expr::Literal(_) | Expr::Star | Expr::Identifier(_) => None,
    }
}

fn secret_of(context: &Context, variables: &[SecretVariable]) -> Option<String> {
    let Some(Expr::Identifier(head)) = context.parts.first().map(|part| &part.inner) else {
        return None;
    };
    if head.as_str().eq_ignore_ascii_case(SECRETS_CONTEXT) {
        if context.parts.len() == 1 || matches!(context.parts[1].inner, Expr::Star) {
            return Some(EVERY_SECRET.to_string());
        }
        let name = context.single_tail()?;
        return (!name.eq_ignore_ascii_case(SHORT_LIVED_SECRET))
            .then(|| format!("{SECRETS_CONTEXT}.{name}"));
    }
    if head.as_str().eq_ignore_ascii_case(ENV_CONTEXT) {
        let name = context.single_tail()?;
        return variables
            .iter()
            .find(|variable| variable.name.eq_ignore_ascii_case(name))
            .and_then(|variable| variable.secret.clone());
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
}
