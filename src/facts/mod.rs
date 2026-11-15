mod checkout;
mod output;
mod secrets;
mod tools;
mod trigger;
mod workspace;

use std::ops::Range;

use github_actions_expressions::call::Function;
use github_actions_expressions::context::Context;
use github_actions_expressions::literal::Literal;
use github_actions_expressions::op::{BinExpr, BinOp, UnOp};
use github_actions_expressions::{Expr, SpannedExpr};
use github_actions_models::common::expr::LoE;
use github_actions_models::common::{Env, EnvValue, If};
use github_actions_models::workflow::job::NormalJob;
use yamlpath::{Component, Route, route};

use crate::agent::{Agent, AgentStep, JobStep, job_steps};
use crate::workflow::Workflow;

pub use checkout::{HeadCheckout, head_checkout, outside_text_cannot_reach};
pub use output::{OutputInjection, output_injection};
pub use secrets::{
    SecretEnv, SecretUse, agent_secrets, long_lived_token, other_secret, secret_env,
};
pub use tools::{BroadTool, broad_tool};
pub use trigger::{
    ExternalEvent, OpenTrigger, UpstreamOrigin, Upstreams, condition_origin, external_events,
    never_runs, open_trigger, outside_events,
};
pub use workspace::{AgentCode, agent_code_with_secrets};

const EXPRESSION_START: &str = "${{";
const EXPRESSION_END: &str = "}}";
const ENV_CONTEXT: &str = "env";
const SETTINGS_INPUT: &str = "settings";
const CLAUDE_ENV_INPUT: &str = "claude_env";
const LINE_CONTINUATION: &str = "\\\n";
const SHELL_COMMAND_FLAG: &str = "-c";
const COMMAND_SEPARATORS: &str = ";&|()`";
const WORD_SEPARATORS: &str = "\"'";
const STABLE_CONTEXTS: [&str; 15] = [
    "inputs.",
    "vars.",
    "needs.",
    "matrix.",
    "github.event.",
    "github.event_name",
    "github.ref",
    "github.head_ref",
    "github.base_ref",
    "github.repository",
    "github.actor",
    "github.triggering_actor",
    "github.workflow",
    "github.sha",
    "github.job",
];
const NUMBER_WORDS: [&str; 2] = ["infinity", "nan"];
const HEREDOC: &str = "<<";
const HERE_STRING: &str = "<<<";
const SHELL_KEYWORDS: [&str; 9] = [
    "if", "then", "else", "elif", "do", "while", "until", "!", "{",
];

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

fn enabled(step: &AgentStep, name: &str) -> bool {
    let LoE::Literal(with) = &step.step.with else {
        return false;
    };
    with.iter().any(|(key, value)| {
        key.eq_ignore_ascii_case(name)
            && match value {
                EnvValue::Boolean(enabled) => *enabled,
                EnvValue::String(text) => text.trim() == "true",
                EnvValue::Number(_) => false,
            }
    })
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

fn running_later_steps<'w>(step: &AgentStep<'w>) -> Vec<(Route<'w>, JobStep<'w>)> {
    let agent_facts: Vec<ValueFact> = step
        .step
        .shared
        .r#if
        .as_ref()
        .and_then(parse_condition)
        .map_or_else(Vec::new, |condition| value_facts(&condition));
    if contradicts(&agent_facts, &[]) {
        return Vec::new();
    }
    job_steps(step.job_id, step.job)
        .into_iter()
        .filter(|(index, ..)| *index > step.index)
        .filter(|(.., later)| {
            let shared = match later {
                JobStep::Run(run) => &run.shared,
                JobStep::Uses(uses) => &uses.shared,
            };
            let Some(condition) = shared.r#if.as_ref().and_then(parse_condition) else {
                return true;
            };
            !constant_false(&condition) && !contradicts(&agent_facts, &value_facts(&condition))
        })
        .map(|(_, route, later)| (route, later))
        .collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Membership {
    OneOf,
    NoneOf,
}

struct ValueFact {
    value: String,
    membership: Membership,
    constants: Vec<String>,
}

impl ValueFact {
    fn rules_out(&self, constant: &String) -> bool {
        match self.membership {
            Membership::OneOf => !self.constants.contains(constant),
            Membership::NoneOf => self.constants.contains(constant),
        }
    }
}

