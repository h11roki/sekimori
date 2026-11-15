use std::borrow::Cow;

use github_actions_expressions::call::Function;
use github_actions_expressions::op::{BinExpr, BinOp};
use github_actions_expressions::{Expr, SpannedExpr};
use github_actions_models::common::expr::LoE;
use yamlpath::{Component, Route};

use super::secrets::shell_secrets;
use super::{
    ENV_CONTEXT, Input, expression_spans, flowing_contexts, input, lookup_in, nearest_env,
    parse_expression, running_later_steps,
};
use crate::agent::{Agent, AgentStep, JobStep, is_root_action};
use crate::workflow::Workflow;

const SCRIPT_ACTION: &str = "actions/github-script";
const SCRIPT_INPUT: &str = "script";
const RUN_KEY: &str = "run";
const WITH_KEY: &str = "with";
const STEPS_CONTEXT: &str = "steps";
const CLAUDE_ARGS_INPUT: &str = "claude_args";
const JSON_SCHEMA_FLAG: &str = "--json-schema";

#[derive(Debug)]
pub struct OutputInjection<'w> {
    pub setting: String,
    pub route: Route<'w>,
    pub secret: Option<String>,
}

pub fn output_injection<'w>(
    workflow: &'w Workflow,
    step: &AgentStep<'w>,
) -> Option<OutputInjection<'w>> {
    let id = step.step.shared.id.as_deref()?;
    let model_text: &[&str] = match step.agent {
        Agent::ClaudeCode | Agent::ClaudeBase => &["structured_output"],
        Agent::ClaudeBaseBeta => &[],
        Agent::Codex => &["final-message"],
        Agent::GeminiCli => &["summary", "error"],
    };
    let text_outputs: Vec<String> = model_text
        .iter()
        .map(|output| format!("steps.{id}.outputs.{output}"))
        .collect();
    let aggregates_hold_model_text = !matches!(
        step.agent,
        Agent::ClaudeCode | Agent::ClaudeBase | Agent::ClaudeBaseBeta
    ) || input(step, CLAUDE_ARGS_INPUT)
        .is_some_and(|(_, value)| value.contains(JSON_SCHEMA_FLAG));
    let aggregates: Vec<String> = if aggregates_hold_model_text {
        vec![
            format!("steps.{id}.outputs"),
            format!("steps.{id}"),
            STEPS_CONTEXT.to_string(),
        ]
    } else {
        Vec::new()
    };
    let mut expansions = running_later_steps(step)
        .into_iter()
        .filter_map(|(route, later)| {
            let (shared, key, code, code_route, script_inputs) = match later {
                JobStep::Run(run) => (
                    &run.shared,
                    RUN_KEY,
                    run.run.as_str(),
                    route.with_key(RUN_KEY),
                    None,
                ),
                JobStep::Uses(uses) if is_root_action(&uses.uses, SCRIPT_ACTION) => {
                    let (LoE::Literal(with), Input::Text { key, value }) =
                        (&uses.with, lookup_in(&uses.with, SCRIPT_INPUT))
                    else {
                        return None;
                    };
                    (
                        &uses.shared,
                        key,
                        value,
                        route.with_keys([Component::from(WITH_KEY), Component::from(key)]),
                        Some(with),
                    )
                }
                JobStep::Uses(_) => return None,
            };
            let env = nearest_env(workflow, step.job_id, step.job, &shared.env, &route);
            let mut sources = text_outputs.clone();
            sources.extend(
                env.variables
                    .iter()
                    .filter(|variable| {
                        expanded_output(&variable.value, &text_outputs, &[]).is_some()
                    })
                    .map(|variable| format!("{ENV_CONTEXT}.{}", variable.name)),
            );
            let output = expanded_output(code, &sources, &aggregates)?.to_string();
            let secret_source = match script_inputs {
                Some(with) => Cow::Owned(
                    with.values()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                None => Cow::Borrowed(code),
            };
            Some(OutputInjection {
                setting: format!("a later step expands `{output}` into its `{key}`"),
                secret: shell_secrets(env, &secret_source).into_iter().next(),
                route: code_route,
            })
        });
    let first = expansions.next()?;
    let secret = first
        .secret
        .clone()
        .or_else(|| expansions.find_map(|later| later.secret));
    Some(OutputInjection { secret, ..first })
}

fn expanded_output<'a>(
    text: &str,
    outputs: &'a [String],
    aggregates: &'a [String],
) -> Option<&'a String> {
    expression_spans(text).into_iter().find_map(|span| {
        let expression = parse_expression(&text[span])?;
        let contexts = flowing_contexts(&expression);
        outputs
            .iter()
            .find(|output| {
                contexts
                    .iter()
                    .any(|context| context.matches(output.as_str()))
            })
            .or_else(|| {
                aggregates
                    .iter()
                    .find(|aggregate| expands_as_json(&expression, aggregate))
            })
    })
}

