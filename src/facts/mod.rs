mod tools;
mod trigger;

use github_actions_models::common::EnvValue;
use github_actions_models::common::expr::LoE;
use yamlpath::{Component, Route};

use crate::agent::AgentStep;

pub use tools::{BroadTool, broad_tool};
pub use trigger::{ExternalEvent, OpenTrigger, external_events, never_runs, open_trigger};

fn input<'w>(step: &AgentStep<'w>, name: &str) -> Option<(&'w str, &'w str)> {
    let LoE::Literal(with) = &step.step.with else {
        return None;
    };
    with.iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .and_then(|(key, value)| match value {
            EnvValue::String(value) => Some((key.as_str(), value.as_str())),
            EnvValue::Number(_) | EnvValue::Boolean(_) => None,
        })
}

fn input_route<'w>(step: &AgentStep<'w>, key: &'w str) -> Route<'w> {
    step.route
        .with_keys([Component::from("with"), Component::from(key)])
}
