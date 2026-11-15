use github_actions_expressions::call::Function;
use github_actions_expressions::op::{BinExpr, BinOp};
use github_actions_expressions::{Expr, SpannedExpr};
use github_actions_models::common::expr::LoE;
use yamlpath::{Component, Route};

use super::secrets::shell_secret;
use super::{
    Input, expression_spans, flowing_contexts, lookup_in, nearest_env, parse_condition,
    parse_expression,
};
use crate::agent::{Agent, AgentStep, JobStep, is_root_action, job_steps};
use crate::workflow::Workflow;

const SCRIPT_ACTION: &str = "actions/github-script";
const SCRIPT_INPUT: &str = "script";
const RUN_KEY: &str = "run";
const WITH_KEY: &str = "with";
const STEPS_CONTEXT: &str = "steps";

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
        Agent::ClaudeCode => &["structured_output"],
        Agent::Codex => &["final-message"],
        Agent::GeminiCli => &["summary", "error"],
    };
    let text_outputs: Vec<String> = model_text
        .iter()
        .map(|output| format!("steps.{id}.outputs.{output}"))
        .collect();
    let aggregates = [
        format!("steps.{id}.outputs"),
        format!("steps.{id}"),
        STEPS_CONTEXT.to_string(),
    ];
    let mut expansions = job_steps(step.job_id, step.job)
        .into_iter()
        .filter(|(index, ..)| *index > step.index)
        .filter_map(|(_, route, later)| {
            let (shared, key, code, code_route) = match later {
                JobStep::Run(run) => (
                    &run.shared,
                    RUN_KEY,
                    run.run.as_str(),
                    route.with_key(RUN_KEY),
                ),
                JobStep::Uses(uses) if is_root_action(&uses.uses, SCRIPT_ACTION) => {
                    let Input::Text { key, value } = lookup_in(&uses.with, SCRIPT_INPUT) else {
                        return None;
                    };
                    (
                        &uses.shared,
                        key,
                        value,
                        route.with_keys([Component::from(WITH_KEY), Component::from(key)]),
                    )
                }
                JobStep::Uses(_) => return None,
            };
            let never_runs = shared
                .r#if
                .as_ref()
                .and_then(parse_condition)
                .and_then(|condition| condition.consteval())
                .is_some_and(|value| !value.as_boolean());
            if never_runs {
                return None;
            }
            let output = expression_spans(code).into_iter().find_map(|span| {
                let expression = parse_expression(&code[span])?;
                let contexts = flowing_contexts(&expression);
                text_outputs
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
            })?;
            let secret_source = match later {
                JobStep::Run(_) => code.to_string(),
                JobStep::Uses(uses) => match &uses.with {
                    LoE::Literal(with) => with
                        .values()
                        .map(|value| value.to_string())
                        .collect::<Vec<_>>()
                        .join("\n"),
                    LoE::Expr(expression) => expression.as_raw().to_string(),
                },
            };
            let env = nearest_env(workflow, step.job_id, step.job, &shared.env, &route);
            Some(OutputInjection {
                setting: format!("a later step expands `{output}` into its `{key}`"),
                secret: shell_secret(env, &secret_source),
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

fn expands_as_json(expression: &SpannedExpr, aggregate: &str) -> bool {
    match &expression.inner {
        Expr::Call(call) if call.func == Function::ToJSON => call.args.iter().any(|argument| {
            flowing_contexts(argument)
                .iter()
                .any(|context| context.matches(aggregate))
        }),
        Expr::Call(call) if matches!(call.func, Function::Format | Function::Join) => call
            .args
            .iter()
            .any(|argument| expands_as_json(argument, aggregate)),
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
