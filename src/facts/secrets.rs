use github_actions_expressions::Expr;
use github_actions_expressions::context::Context;
use yamlpath::{Component, Route};

use super::checkout::CHECKOUT_ACTION;
use super::{
    ENV_CONTEXT, EnvVariable, Input, NearestEnv, SETTINGS_INPUT, agent_env, expression_spans,
    flag_in, flowing_contexts, input, input_route, lookup, lookup_in, parse_expression,
    references_variable,
};
use crate::agent::{Agent, AgentStep, JobStep, is_root_action, job_steps};
use crate::workflow::Workflow;

const SECRETS_CONTEXT: &str = "secrets";
const SHORT_LIVED_SECRET: &str = "GITHUB_TOKEN";
pub(super) const EVERY_SECRET: &str = "every secret";
const CLAUDE_TOKEN_INPUT: &str = "github_token";
const TOKEN_VARIABLES: [&str; 2] = ["GITHUB_TOKEN", "GH_TOKEN"];
const CLAUDE_AGENT_INPUTS: [&str; 3] = ["prompt", "settings", "claude_args"];
const CLAUDE_BASE_INPUTS: [&str; 3] = ["prompt", "settings", "claude_args"];
const CLAUDE_BASE_BETA_INPUTS: [&str; 5] = [
    "prompt",
    "settings",
    "mcp_config",
    "system_prompt",
    "append_system_prompt",
];
const CLAUDE_INPUTS_WITH_TOKEN: [&str; 4] = ["prompt", "settings", "claude_args", "github_token"];
const CODEX_AGENT_INPUTS: [&str; 2] = ["prompt", "codex-args"];
const GEMINI_AGENT_INPUTS: [&str; 2] = ["prompt", "settings"];
const CLAUDE_CREDENTIAL_VARIABLES: [&str; 4] = [
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "ANTHROPIC_CUSTOM_HEADERS",
];
const CLAUDE_CREDENTIAL_INPUT_VARIABLES: [&str; 2] =
    ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"];
const CLAUDE_CREDENTIAL_PREFIXES: [&str; 2] = ["AWS_", "ANTHROPIC_FOUNDRY_"];
const CODEX_CREDENTIAL_VARIABLES: [&str; 2] = ["OPENAI_API_KEY", "AWS_BEARER_TOKEN_BEDROCK"];
const CODEX_SAFETY_STRATEGY: &str = "safety-strategy";
const CODEX_UNPRIVILEGED_USER: &str = "unprivileged-user";
const CODEX_USER_INPUT: &str = "codex-user";
const RUNNER_USER: &str = "runner";
const GEMINI_CREDENTIAL_VARIABLES: [&str; 2] = ["GEMINI_API_KEY", "GOOGLE_API_KEY"];
const CLAUDE_CREDENTIAL_INPUTS: [&str; 2] = ["anthropic_api_key", "claude_code_oauth_token"];
const GEMINI_CREDENTIAL_INPUTS: [&str; 2] = ["gemini_api_key", "google_api_key"];
const LONG_LIVED: &str = "a stored token rather than GITHUB_TOKEN or an app token";
const CHECKOUT_CREDENTIAL_INPUTS: [&str; 2] = ["token", "ssh-key"];
const PERSIST_CREDENTIALS_INPUT: &str = "persist-credentials";

#[derive(Debug)]
pub struct SecretUse<'w> {
    pub setting: String,
    pub route: Route<'w>,
}

pub struct SecretEnv<'w> {
    env: NearestEnv<'w>,
    secrets: Vec<Vec<String>>,
    inherited: bool,
}

impl<'w> SecretEnv<'w> {
    fn variables(&self) -> impl Iterator<Item = (&EnvVariable<'w>, &[String])> {
        self.env
            .variables
            .iter()
            .zip(self.secrets.iter().map(Vec::as_slice))
    }

    fn inherited_variables(&self) -> impl Iterator<Item = (&EnvVariable<'w>, &[String])> {
        self.inherited
            .then(|| self.variables())
            .into_iter()
            .flatten()
    }

    fn inherited_expression(&self) -> Option<&(&'w str, Route<'w>)> {
        self.env.expression.as_ref().filter(|_| self.inherited)
    }
}