fn contradicts(first: &[ValueFact], second: &[ValueFact]) -> bool {
    let facts = || first.iter().chain(second);
    facts()
        .filter(|fact| fact.membership == Membership::OneOf)
        .any(|fact| {
            fact.constants.iter().all(|constant| {
                facts().any(|other| other.value == fact.value && other.rules_out(constant))
            })
        })
}

fn value_facts(condition: &SpannedExpr) -> Vec<ValueFact> {
    let fact = |value: &SpannedExpr, membership, constants: Vec<String>| {
        Some(ValueFact {
            value: stable_value(value)?,
            membership,
            constants: comparable_constants(constants)?,
        })
    };
    let membership_fact = |expression: &SpannedExpr, membership| match &expression.inner {
        Expr::Call(call) if call.func == Function::Contains => match call.args.as_slice() {
            [list, value] => fact(value, membership, json_string_list(list)?),
            _ => None,
        },
        _ => None,
    };
    match &condition.inner {
        Expr::BinExpr(BinExpr {
            op: BinOp::And,
            lhs,
            rhs,
        }) => {
            let mut facts = value_facts(lhs);
            facts.extend(value_facts(rhs));
            facts
        }
        Expr::BinExpr(BinExpr {
            op: op @ (BinOp::Eq | BinOp::Neq),
            lhs,
            rhs,
        }) => {
            let membership = if *op == BinOp::Eq {
                Membership::OneOf
            } else {
                Membership::NoneOf
            };
            match (&lhs.inner, &rhs.inner) {
                (_, Expr::Literal(Literal::String(constant))) => {
                    fact(lhs, membership, vec![constant.to_string()])
                }
                (Expr::Literal(Literal::String(constant)), _) => {
                    fact(rhs, membership, vec![constant.to_string()])
                }
                _ => None,
            }
            .into_iter()
            .collect()
        }
        Expr::UnExpr {
            op: UnOp::Not,
            expr,
        } => membership_fact(expr, Membership::NoneOf)
            .into_iter()
            .collect(),
        _ => membership_fact(condition, Membership::OneOf)
            .into_iter()
            .collect(),
    }
}

fn stable_value(expression: &SpannedExpr) -> Option<String> {
    let Expr::Context(context) = &expression.inner else {
        return None;
    };
    if context
        .parts
        .iter()
        .any(|part| matches!(part.inner, Expr::Index(_) | Expr::Star))
    {
        return None;
    }
    context.as_pattern().filter(|pattern| {
        STABLE_CONTEXTS
            .iter()
            .any(|prefix| pattern.starts_with(prefix))
    })
}

fn comparable_constants(constants: Vec<String>) -> Option<Vec<String>> {
    constants
        .into_iter()
        .map(|constant| {
            let trimmed = constant.trim();
            let comparable = trimmed.is_ascii()
                && trimmed.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                && !NUMBER_WORDS
                    .iter()
                    .any(|word| trimmed.eq_ignore_ascii_case(word));
            comparable.then(|| constant.to_ascii_lowercase())
        })
        .collect()
}

fn json_string_list(expression: &SpannedExpr) -> Option<Vec<String>> {
    let Expr::Call(from_json) = &expression.inner else {
        return None;
    };
    let (Function::FromJSON, [json]) = (&from_json.func, from_json.args.as_slice()) else {
        return None;
    };
    let Expr::Literal(Literal::String(text)) = &json.inner else {
        return None;
    };
    serde_json::from_str(text).ok()
}

fn shell_programs(run: &str) -> Vec<Vec<String>> {
    let script = flatten_expressions(&run.replace(LINE_CONTINUATION, " "));
    let mut programs = Vec::new();
    let mut heredoc_end: Option<String> = None;
    let mut commands = Vec::new();
    for line in script.lines() {
        if let Some(end) = &heredoc_end {
            if line.trim() == end {
                heredoc_end = None;
            }
            continue;
        }
        let line = before_comment(line);
        if let Some(start) = line.find(HEREDOC)
            && !line[start..].starts_with(HERE_STRING)
        {
            let marker: String = line[start + HEREDOC.len()..]
                .trim_start_matches('-')
                .trim_start()
                .trim_start_matches(['\'', '"', '\\'])
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            heredoc_end = (!marker.is_empty()).then_some(marker);
        }
        commands.extend(line.split(|c: char| COMMAND_SEPARATORS.contains(c)));
    }
    for command in commands {
        let words: Vec<&str> = command
            .split(|c: char| c.is_whitespace() || WORD_SEPARATORS.contains(c))
            .filter(|word| !word.is_empty())
            .collect();
        for position in program_positions(&words) {
            programs.push(
                words[position..]
                    .iter()
                    .map(|word| word.to_string())
                    .collect(),
            );
        }
    }
    programs
}

