use github_actions_expressions::{Expr, SpannedExpr};
use yamlpath::{Component, Route};

use super::{Input, expression_spans, lookup_in, parse_expression};
use crate::agent::{Agent, AgentStep, JobStep, is_root_action, job_steps};

const MODEL_TEXT_OUTPUTS: [(Agent, &[&str]); 3] = [
    (Agent::ClaudeCode, &["structured_output"]),
    (Agent::Codex, &["final-message"]),
    (Agent::GeminiCli, &["summary", "error"]),
];
const SCRIPT_ACTION: &str = "actions/github-script";
const SCRIPT_INPUT: &str = "script";
const RUN_KEY: &str = "run";
const WITH_KEY: &str = "with";

#[derive(Debug)]
pub struct OutputInjection<'w> {
    pub setting: String,
    pub route: Route<'w>,
}

pub fn output_injection<'w>(step: &AgentStep<'w>) -> Option<OutputInjection<'w>> {
    let id = step.step.shared.id.as_deref()?;
    let (_, outputs) = MODEL_TEXT_OUTPUTS
        .iter()
        .find(|(agent, _)| *agent == step.agent)?;
    let outputs: Vec<String> = outputs
        .iter()
        .map(|output| format!("steps.{id}.outputs.{output}"))
        .collect();
    job_steps(step.job_id, step.job)
        .into_iter()
        .filter(|(index, ..)| *index > step.index)
        .find_map(|(_, route, later)| {
            let (key, code, route) = match later {
                JobStep::Run(run) => (RUN_KEY, run.run.as_str(), route.with_key(RUN_KEY)),
                JobStep::Uses(uses) if is_root_action(&uses.uses, SCRIPT_ACTION) => {
                    match lookup_in(&uses.with, SCRIPT_INPUT) {
                        Input::Text { key, value } => (
                            key,
                            value,
                            route.with_keys([Component::from(WITH_KEY), Component::from(key)]),
                        ),
                        Input::Missing | Input::Unreadable => return None,
                    }
                }
                JobStep::Uses(_) => return None,
            };
            let output = expression_spans(code).into_iter().find_map(|span| {
                let expression = parse_expression(&code[span])?;
                outputs
                    .iter()
                    .find(|output| flows_into(&expression, output))
            })?;
            Some(OutputInjection {
                setting: format!("a later step expands `{output}` into its `{key}`"),
                route,
            })
        })
}

fn flows_into(expression: &SpannedExpr, output: &str) -> bool {
    expression
        .dataflow_contexts()
        .into_iter()
        .any(|(context, _)| {
            context.matches(output)
                || context.parts.first().is_some_and(|head| {
                    matches!(head.inner, Expr::Call(_)) && flows_into(head, output)
                })
        })
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
        output_injection(&steps[0]).map(|found| {
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
        ] {
            assert_eq!(injection(agent, later), None, "{later}");
        }
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
        assert!(output_injection(&agent_steps(&workflow)[0]).is_none());
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
        assert!(output_injection(&agent_steps(&without_id)[0]).is_none());
    }
}