pub fn secret_env<'w>(workflow: &'w Workflow, step: &AgentStep<'w>) -> SecretEnv<'w> {
    let runs_as_unprivileged_user = step.agent == Agent::Codex
        && input(step, CODEX_SAFETY_STRATEGY)
            .is_some_and(|(_, value)| value.trim() == CODEX_UNPRIVILEGED_USER)
        && input(step, CODEX_USER_INPUT)
            .is_some_and(|(_, user)| !user.trim().is_empty() && user.trim() != RUNNER_USER);
    resolved_secrets(agent_env(workflow, step), !runs_as_unprivileged_user)
}

pub(super) fn shell_secrets<'w>(env: NearestEnv<'w>, code: &str) -> Vec<String> {
    let env = resolved_secrets(env, true);
    let mut secrets: Vec<String> = env
        .variables()
        .flat_map(|(_, secrets)| secrets.iter().cloned())
        .collect();
    if let Some((text, _)) = &env.env.expression {
        secrets.extend(all_secrets(text, None));
    }
    secrets.extend(all_secrets(code, Some(&env)));
    secrets
}

pub fn agent_secrets<'w>(step: &AgentStep<'w>, env: &SecretEnv<'w>) -> Vec<String> {
    let agent_inputs: &[&str] = match step.agent {
        Agent::ClaudeCode => &CLAUDE_INPUTS_WITH_TOKEN,
        Agent::ClaudeBase => &CLAUDE_BASE_INPUTS,
        Agent::ClaudeBaseBeta => &CLAUDE_BASE_BETA_INPUTS,
        Agent::Codex => &CODEX_AGENT_INPUTS,
        Agent::GeminiCli => &GEMINI_AGENT_INPUTS,
    };
    let gemini_settings = match lookup(step, SETTINGS_INPUT) {
        Input::Text { value, .. } => value,
        Input::Missing | Input::Unreadable => "",
    };
    let mut secrets: Vec<String> = agent_inputs
        .iter()
        .filter_map(|name| input(step, name))
        .flat_map(|(_, value)| all_secrets(value, Some(env)))
        .collect();
    secrets.extend(
        env.inherited_variables()
            .filter(|(variable, _)| match step.agent {
                Agent::Codex => true,
                Agent::ClaudeCode => reaches_agent(step.agent, variable.name, gemini_settings),
                Agent::ClaudeBase | Agent::ClaudeBaseBeta | Agent::GeminiCli => {
                    reaches_agent(step.agent, variable.name, gemini_settings)
                        || TOKEN_VARIABLES.contains(&variable.name)
                }
            })
            .flat_map(|(_, secrets)| secrets.iter().cloned()),
    );
    if step.agent != Agent::GeminiCli
        && let Some((text, _)) = env.inherited_expression()
    {
        secrets.extend(all_secrets(text, None));
    }
    let credentials = credential_secrets(step, env);
    secrets.retain(|secret| {
        !credentials
            .iter()
            .any(|credential| credential.eq_ignore_ascii_case(secret))
    });
    secrets
}

