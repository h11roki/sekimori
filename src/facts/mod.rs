mod checkout;
mod tools;
mod trigger;

use github_actions_models::common::expr::LoE;
use github_actions_models::common::{Env, EnvValue};
use yamlpath::{Component, Route};

use crate::agent::AgentStep;

pub use checkout::{HeadCheckout, head_checkout};
pub use tools::{BroadTool, broad_tool};
pub use trigger::{ExternalEvent, OpenTrigger, external_events, never_runs, open_trigger};

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
