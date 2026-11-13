mod checkout;
mod secrets;
mod tools;
mod trigger;

use github_actions_models::common::expr::LoE;
use github_actions_models::common::{Env, EnvValue};
use std::ops::Range;

use yamlpath::{Component, Route};

use crate::agent::AgentStep;

pub use checkout::{HeadCheckout, head_checkout};
pub use secrets::{SecretUse, long_lived_token, other_secret};
pub use tools::{BroadTool, broad_tool};
pub use trigger::{ExternalEvent, OpenTrigger, external_events, never_runs, open_trigger};

const EXPRESSION_START: &str = "${{";
const EXPRESSION_END: &str = "}}";

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
    let mut spans = Vec::new();
    let mut offset = 0;
    while let Some(start) = text[offset..].find(EXPRESSION_START) {
        let start = offset + start;
        let end = text[start..]
            .find(EXPRESSION_END)
            .map_or(text.len(), |end| start + end + EXPRESSION_END.len());
        spans.push(start..end);
        offset = end;
    }
    spans
}

fn expression_bodies(text: &str) -> impl Iterator<Item = &str> {
    expression_spans(text).into_iter().map(|span| {
        let expression = &text[span];
        let body = &expression[EXPRESSION_START.len()..];
        body.strip_suffix(EXPRESSION_END).unwrap_or(body)
    })
}