fn resolved_secrets(env: NearestEnv<'_>, inherited: bool) -> SecretEnv<'_> {
    let direct = SecretEnv {
        secrets: env
            .variables
            .iter()
            .map(|variable| all_secrets(&variable.value, None))
            .collect(),
        env,
        inherited,
    };
    let secrets = direct
        .variables()
        .map(|(variable, _)| all_secrets(&variable.value, Some(&direct)))
        .collect();
    SecretEnv {
        env: direct.env,
        secrets,
        inherited,
    }
}

pub fn long_lived_token<'w>(step: &AgentStep<'w>, env: &SecretEnv<'w>) -> Option<SecretUse<'w>> {
    let handed = match step.agent {
        Agent::ClaudeCode => input(step, CLAUDE_TOKEN_INPUT).and_then(|(key, value)| {
            let secret = all_secrets(value, Some(env)).into_iter().next()?;
            Some((key.to_string(), secret, input_route(step, key)))
        }),
        Agent::ClaudeBase | Agent::ClaudeBaseBeta | Agent::Codex | Agent::GeminiCli => env
            .inherited_variables()
            .filter(|(variable, _)| TOKEN_VARIABLES.contains(&variable.name))
            .find_map(|(variable, secrets)| {
                Some((
                    format!("env {}", variable.name),
                    secrets.first()?.clone(),
                    variable.route.clone(),
                ))
            }),
    };
    if let Some((setting, secret, route)) = handed {
        return Some(SecretUse {
            setting: format!("{setting} uses {secret}, {LONG_LIVED}"),
            route,
        });
    }
    job_steps(step.job_id, step.job)
        .into_iter()
        .take_while(|(index, ..)| *index < step.index)
        .find_map(|(_, route, earlier)| {
            let JobStep::Uses(checkout) = earlier else {
                return None;
            };
            if !is_root_action(&checkout.uses, CHECKOUT_ACTION)
                || flag_in(&checkout.with, PERSIST_CREDENTIALS_INPUT) == Some(false)
            {
                return None;
            }
            CHECKOUT_CREDENTIAL_INPUTS.iter().find_map(|name| {
                let Input::Text { key, value } = lookup_in(&checkout.with, name) else {
                    return None;
                };
                let secret = all_secrets(value, None).into_iter().next()?;
                Some(SecretUse {
                    setting: format!(
                        "{} keeps {key} {secret} in the git config, {LONG_LIVED}",
                        checkout.uses.raw()
                    ),
                    route: route.with_keys([Component::from("with"), Component::from(key)]),
                })
            })
        })
}

pub fn other_secret<'w>(step: &AgentStep<'w>, env: &SecretEnv<'w>) -> Option<SecretUse<'w>> {
    let agent_inputs: &[&str] = match step.agent {
        Agent::ClaudeCode => &CLAUDE_AGENT_INPUTS,
        Agent::ClaudeBase => &CLAUDE_BASE_INPUTS,
        Agent::ClaudeBaseBeta => &CLAUDE_BASE_BETA_INPUTS,
        Agent::Codex => &CODEX_AGENT_INPUTS,
        Agent::GeminiCli => &GEMINI_AGENT_INPUTS,
    };
    let credentials = credential_secrets(step, env);
    let is_other = |secret: &String| {
        !credentials
            .iter()
            .any(|credential| credential.eq_ignore_ascii_case(secret))
    };
    let from_input = agent_inputs.iter().find_map(|name| {
        let (key, value) = input(step, name)?;
        let secret = all_secrets(value, Some(env)).into_iter().find(is_other)?;
        Some(SecretUse {
            setting: format!("{key} passes {secret} to the agent"),
            route: input_route(step, key),
        })
    });
    let gemini_settings = match lookup(step, SETTINGS_INPUT) {
        Input::Text { value, .. } => value,
        Input::Missing | Input::Unreadable => "",
    };
    let from_variable = || {
        env.inherited_variables()
            .filter(|(variable, _)| reaches_agent(step.agent, variable.name, gemini_settings))
            .find_map(|(variable, secrets)| {
                let secret = secrets.iter().find(|secret| is_other(secret))?.clone();
                Some(SecretUse {
                    setting: format!("env {} passes {secret} to the agent", variable.name),
                    route: variable.route.clone(),
                })
            })
    };
    let from_expression = || {
        if step.agent == Agent::GeminiCli {
            return None;
        }
        let (text, route) = env.inherited_expression()?;
        let secret = all_secrets(text, None).into_iter().find(is_other)?;
        Some(SecretUse {
            setting: format!("env passes {secret} to the agent"),
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
        Agent::ClaudeCode | Agent::ClaudeBase | Agent::ClaudeBaseBeta => {
            !is_claude_credential(name)
        }
        Agent::Codex => !CODEX_CREDENTIAL_VARIABLES.contains(&name),
        Agent::GeminiCli => {
            !GEMINI_CREDENTIAL_VARIABLES.contains(&name)
                && references_variable(gemini_settings, name)
        }
    }
}

fn credential_secrets(step: &AgentStep, env: &SecretEnv) -> Vec<String> {
    let (credential_inputs, is_credential_variable): (&[&str], fn(&str) -> bool) = match step.agent
    {
        Agent::ClaudeCode => (&CLAUDE_CREDENTIAL_INPUTS, is_claude_credential),
        Agent::ClaudeBase | Agent::ClaudeBaseBeta => (&CLAUDE_CREDENTIAL_INPUTS, |name| {
            is_claude_credential(name) && !CLAUDE_CREDENTIAL_INPUT_VARIABLES.contains(&name)
        }),
        Agent::GeminiCli => (&GEMINI_CREDENTIAL_INPUTS, |_| false),
        Agent::Codex => return Vec::new(),
    };
    let from_inputs = credential_inputs
        .iter()
        .filter_map(|name| input(step, name))
        .flat_map(|(_, value)| all_secrets(value, Some(env)));
    let from_variables = env
        .inherited_variables()
        .filter(|(variable, _)| is_credential_variable(variable.name))
        .flat_map(|(_, secrets)| secrets.iter().cloned());
    from_inputs
        .chain(from_variables)
        .filter(|secret| secret != EVERY_SECRET)
        .collect()
}

fn is_claude_credential(name: &str) -> bool {
    CLAUDE_CREDENTIAL_VARIABLES.contains(&name)
        || CLAUDE_CREDENTIAL_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

fn all_secrets(text: &str, env: Option<&SecretEnv>) -> Vec<String> {
    expression_spans(text)
        .into_iter()
        .filter_map(|span| parse_expression(&text[span]))
        .flat_map(|expression| {
            flowing_contexts(&expression)
                .into_iter()
                .flat_map(|context| secrets_of(context, env))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn secrets_of(context: &Context, env: Option<&SecretEnv>) -> Vec<String> {
    let Some(Expr::Identifier(head)) = context.parts.first().map(|part| &part.inner) else {
        return Vec::new();
    };
    let whole = context.parts.len() == 1 || matches!(context.parts[1].inner, Expr::Star);
    if head.as_str().eq_ignore_ascii_case(SECRETS_CONTEXT) {
        if whole {
            return vec![EVERY_SECRET.to_string()];
        }
        return context
            .single_tail()
            .filter(|name| !name.eq_ignore_ascii_case(SHORT_LIVED_SECRET))
            .map(|name| format!("{SECRETS_CONTEXT}.{name}"))
            .into_iter()
            .collect();
    }
    if head.as_str().eq_ignore_ascii_case(ENV_CONTEXT) {
        let Some(env) = env else {
            return Vec::new();
        };
        if whole {
            return env
                .variables()
                .flat_map(|(_, secrets)| secrets.iter().cloned())
                .collect();
        }
        let Some(name) = context.single_tail() else {
            return Vec::new();
        };
        return env
            .variables()
            .find(|(variable, _)| variable.name.eq_ignore_ascii_case(name))
            .map(|(_, secrets)| secrets.to_vec())
            .unwrap_or_default();
    }
    Vec::new()
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
    fn a_stored_token_kept_by_an_earlier_checkout_is_long_lived() {
        let source = |checkout: &str, agent: &str| {
            format!(
                "on: issues
jobs:
  agent:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
        with:
{checkout}
      - uses: {agent}
"
            )
        };
        assert_eq!(
            found(
                long_lived_token,
                &source("          token: ${{ secrets.BOT_PAT }}", "anthropics/claude-code-action@v1")
            ),
            Some("8: actions/checkout@v6 keeps token secrets.BOT_PAT in the git config, a stored token rather than GITHUB_TOKEN or an app token".to_string())
        );
        assert!(
            found(
                long_lived_token,
                &source(
                    "          ssh-key: ${{ secrets.DEPLOY_KEY }}",
                    "google-github-actions/run-gemini-cli@v0"
                )
            )
            .is_some()
        );
        assert!(
            found(
                long_lived_token,
                &source(
                    "          token: ${{ secrets.BOT_PAT }}\n          path: other",
                    "openai/codex-action@v1\n        with:\n          safety-strategy: unprivileged-user\n          codex-user: codex"
                )
            )
            .is_some()
        );
        for checkout in [
            "          token: ${{ secrets.BOT_PAT }}\n          persist-credentials: false",
            "          token: ${{ secrets.GITHUB_TOKEN }}",
            "          token: ${{ steps.app.outputs.token }}",
            "          fetch-depth: 0",
        ] {
            assert_eq!(
                found(
                    long_lived_token,
                    &source(checkout, "anthropics/claude-code-action@v1")
                ),
                None,
                "{checkout}"
            );
        }
        let after = "on: issues
jobs:
  agent:
    runs-on: ubuntu-latest
    steps:
      - uses: anthropics/claude-code-action@v1
      - uses: actions/checkout@v6
        with:
          token: ${{ secrets.BOT_PAT }}
";
        assert_eq!(found(long_lived_token, after), None);
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
    fn codex_running_as_another_user_gets_no_env() {
        let source = |with: &str| {
            format!(
                "on: push\njobs:\n  a:\n    runs-on: ubuntu-latest\n    env:\n      GH_TOKEN: ${{{{ secrets.BOT_PAT }}}}\n      NPM_TOKEN: ${{{{ secrets.NPM }}}}\n    steps:\n      - uses: openai/codex-action@v1\n        with:\n          prompt: ${{{{ secrets.PROMPT }}}}\n{with}"
            )
        };
        let separate =
            source("          safety-strategy: unprivileged-user\n          codex-user: codex\n");
        assert_eq!(
            found(other_secret, &separate),
            Some("11: prompt passes secrets.PROMPT to the agent".to_string())
        );
        assert_eq!(found(long_lived_token, &separate), None);
        let workflow = Workflow::parse(Path::new("test.yml"), separate).unwrap();
        let steps = agent_steps(&workflow);
        assert_eq!(
            agent_secrets(&steps[0], &secret_env(&workflow, &steps[0])),
            ["secrets.PROMPT"]
        );
        assert!(found(long_lived_token, &source("")).is_some());
        assert!(
            found(
                long_lived_token,
                &source(
                    "          safety-strategy: unprivileged-user\n          codex-user: runner\n"
                )
            )
            .is_some()
        );
        let through_env =
            source("          safety-strategy: unprivileged-user\n          codex-user: codex\n")
                .replace(
                    "prompt: ${{ secrets.PROMPT }}",
                    "prompt: use ${{ env.NPM_TOKEN }}",
                );
        assert_eq!(
            found(other_secret, &through_env),
            Some("11: prompt passes secrets.NPM to the agent".to_string())
        );
    }

    #[test]
    fn codex_holds_every_variable_of_its_env() {
        let held = |with: &str| {
            let source = format!(
                "on: push\njobs:\n  a:\n    runs-on: ubuntu-latest\n    env:\n      AWS_BEARER_TOKEN_BEDROCK: ${{{{ secrets.BEDROCK }}}}\n      NPM_TOKEN: ${{{{ secrets.NPM }}}}\n    steps:\n      - uses: openai/codex-action@v1\n{with}"
            );
            let workflow = Workflow::parse(Path::new("test.yml"), source).unwrap();
            let steps = agent_steps(&workflow);
            agent_secrets(&steps[0], &secret_env(&workflow, &steps[0]))
        };
        assert_eq!(held(""), ["secrets.BEDROCK", "secrets.NPM"]);
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
            "env:\n  OPENAI_API_KEY: ${{ secrets.OPENAI_API_KEY }}\n  AWS_BEARER_TOKEN_BEDROCK: ${{ secrets.BEDROCK }}\n  ALL: ${{ toJSON(secrets) }}\n",
            "openai/codex-action@v1",
            "",
        );
        assert_eq!(
            found(other_secret, &codex),
            Some("5: env ALL passes every secret to the agent".to_string())
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
    fn the_agents_own_credential_in_another_input_or_env_is_not_another_secret() {
        let gemini = workflow(
            "env:\n  IMAGE_KEY: ${{ secrets.gemini_api_key }}\n",
            "google-github-actions/run-gemini-cli@v0",
            "        with:
          gemini_api_key: ${{ secrets.GEMINI_API_KEY }}
          settings: |-
            {\"mcpServers\": {\"image\": {\"env\": {\"GEMINI_API_KEY\": \"${{ secrets.GEMINI_API_KEY }}\", \"KEY\": \"$IMAGE_KEY\"}}}}
",
        );
        assert_eq!(found(other_secret, &gemini), None);
        let claude = workflow(
            "env:\n  KEY: ${{ secrets.ANTHROPIC }}\n  SENTRY: ${{ secrets.SENTRY }}\n",
            "anthropics/claude-code-action@v1",
            "        with:
          anthropic_api_key: ${{ secrets.FALLBACK || env.KEY }}
          prompt: ${{ secrets.FALLBACK || secrets.SENTRY }}
",
        );
        assert_eq!(
            found(other_secret, &claude),
            Some("12: prompt passes secrets.SENTRY to the agent".to_string())
        );
    }

    #[test]
    fn another_secret_next_to_the_credential_in_an_env_variable_or_the_whole_env_counts() {
        let variable = workflow(
            "env:\n  KEY: ${{ secrets.GEMINI_API_KEY || secrets.SENTRY }}\n",
            "google-github-actions/run-gemini-cli@v0",
            "        with:
          gemini_api_key: ${{ secrets.GEMINI_API_KEY }}
          settings: '{\"mcpServers\": {\"s\": {\"env\": {\"KEY\": \"$KEY\"}}}}'
",
        );
        assert_eq!(
            found(other_secret, &variable),
            Some("3: env KEY passes secrets.SENTRY to the agent".to_string())
        );
        let whole = workflow(
            "env:\n  A: ${{ secrets.GEMINI_API_KEY }}\n  B: ${{ secrets.SENTRY }}\n",
            "google-github-actions/run-gemini-cli@v0",
            "        with:
          gemini_api_key: ${{ secrets.GEMINI_API_KEY }}
          settings: '{\"mcpServers\": {\"s\": {\"env\": ${{ toJSON(env) }}}}}'
",
        );
        assert_eq!(
            found(other_secret, &whole),
            Some("12: settings passes secrets.SENTRY to the agent".to_string())
        );
    }

    #[test]
    fn another_secret_behind_the_credential_in_a_referenced_env_variable_counts() {
        let source = workflow(
            "env:\n  KEY: ${{ secrets.GEMINI_API_KEY || secrets.SENTRY }}\n",
            "google-github-actions/run-gemini-cli@v0",
            "        with:
          gemini_api_key: ${{ secrets.GEMINI_API_KEY }}
          prompt: ${{ env.KEY }}
",
        );
        assert_eq!(
            found(other_secret, &source),
            Some("11: prompt passes secrets.SENTRY to the agent".to_string())
        );
    }

    #[test]
    fn gemini_credentials_come_only_from_its_inputs_because_the_action_overrides_the_env() {
        let source = workflow(
            "env:\n  GEMINI_API_KEY: ${{ secrets.PROD_KEY }}\n",
            "google-github-actions/run-gemini-cli@v0",
            "        with:
          settings: '{\"mcpServers\": {\"s\": {\"env\": {\"K\": \"${{ secrets.PROD_KEY }}\"}}}}'
",
        );
        assert_eq!(
            found(other_secret, &source),
            Some("10: settings passes secrets.PROD_KEY to the agent".to_string())
        );
    }

    #[test]
    fn a_credential_given_through_its_env_name_is_the_agents_own() {
        let settings = "        with:
          settings: '{\"env\": {\"KEY\": \"${{ secrets.CLAUDE_KEY }}\"}}'
";
        let env = "env:\n  ANTHROPIC_API_KEY: ${{ secrets.CLAUDE_KEY }}\n";
        let action = workflow(env, "anthropics/claude-code-action@v1", settings);
        assert_eq!(found(other_secret, &action), None);
        let base = workflow(
            env,
            "anthropics/claude-code-action/base-action@v1",
            settings,
        );
        assert_eq!(
            found(other_secret, &base),
            Some("10: settings passes secrets.CLAUDE_KEY to the agent".to_string())
        );
    }

    #[test]
    fn codex_sees_its_api_key_only_when_it_is_written_outside_the_credential_input() {
        let source = workflow(
            "env:\n  MY_KEY: ${{ secrets.OPENAI_API_KEY }}\n",
            "openai/codex-action@v1",
            "        with:
          openai-api-key: ${{ secrets.OPENAI_API_KEY }}
",
        );
        assert_eq!(
            found(other_secret, &source),
            Some("3: env MY_KEY passes secrets.OPENAI_API_KEY to the agent".to_string())
        );
    }

    #[test]
    fn a_secret_only_named_like_a_credential_still_counts_outside_the_credential_input() {
        let source = workflow(
            "",
            "google-github-actions/run-gemini-cli@v0",
            "        with:
          gemini_api_key: ${{ secrets.AGENT_KEY }}
          settings: '{\"mcpServers\": {\"image\": {\"env\": {\"GEMINI_API_KEY\": \"${{ secrets.GEMINI_API_KEY }}\"}}}}'
",
        );
        assert_eq!(
            found(other_secret, &source),
            Some("9: settings passes secrets.GEMINI_API_KEY to the agent".to_string())
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
