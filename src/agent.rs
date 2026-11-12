use github_actions_models::common::Uses;
use github_actions_models::workflow::Job;
use github_actions_models::workflow::job::{ParallelStep, Step, UsesStep};
use yamlpath::{Component, Route, route};

use crate::workflow::Workflow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    ClaudeCode,
    Codex,
    GeminiCli,
}

impl Agent {
    fn of(uses: &Uses) -> Option<Agent> {
        let Uses::Repository(repository) = uses else {
            return None;
        };
        if repository
            .subpath()
            .is_some_and(|subpath| !subpath.is_empty())
        {
            return None;
        }
        match repository.slug().to_ascii_lowercase().as_str() {
            "anthropics/claude-code-action" => Some(Agent::ClaudeCode),
            "openai/codex-action" => Some(Agent::Codex),
            "google-github-actions/run-gemini-cli" => Some(Agent::GeminiCli),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct AgentStep<'w> {
    pub agent: Agent,
    pub job_id: &'w str,
    pub step: &'w UsesStep,
    pub route: Route<'w>,
}

impl<'w> AgentStep<'w> {
    fn find(job_id: &'w str, step: &'w UsesStep, route: Route<'w>) -> Option<Self> {
        Agent::of(&step.uses).map(|agent| AgentStep {
            agent,
            job_id,
            step,
            route,
        })
    }
}

pub fn agent_steps(workflow: &Workflow) -> Vec<AgentStep<'_>> {
    let mut found = Vec::new();
    for (job_id, job) in &workflow.model.jobs {
        let Job::NormalJob(job) = job else {
            continue;
        };
        let steps = route!["jobs", job_id.as_str(), "steps"];
        for (index, step) in job.steps.iter().enumerate() {
            let route = steps.with_key(index);
            match step {
                Step::Uses(step) => found.extend(AgentStep::find(job_id, step, route)),
                Step::Parallel { parallel } => {
                    found.extend(parallel.iter().enumerate().filter_map(
                        |(inner, step)| match step {
                            ParallelStep::Uses(step) => AgentStep::find(
                                job_id,
                                step,
                                route.with_keys([
                                    Component::from("parallel"),
                                    Component::from(inner),
                                ]),
                            ),
                            ParallelStep::Run(_) => None,
                        },
                    ))
                }
                Step::Run(_) | Step::Wait { .. } | Step::WaitAll { .. } | Step::Cancel { .. } => {}
            }
        }
    }
    found
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
    fn trailing_slash_still_points_at_the_root_action() {
        let workflow = workflow(
            "on: issues
jobs:
  agent:
    runs-on: ubuntu-latest
    steps:
      - uses: openai/codex-action/@v1
",
        );
        let agents: Vec<_> = agent_steps(&workflow)
            .iter()
            .map(|step| step.agent)
            .collect();
        assert_eq!(agents, [Agent::Codex]);
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
