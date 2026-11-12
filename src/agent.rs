use github_actions_models::common::Uses;
use github_actions_models::workflow::Job;
use github_actions_models::workflow::job::{NormalJob, ParallelStep, Step, UsesStep};
use yamlpath::{Component, Route, route};

use crate::workflow::Workflow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    ClaudeCode,
    Codex,
    GeminiCli,
}

impl Agent {
    const ALL: [Agent; 3] = [Agent::ClaudeCode, Agent::Codex, Agent::GeminiCli];

    pub fn action(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "anthropics/claude-code-action",
            Agent::Codex => "openai/codex-action",
            Agent::GeminiCli => "google-github-actions/run-gemini-cli",
        }
    }

    fn of(uses: &Uses) -> Option<Agent> {
        let Uses::Repository(repository) = uses else {
            return None;
        };
        if repository.subpath().is_some() {
            return None;
        }
        Agent::ALL
            .into_iter()
            .find(|agent| agent.action().eq_ignore_ascii_case(repository.slug()))
    }
}

#[derive(Debug)]
pub struct AgentStep<'w> {
    pub agent: Agent,
    pub job_id: &'w str,
    pub step: &'w UsesStep,
    pub route: Route<'w>,
}

pub fn agent_steps(workflow: &Workflow) -> Vec<AgentStep<'_>> {
    workflow
        .model
        .jobs
        .iter()
        .filter_map(|(job_id, job)| match job {
            Job::NormalJob(job) => Some((job_id.as_str(), job.as_ref())),
            Job::ReusableWorkflowCallJob(_) => None,
        })
        .flat_map(|(job_id, job)| {
            uses_steps(job_id, job)
                .into_iter()
                .filter_map(move |(route, step)| {
                    Agent::of(&step.uses).map(|agent| AgentStep {
                        agent,
                        job_id,
                        step,
                        route,
                    })
                })
        })
        .collect()
}

fn uses_steps<'w>(job_id: &'w str, job: &'w NormalJob) -> Vec<(Route<'w>, &'w UsesStep)> {
    let steps = route!["jobs", job_id, "steps"];
    job.steps
        .iter()
        .enumerate()
        .flat_map(|(index, step)| {
            let route = steps.with_key(index);
            match step {
                Step::Uses(step) => vec![(route, step)],
                Step::Parallel { parallel } => parallel
                    .iter()
                    .enumerate()
                    .filter_map(|(inner, step)| match step {
                        ParallelStep::Uses(step) => Some((
                            route.with_keys([Component::from("parallel"), Component::from(inner)]),
                            step,
                        )),
                        ParallelStep::Run(_) => None,
                    })
                    .collect(),
                Step::Run(_) | Step::Wait { .. } | Step::WaitAll { .. } | Step::Cancel { .. } => {
                    Vec::new()
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn workflow(source: &str) -> Workflow {
        Workflow::parse(Path::new("test.yml"), source.to_string()).unwrap()
    }

    fn line_of(workflow: &Workflow, step: &AgentStep) -> usize {
        workflow
            .document
            .query_pretty(&step.route)
            .unwrap()
            .location
            .point_span
            .0
            .0
            + 1
    }

    #[test]
    fn finds_each_official_agent_action_with_its_job_and_line() {
        let workflow = workflow(
            "on: issue_comment
jobs:
  claude:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - uses: anthropics/claude-code-action@v1
  codex:
    runs-on: ubuntu-latest
    steps:
      - uses: openai/codex-action@0123456789abcdef0123456789abcdef01234567
  gemini:
    runs-on: ubuntu-latest
    steps:
      - uses: google-github-actions/run-gemini-cli@v0
",
        );
        let found: Vec<_> = agent_steps(&workflow)
            .iter()
            .map(|step| (step.agent, step.job_id, line_of(&workflow, step)))
            .collect();
        assert_eq!(
            found,
            [
                (Agent::ClaudeCode, "claude", 7),
                (Agent::Codex, "codex", 11),
                (Agent::GeminiCli, "gemini", 15),
            ]
        );
    }

    #[test]
    fn owner_and_repository_are_compared_ignoring_case() {
        let workflow = workflow(
            "on: issues
jobs:
  agent:
    runs-on: ubuntu-latest
    steps:
      - uses: Anthropics/Claude-Code-Action@v1
",
        );
        let agents: Vec<_> = agent_steps(&workflow)
            .iter()
            .map(|step| step.agent)
            .collect();
        assert_eq!(agents, [Agent::ClaudeCode]);
    }

    #[test]
    fn other_actions_subpaths_local_docker_and_run_steps_are_not_agents() {
        let workflow = workflow(
            "on: issues
jobs:
  other:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - uses: anthropics/claude-code-action/base-action@v1
      - uses: anthropics/claude-code-base-action@v1
      - uses: ./.github/actions/claude-code-action
      - uses: docker://openai/codex-action:latest
      - run: npx @google/gemini-cli
",
        );
        assert!(agent_steps(&workflow).is_empty());
    }

    #[test]
    fn finds_agents_inside_parallel_steps() {
        let workflow = workflow(
            "on: issues
jobs:
  review:
    runs-on: ubuntu-latest
    steps:
      - parallel:
          - run: echo start
          - uses: openai/codex-action@v1
",
        );
        let steps = agent_steps(&workflow);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].agent, Agent::Codex);
        assert_eq!(line_of(&workflow, &steps[0]), 8);
    }

    #[test]
    fn reusable_workflow_calls_are_not_followed() {
        let workflow = workflow(
            "on: issues
jobs:
  call:
    uses: anthropics/claude-code-action/.github/workflows/claude.yml@v1
",
        );
        assert!(agent_steps(&workflow).is_empty());
    }
}
