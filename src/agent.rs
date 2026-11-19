use github_actions_models::common::Uses;
use github_actions_models::workflow::job::{NormalJob, ParallelStep, RunStep, Step, UsesStep};
use github_actions_models::workflow::{Job, Workflow as WorkflowModel};
use yamlpath::{Component, Route, route};

use crate::workflow::Workflow;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    ClaudeCode,
    ClaudeBase,
    ClaudeBaseBeta,
    Codex,
    GeminiCli,
}

pub(crate) const USES_KEY: &str = "uses";
const TAG_REF_PREFIX: &str = "refs/tags/";
const COMMIT_SHA_LENGTH: usize = 40;

const AGENT_ACTIONS: [(&str, &str, Agent); 5] = [
    ("anthropics/claude-code-action", "", Agent::ClaudeCode),
    (
        "anthropics/claude-code-action",
        "base-action",
        Agent::ClaudeBase,
    ),
    (
        "anthropics/claude-code-base-action",
        "",
        Agent::ClaudeBaseBeta,
    ),
    ("openai/codex-action", "", Agent::Codex),
    ("google-github-actions/run-gemini-cli", "", Agent::GeminiCli),
];

impl Agent {
    fn of(uses: &Uses) -> Option<Agent> {
        AGENT_ACTIONS
            .iter()
            .find(|(action, subpath, _)| is_action(uses, action, subpath))
            .map(|(.., agent)| *agent)
    }
}

pub(crate) fn is_root_action(uses: &Uses, action: &str) -> bool {
    is_action(uses, action, "")
}

fn is_action(uses: &Uses, action: &str, subpath: &str) -> bool {
    let Uses::Repository(repository) = uses else {
        return false;
    };
    action.eq_ignore_ascii_case(repository.slug())
        && repository.subpath().unwrap_or_default().trim_matches('/') == subpath
}

#[derive(Debug)]
pub struct AgentStep<'w> {
    pub agent: Agent,
    pub model: &'w WorkflowModel,
    pub job_id: &'w str,
    pub job: &'w NormalJob,
    pub index: usize,
    pub step: &'w UsesStep,
    pub route: Route<'w>,
    pub major_version: Option<u64>,
}

pub fn agent_steps(workflow: &Workflow) -> Vec<AgentStep<'_>> {
    let mut found = Vec::new();
    for (job_id, job) in &workflow.model.jobs {
        let Job::NormalJob(job) = job else {
            continue;
        };
        for (index, route, step) in job_steps(job_id, job) {
            if let JobStep::Uses(step) = step
                && let Some(agent) = Agent::of(&step.uses)
            {
                found.push(AgentStep {
                    agent,
                    model: &workflow.model,
                    job_id,
                    job,
                    index,
                    step,
                    major_version: major_version(workflow, &step.uses, &route),
                    route,
                });
            }
        }
    }
    found
}

fn major_version(workflow: &Workflow, uses: &Uses, route: &Route) -> Option<u64> {
    let Uses::Repository(repository) = uses else {
        return None;
    };
    let git_ref = repository.git_ref();
    let tag = git_ref.strip_prefix(TAG_REF_PREFIX).unwrap_or(git_ref);
    if let Some(major) = version_number(tag) {
        return Some(major);
    }
    let pinned =
        git_ref.len() == COMMIT_SHA_LENGTH && git_ref.chars().all(|c| c.is_ascii_hexdigit());
    if !pinned {
        return None;
    }
    workflow
        .comment_parts(&route.with_key(USES_KEY))
        .into_iter()
        .find_map(version_number)
}

fn version_number(text: &str) -> Option<u64> {
    let tag = text.rsplit(['@', '=']).next().unwrap_or(text);
    let mut parts = tag.strip_prefix('v')?.split('.');
    let major = parts.next()?;
    let digits = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit());
    (digits(major) && parts.all(digits)).then(|| major.parse().ok())?
}

pub(crate) enum JobStep<'w> {
    Uses(&'w UsesStep),
    Run(&'w RunStep),
}

pub(crate) fn job_steps<'w>(
    job_id: &'w str,
    job: &'w NormalJob,
) -> Vec<(usize, Route<'w>, JobStep<'w>)> {
    let steps = route!["jobs", job_id, "steps"];
    let mut found = Vec::new();
    for (index, step) in job.steps.iter().enumerate() {
        match step {
            Step::Uses(step) => found.push((index, steps.with_key(index), JobStep::Uses(step))),
            Step::Run(step) => found.push((index, steps.with_key(index), JobStep::Run(step))),
            Step::Parallel { parallel } => {
                for (inner, step) in parallel.iter().enumerate() {
                    let route = steps.with_keys([
                        Component::from(index),
                        Component::from("parallel"),
                        Component::from(inner),
                    ]);
                    found.push(match step {
                        ParallelStep::Uses(step) => (index, route, JobStep::Uses(step)),
                        ParallelStep::Run(step) => (index, route, JobStep::Run(step)),
                    });
                }
            }
            Step::Wait { .. } | Step::WaitAll { .. } | Step::Cancel { .. } => {}
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
        workflow.position(&step.route).unwrap().0
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
      - uses: anthropics/claude-code-action/other-action@v1
      - uses: ./.github/actions/claude-code-action
      - uses: docker://openai/codex-action:latest
      - run: npx @google/gemini-cli
",
        );
        assert!(agent_steps(&workflow).is_empty());
    }

    #[test]
    fn the_base_action_is_found_under_both_names() {
        let workflow = workflow(
            "on: issues
jobs:
  base:
    runs-on: ubuntu-latest
    steps:
      - uses: anthropics/claude-code-action/base-action@v1
      - uses: Anthropics/Claude-Code-Base-Action@v1
",
        );
        let agents: Vec<Agent> = agent_steps(&workflow)
            .iter()
            .map(|step| step.agent)
            .collect();
        assert_eq!(agents, [Agent::ClaudeBase, Agent::ClaudeBaseBeta]);
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
