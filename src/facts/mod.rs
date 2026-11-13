mod checkout;
mod secrets;
mod tools;
mod trigger;

use std::ops::Range;

use github_actions_models::common::expr::LoE;
use github_actions_models::common::{Env, EnvValue};
use github_actions_models::workflow::job::NormalJob;
use yamlpath::{Component, Route, route};

use crate::agent::AgentStep;
use crate::workflow::Workflow;

pub use checkout::{HeadCheckout, head_checkout};
pub use secrets::{SecretEnv, SecretUse, long_lived_token, other_secret, secret_env};
pub use tools::{BroadTool, broad_tool};
pub use trigger::{ExternalEvent, OpenTrigger, external_events, never_runs, open_trigger};

const EXPRESSION_START: &str = "${{";
const EXPRESSION_END: &str = "}}";
const ENV_CONTEXT: &str = "env";
const SETTINGS_INPUT: &str = "settings";

enum Input<'w> {
    Missing,
    Text { key: &'w str, value: &'w str },
    Unreadable,
}

fn lookup<'w>(step: &AgentStep<'w>, name: &str) -> Input<'w> {
    lookup_in(&step.step.with, name)
}

fn lookup_in<'w>(with: &'w LoE<Env>, name: &str) -> Input<'w> {
    let LoE::Literal(with) = with else {
        return Input::Unreadable;
    };
    match with.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)) {
        None => Input::Missing,
        Some((key, EnvValue::String(value))) => Input::Text {
            key: key.as_str(),
            value: value.as_str(),
        },
        Some((_, EnvValue::Number(_) | EnvValue::Boolean(_))) => Input::Unreadable,
    }
}

fn input<'w>(step: &AgentStep<'w>, name: &str) -> Option<(&'w str, &'w str)> {
    match lookup(step, name) {
        Input::Text { key, value } => Some((key, value)),
        Input::Missing | Input::Unreadable => None,
    }
}

fn input_route<'w>(step: &AgentStep<'w>, key: &'w str) -> Route<'w> {
    step.route
        .with_keys([Component::from("with"), Component::from(key)])
}

fn before_comment(text: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    let mut previous = ' ';
    for (index, c) in text.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' && quote != Some('\'') {
            escaped = true;
        } else {
            match quote {
                Some(open) if c == open => quote = None,
                Some(_) => {}
                None if c == '\'' || c == '"' => quote = Some(c),
                None if c == '#' && previous.is_whitespace() => return &text[..index],
                None => {}
            }
        }
        previous = c;
    }
    text
}

fn program_name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

fn ends_word(text: &str, reference: &str) -> bool {
    text.match_indices(reference).any(|(start, _)| {
        !text[start + reference.len()..]
            .starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
    })
}

fn references_variable(text: &str, name: &str) -> bool {
    [format!("${name}"), format!("${{{name}}}")]
        .iter()
        .any(|reference| ends_word(text, reference))
}

fn expression_spans(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut offset = 0;
    while let Some(found) = text[offset..].find(EXPRESSION_START) {
        let start = offset + found;
        let mut in_string = false;
        let mut end = text.len();
        for index in start + EXPRESSION_START.len()..bytes.len() {
            match bytes[index] {
                b'\'' => in_string = !in_string,
                b'}' if !in_string && text[index..].starts_with(EXPRESSION_END) => {
                    end = index + EXPRESSION_END.len();
                    break;
                }
                _ => {}
            }
        }
        spans.push(start..end);
        offset = end;
    }
    spans
}

struct EnvVariable<'w> {
    name: &'w str,
    value: String,
    route: Route<'w>,
}

struct NearestEnv<'w> {
    variables: Vec<EnvVariable<'w>>,
    expression: Option<(&'w str, Route<'w>)>,
}

fn nearest_env<'w>(
    workflow: &'w Workflow,
    job_id: &'w str,
    job: &'w NormalJob,
    step_env: &'w LoE<Env>,
    step_route: &Route<'w>,
) -> NearestEnv<'w> {
    let scopes = [
        (step_env, step_route.with_key(ENV_CONTEXT)),
        (&job.env, route!["jobs", job_id, ENV_CONTEXT]),
        (&workflow.model.env, route![ENV_CONTEXT]),
    ];
    let mut variables: Vec<EnvVariable<'w>> = Vec::new();
    for (env, scope) in scopes {
        let env = match env {
            LoE::Literal(env) => env,
            LoE::Expr(expression) => {
                return NearestEnv {
                    variables,
                    expression: Some((expression.as_raw(), scope)),
                };
            }
        };
        for (name, value) in env {
            if variables.iter().any(|variable| variable.name == name) {
                continue;
            }
            variables.push(EnvVariable {
                name,
                value: value.to_string(),
                route: scope.with_key(name.as_str()),
            });
        }
    }
    NearestEnv {
        variables,
        expression: None,
    }
}

fn agent_env<'w>(workflow: &'w Workflow, step: &AgentStep<'w>) -> NearestEnv<'w> {
    nearest_env(
        workflow,
        step.job_id,
        step.job,
        &step.step.shared.env,
        &step.route,
    )
}