fn expands_as_json(expression: &SpannedExpr, aggregate: &str) -> bool {
    match &expression.inner {
        Expr::Call(call) => match call.func {
            Function::ToJSON => call.args.iter().any(|argument| {
                flowing_contexts(argument)
                    .iter()
                    .any(|context| context.matches(aggregate))
            }),
            Function::Format | Function::Join | Function::FromJSON => call
                .args
                .iter()
                .any(|argument| expands_as_json(argument, aggregate)),
            Function::Case => call
                .args
                .iter()
                .skip(1)
                .step_by(2)
                .chain(call.args.last())
                .any(|argument| expands_as_json(argument, aggregate)),
            _ => false,
        },
        Expr::Context(context) => context
            .parts
            .first()
            .is_some_and(|head| expands_as_json(head, aggregate)),
        Expr::BinExpr(BinExpr {
            op: BinOp::And,
            rhs,
            ..
        }) => expands_as_json(rhs, aggregate),
        Expr::BinExpr(BinExpr {
            op: BinOp::Or,
            lhs,
            rhs,
        }) => expands_as_json(lhs, aggregate) || expands_as_json(rhs, aggregate),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::agent::agent_steps;
    use crate::workflow::Workflow;

    fn injection(agent: &str, later: &str) -> Option<String> {
        let workflow = Workflow::parse(
            Path::new("test.yml"),
            format!(
                "on: issue_comment
jobs:
  agent:
    runs-on: ubuntu-latest
    steps:
      - id: agent
        uses: {agent}
{later}"
            ),
        )
        .unwrap();
        let steps = agent_steps(&workflow);
        output_injection(&workflow, &steps[0]).map(|found| {
            let (line, _) = workflow.position(&found.route).unwrap();
            format!("{line}: {}", found.setting)
        })
    }

    #[test]
    fn model_text_expanded_into_a_later_run_or_script_is_found() {
        for (agent, later, expected) in [
            (
                "google-github-actions/run-gemini-cli@v0",
                "      - run: echo \"${{ steps.agent.outputs.summary }}\"",
                "8: a later step expands `steps.agent.outputs.summary` into its `run`",
            ),
            (
                "google-github-actions/run-gemini-cli@v0",
                "      - run: echo '${{ steps.agent.outputs.error || 'none' }}'",
                "8: a later step expands `steps.agent.outputs.error` into its `run`",
            ),
            (
                "openai/codex-action@v1",
                "      - run: echo \"${{ steps.agent.outputs['final-message'] }}\"",
                "8: a later step expands `steps.agent.outputs.final-message` into its `run`",
            ),
            (
                "anthropics/claude-code-action@v1",
                "      - uses: actions/github-script@v8
        with:
          script: |
            core.info(`${{ fromJSON(steps.agent.outputs.structured_output).title }}`)",
                "10: a later step expands `steps.agent.outputs.structured_output` into its `script`",
            ),
            (
                "google-github-actions/run-gemini-cli@v0",
                "      - run: echo '${{ toJSON(steps.agent.outputs) }}' | jq .summary",
                "8: a later step expands `steps.agent.outputs` into its `run`",
            ),
            (
                "google-github-actions/run-gemini-cli@v0",
                "      - run: echo '${{ format('{0}', toJSON(steps)) }}' | jq -r .agent.outputs.summary",
                "8: a later step expands `steps` into its `run`",
            ),
            (
                "google-github-actions/run-gemini-cli@v0",
                "      - run: echo \"${{ case(github.event_name == 'issues', toJSON(steps.agent.outputs), '') }}\"",
                "8: a later step expands `steps.agent.outputs` into its `run`",
            ),
            (
                "google-github-actions/run-gemini-cli@v0",
                "      - run: echo \"${{ fromJSON(toJSON(steps.agent.outputs)).summary }}\"",
                "8: a later step expands `steps.agent.outputs` into its `run`",
            ),
            (
                "google-github-actions/run-gemini-cli@v0",
                "      - run: echo \"${{ steps.agent.outcome == 'success' && toJSON(steps.agent.outputs) || '' }}\"",
                "8: a later step expands `steps.agent.outputs` into its `run`",
            ),
            (
                "google-github-actions/run-gemini-cli@v0",
                "      - run: echo \"${{ join(toJSON(steps), ' ') }}\"",
                "8: a later step expands `steps` into its `run`",
            ),
            (
                "google-github-actions/run-gemini-cli@v0",
                "      - run: echo \"${{ env.ANSWER }}\"
        env:
          ANSWER: ${{ steps.agent.outputs.summary }}",
                "8: a later step expands `env.ANSWER` into its `run`",
            ),
            (
                "anthropics/claude-code-action@v1
        with:
          claude_args: --json-schema '{\"type\":\"object\"}'",
                "      - run: echo '${{ toJSON(steps.agent.outputs) }}'",
                "10: a later step expands `steps.agent.outputs` into its `run`",
            ),
        ] {
            assert_eq!(
                injection(agent, later).as_deref(),
                Some(expected),
                "{later}"
            );
        }
    }

    #[test]
    fn safe_uses_of_the_output_are_not_found() {
        for (agent, later) in [
            (
                "google-github-actions/run-gemini-cli@v0",
                "      - run: echo \"$SUMMARY\"
        env:
          SUMMARY: ${{ steps.agent.outputs.summary }}",
            ),
            (
                "google-github-actions/run-gemini-cli@v0",
                "      - run: echo \"${{ steps.agent.outputs.summary != '' }}\"",
            ),
            (
                "google-github-actions/run-gemini-cli@v0",
                "      - uses: peter-evans/create-or-update-comment@v4
        with:
          body: ${{ steps.agent.outputs.summary }}",
            ),
            (
                "anthropics/claude-code-action@v1",
                "      - run: cat \"${{ steps.agent.outputs.execution_file }}\"",
            ),
            (
                "openai/codex-action@v1",
                "      - run: echo \"${{ steps.other.outputs.final-message }}\"",
            ),
            (
                "google-github-actions/run-gemini-cli@v0",
                "      - run: echo \"${{ steps.agent.outputs }} ${{ steps.agent }}\"",
            ),
            (
                "anthropics/claude-code-action@v1",
                "      - run: echo '${{ toJSON(steps.agent.outputs) }}' | jq .session_id",
            ),
            (
                "google-github-actions/run-gemini-cli@v0
        if: inputs.mode == 'review'",
                "      - if: inputs.mode == 'fix'
        run: echo \"${{ steps.agent.outputs.summary }}\"",
            ),
            (
                "google-github-actions/run-gemini-cli@v0",
                "      - if: false
        run: echo \"${{ steps.agent.outputs.summary }}\"",
            ),
            (
                "google-github-actions/run-gemini-cli@v0",
                "      - uses: actions/github-script@v8
        with: ${{ fromJSON(vars.SCRIPT) }}",
            ),
        ] {
            assert_eq!(injection(agent, later), None, "{later}");
        }
    }

    #[test]
    fn secrets_that_reach_the_expanding_step_are_reported() {
        let secret_of = |later: &str| {
            let workflow = Workflow::parse(
                Path::new("test.yml"),
                format!(
                    "on: issue_comment
env:
  REGION: us
jobs:
  agent:
    runs-on: ubuntu-latest
    steps:
      - id: agent
        uses: google-github-actions/run-gemini-cli@v0
{later}"
                ),
            )
            .unwrap();
            output_injection(&workflow, &agent_steps(&workflow)[0]).and_then(|found| found.secret)
        };
        assert_eq!(
            secret_of(
                "      - run: npm publish --tag \"${{ steps.agent.outputs.summary }}\"
        env:
          NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}"
            )
            .as_deref(),
            Some("secrets.NPM_TOKEN")
        );
        assert_eq!(
            secret_of(
                "      - uses: actions/github-script@v8
        with:
          github-token: ${{ secrets.BOT_PAT }}
          script: core.info(`${{ steps.agent.outputs.summary }}`)"
            )
            .as_deref(),
            Some("secrets.BOT_PAT")
        );
        assert_eq!(
            secret_of(
                "      - run: echo \"${{ steps.agent.outputs.summary }}\"
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}"
            ),
            None
        );
        assert_eq!(
            secret_of(
                "      - run: echo \"${{ steps.agent.outputs.summary }}\" >> \"$GITHUB_STEP_SUMMARY\"
      - run: npm publish --tag \"${{ steps.agent.outputs.summary }}\"
        env:
          NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}"
            )
            .as_deref(),
            Some("secrets.NPM_TOKEN")
        );
        assert_eq!(
            secret_of(
                "      - run: echo \"${{ steps.agent.outputs.summary }}\"
        env: ${{ fromJSON(secrets.DEPLOY_ENV) }}"
            )
            .as_deref(),
            Some("secrets.DEPLOY_ENV")
        );
    }

    #[test]
    fn only_steps_after_the_agent_in_its_job_count() {
        let workflow = Workflow::parse(
            Path::new("test.yml"),
            "on: issue_comment
jobs:
  agent:
    runs-on: ubuntu-latest
    steps:
      - parallel:
          - id: agent
            uses: google-github-actions/run-gemini-cli@v0
          - run: echo \"${{ steps.agent.outputs.summary }}\"
  other:
    runs-on: ubuntu-latest
    steps:
      - run: echo \"${{ steps.agent.outputs.summary }}\"
"
            .to_string(),
        )
        .unwrap();
        assert!(output_injection(&workflow, &agent_steps(&workflow)[0]).is_none());
        let without_id = Workflow::parse(
            Path::new("test.yml"),
            "on: issue_comment
jobs:
  agent:
    runs-on: ubuntu-latest
    steps:
      - uses: google-github-actions/run-gemini-cli@v0
      - run: echo \"${{ steps.agent.outputs.summary }}\"
"
            .to_string(),
        )
        .unwrap();
        assert!(output_injection(&without_id, &agent_steps(&without_id)[0]).is_none());
    }
}