fn flatten_expressions(text: &str) -> String {
    let mut flattened = String::new();
    let mut last = 0;
    for span in expression_spans(text) {
        flattened.push_str(&text[last..span.start]);
        flattened.extend(text[span.clone()].chars().filter(|c| {
            !c.is_whitespace() && !COMMAND_SEPARATORS.contains(*c) && !WORD_SEPARATORS.contains(*c)
        }));
        last = span.end;
    }
    flattened.push_str(&text[last..]);
    flattened
}

fn program_positions(words: &[&str]) -> Vec<usize> {
    let first = words
        .iter()
        .position(|word| !is_assignment(word) && !SHELL_KEYWORDS.contains(word));
    let after_shell_flag = words
        .iter()
        .enumerate()
        .filter(|(_, word)| **word == SHELL_COMMAND_FLAG)
        .map(|(index, _)| index + 1)
        .filter(|index| *index < words.len());
    first.into_iter().chain(after_shell_flag).collect()
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
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
        let body = start + EXPRESSION_START.len();
        let mut in_string = false;
        let mut outside_strings = None;
        for index in body..bytes.len() {
            match bytes[index] {
                b'\'' => in_string = !in_string,
                b'}' if !in_string && text[index..].starts_with(EXPRESSION_END) => {
                    outside_strings = Some(index);
                    break;
                }
                _ => {}
            }
        }
        let end = outside_strings
            .or_else(|| text[body..].find(EXPRESSION_END).map(|end| body + end))
            .map_or(text.len(), |end| end + EXPRESSION_END.len());
        spans.push(start..end);
        offset = end;
    }
    spans
}

fn flowing_contexts<'a, 'src>(expression: &'a SpannedExpr<'src>) -> Vec<&'a Context<'src>> {
    expression
        .dataflow_contexts()
        .into_iter()
        .flat_map(|(context, _)| {
            let mut found = vec![context];
            if let Some(head) = context.parts.first()
                && matches!(head.inner, Expr::Call(_))
            {
                found.extend(flowing_contexts(head));
            }
            found
        })
        .collect()
}

fn constant_false(condition: &SpannedExpr) -> bool {
    condition
        .consteval()
        .is_some_and(|value| !value.as_boolean())
}

fn parse_condition(condition: &If) -> Option<SpannedExpr<'_>> {
    match condition {
        If::Bool(runs) => Expr::parse(if *runs { "true" } else { "false" }).ok(),
        If::Expr(raw) => parse_expression(raw),
    }
}

fn parse_expression(text: &str) -> Option<SpannedExpr<'_>> {
    let trimmed = text.trim();
    let body = trimmed
        .strip_prefix(EXPRESSION_START)
        .map(|body| body.strip_suffix(EXPRESSION_END).unwrap_or(body))
        .unwrap_or(trimmed);
    Expr::parse(body.trim()).ok()
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
    let mut env = nearest_env(
        workflow,
        step.job_id,
        step.job,
        &step.step.shared.env,
        &step.route,
    );
    if step.agent != Agent::ClaudeBaseBeta {
        return env;
    }
    let Some((key, value)) = input(step, CLAUDE_ENV_INPUT) else {
        return env;
    };
    let route = input_route(step, key);
    let mut variables: Vec<EnvVariable<'w>> = value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| !name.trim().is_empty())
        .map(|(name, value)| EnvVariable {
            name: name.trim(),
            value: value.trim().to_string(),
            route: route.clone(),
        })
        .collect();
    let overridden: Vec<&str> = variables.iter().map(|variable| variable.name).collect();
    variables.extend(
        env.variables
            .into_iter()
            .filter(|variable| !overridden.contains(&variable.name)),
    );
    env.variables = variables;
    env
}
