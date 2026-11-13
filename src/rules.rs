use std::fmt;

use github_actions_models::common::{BasePermission, Permission, Permissions};
use github_actions_models::workflow::job::NormalJob;
use yamlpath::Route;

use crate::agent::{AgentStep, agent_steps};
use crate::facts::{
    BroadTool, ExternalEvent, HeadCheckout, OpenTrigger, SecretUse, broad_tool, external_events,
    head_checkout, long_lived_token, never_runs, open_trigger, other_secret, secret_env,
};
use crate::workflow::Workflow;

const COUNTED_WRITE_SCOPES: [&str; 5] =
    ["contents", "actions", "packages", "deployments", "id-token"];
pub const MAX_TOTAL: u8 = 3;
const WARNING_TOTAL: u8 = 2;
pub const HALVES_PER_POINT: u8 = 2;
const ERROR_HALVES: u8 = MAX_TOTAL * HALVES_PER_POINT;
const WARNING_HALVES: u8 = WARNING_TOTAL * HALVES_PER_POINT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    UnrestrictedTrigger,
    BroadTools,
    PrHeadCheckout,
    LongLivedToken,
    OtherSecrets,
}

impl Rule {
    pub fn id(self) -> &'static str {
        match self {
            Rule::UnrestrictedTrigger => "unrestricted-trigger",
            Rule::BroadTools => "broad-tools",
            Rule::PrHeadCheckout => "pr-head-checkout",
            Rule::LongLivedToken => "long-lived-token",
            Rule::OtherSecrets => "other-secrets",
        }
    }

    pub fn summary(self) -> &'static str {
        match self {
            Rule::UnrestrictedTrigger => "can be started without write access",
            Rule::BroadTools => "can run unrestricted tools",
            Rule::PrHeadCheckout => "loads settings from a checked-out PR head",
            Rule::LongLivedToken => "gets a long-lived GitHub token",
            Rule::OtherSecrets => "can read other secrets",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    Half,
    Full,
}

impl Weight {
    pub fn halves(self) -> u8 {
        match self {
            Weight::Half => 1,
            Weight::Full => HALVES_PER_POINT,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct AxisScore {
    pub weight: Weight,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Warning,
    Error,
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Level::Warning => "warning",
            Level::Error => "error",
        })
    }
}

#[derive(Debug)]
pub struct Hit<'w> {
    pub rule: Rule,
    pub route: Route<'w>,
}

#[derive(Debug)]
pub struct Finding<'a, 'w> {
    pub level: Level,
    pub hit: &'a Hit<'w>,
    pub assessment: &'a Assessment<'w>,
}

#[derive(Debug)]
pub struct Assessment<'w> {
    pub step: AgentStep<'w>,
    pub who_can_trigger: Option<AxisScore>,
    pub allowed_tools: Option<AxisScore>,
    pub permissions_and_secrets: Option<AxisScore>,
    pub hits: Vec<Hit<'w>>,
}

impl<'w> Assessment<'w> {
    pub fn total_halves(&self) -> u8 {
        [
            &self.who_can_trigger,
            &self.allowed_tools,
            &self.permissions_and_secrets,
        ]
        .into_iter()
        .flatten()
        .map(|score| score.weight.halves())
        .sum()
    }

    pub fn level(&self) -> Option<Level> {
        match self.total_halves() {
            ERROR_HALVES.. => Some(Level::Error),
            WARNING_HALVES.. => Some(Level::Warning),
            _ => None,
        }
    }

    pub fn findings(&self) -> Vec<Finding<'_, 'w>> {
        let Some(level) = self.level() else {
            return Vec::new();
        };
        self.hits
            .iter()
            .map(|hit| Finding {
                level,
                hit,
                assessment: self,
            })
            .collect()
    }
}

pub fn assess(workflow: &Workflow) -> Vec<Assessment<'_>> {
    let events = external_events(&workflow.model.on.events);
    agent_steps(workflow)
        .into_iter()
        .filter(|step| !never_runs(step))
        .map(|step| assess_step(workflow, step, &events))
        .collect()
}

fn assess_step<'w>(
    workflow: &'w Workflow,
    step: AgentStep<'w>,
    events: &[ExternalEvent],
) -> Assessment<'w> {
    let mut hits = Vec::new();
    let mut who_can_trigger = None;
    if !events.is_empty() {
        who_can_trigger = Some(match open_trigger(&step, events) {
            Some(OpenTrigger {
                setting,
                route,
                events: open_events,
            }) => {
                hits.push(Hit {
                    rule: Rule::UnrestrictedTrigger,
                    route,
                });
                AxisScore {
                    weight: Weight::Full,
                    reason: format!("{setting}, on {}", event_names(&open_events)),
                }
            }
            None => AxisScore {
                weight: Weight::Half,
                reason: format!(
                    "on {}, text from outside contributors can reach the agent",
                    event_names(events)
                ),
            },
        });
    }
    let mut tool_reasons = Vec::new();
    if let Some(BroadTool { setting, route }) = broad_tool(&step) {
        hits.push(Hit {
            rule: Rule::BroadTools,
            route,
        });
        tool_reasons.push(setting);
    }
    if let Some(HeadCheckout { setting, route }) = head_checkout(workflow, &step, events) {
        hits.push(Hit {
            rule: Rule::PrHeadCheckout,
            route,
        });
        tool_reasons.push(setting);
    }
    let allowed_tools = (!tool_reasons.is_empty()).then(|| AxisScore {
        weight: Weight::Full,
        reason: tool_reasons.join("; "),
    });
    let env = secret_env(workflow, &step);
    let mut secret_reasons = Vec::new();
    for (rule, found) in [
        (Rule::LongLivedToken, long_lived_token(&step, &env)),
        (Rule::OtherSecrets, other_secret(&step, &env)),
    ] {
        if let Some(SecretUse { setting, route }) = found {
            hits.push(Hit { rule, route });
            secret_reasons.push(setting);
        }
    }
    secret_reasons.extend(write_permission(workflow, step.job));
    let permissions_and_secrets = (!secret_reasons.is_empty()).then(|| AxisScore {
        weight: Weight::Full,
        reason: secret_reasons.join("; "),
    });
    Assessment {
        step,
        who_can_trigger,
        allowed_tools,
        permissions_and_secrets,
        hits,
    }
}

fn event_names(events: &[ExternalEvent]) -> String {
    events
        .iter()
        .map(|event| event.name())
        .collect::<Vec<_>>()
        .join(", ")
}

fn write_permission(workflow: &Workflow, job: &NormalJob) -> Option<String> {
    let permissions = match &job.permissions {
        Permissions::Base(BasePermission::Default) => &workflow.model.permissions,
        set_on_job => set_on_job,
    };
    match permissions {
        Permissions::Base(BasePermission::Default) => {
            Some("no permissions are set, so GITHUB_TOKEN gets the repository default".to_string())
        }
        Permissions::Base(BasePermission::WriteAll) => Some("permissions: write-all".to_string()),
        Permissions::Base(BasePermission::ReadAll) => None,
        Permissions::Explicit(scopes) => {
            let writes: Vec<&str> = COUNTED_WRITE_SCOPES
                .into_iter()
                .filter(|scope| scopes.get(*scope) == Some(&Permission::Write))
                .collect();
            (!writes.is_empty()).then(|| format!("{} write", writes.join(", ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    struct Scores {
        trigger: Option<AxisScore>,
        tools: Option<AxisScore>,
        permissions: Option<AxisScore>,
        total: u8,
        level: Option<Level>,
    }

    fn assessed(source: &str) -> Scores {
        let workflow = Workflow::parse(Path::new("test.yml"), source.to_string()).unwrap();
        let mut assessments = assess(&workflow);
        assert_eq!(assessments.len(), 1);
        let assessment = assessments.remove(0);
        Scores {
            total: assessment.total_halves(),
            level: assessment.level(),
            trigger: assessment.who_can_trigger,
            tools: assessment.allowed_tools,
            permissions: assessment.permissions_and_secrets,
        }
    }

    fn score(weight: Weight, reason: &str) -> Option<AxisScore> {
        Some(AxisScore {
            weight,
            reason: reason.to_string(),
        })
    }

    #[test]
    fn wildcard_users_on_a_comment_without_permissions_is_a_warning() {
        let Scores {
            trigger,
            permissions,
            total,
            level,
            ..
        } = assessed(
            "on: [issue_comment, push]
jobs:
  claude:
    runs-on: ubuntu-latest
    steps:
      - uses: anthropics/claude-code-action@v1
        with:
          github_token: ${{ secrets.GITHUB_TOKEN }}
          allowed_non_write_users: '*'
",
        );
        assert_eq!(
            trigger,
            score(
                Weight::Full,
                "allowed_non_write_users: '*', on issue_comment"
            )
        );
        assert_eq!(
            permissions,
            score(
                Weight::Full,
                "no permissions are set, so GITHUB_TOKEN gets the repository default"
            )
        );
        assert_eq!(total, 4);
        assert_eq!(level, Some(Level::Warning));
    }

    #[test]
    fn restricted_trigger_counts_half_and_stays_below_a_warning() {
        let Scores {
            trigger,
            total,
            level,
            ..
        } = assessed(
            "on: issues
jobs:
  claude:
    runs-on: ubuntu-latest
    steps:
      - uses: anthropics/claude-code-action@v1
",
        );
        assert_eq!(
            trigger,
            score(
                Weight::Half,
                "on issues, text from outside contributors can reach the agent"
            )
        );
        assert_eq!(total, 3);
        assert_eq!(level, None);
    }

    #[test]
    fn no_external_trigger_means_no_one_outside_can_trigger() {
        let Scores {
            trigger,
            total,
            level,
            ..
        } = assessed(
            "on:
  push:
  pull_request:
  workflow_dispatch:
jobs:
  codex:
    runs-on: ubuntu-latest
    steps:
      - uses: openai/codex-action@v1
        with:
          allow-users: '*'
",
        );
        assert_eq!(trigger, None);
        assert_eq!(total, 2);
        assert_eq!(level, None);
    }

    #[test]
    fn all_three_axes_together_are_an_error_reported_once_per_rule() {
        let workflow = Workflow::parse(
            Path::new("test.yml"),
            "on: issue_comment
jobs:
  codex:
    runs-on: ubuntu-latest
    steps:
      - uses: openai/codex-action@v1
        with:
          allow-users: '*'
          sandbox: danger-full-access
"
            .to_string(),
        )
        .unwrap();
        let assessments = assess(&workflow);
        let findings: Vec<_> = assessments[0]
            .findings()
            .iter()
            .map(|finding| (finding.level, finding.hit.rule))
            .collect();
        assert_eq!(
            findings,
            [
                (Level::Error, Rule::UnrestrictedTrigger),
                (Level::Error, Rule::BroadTools)
            ]
        );
    }

    #[test]
    fn broad_tools_fill_the_allowed_tools_axis_with_their_setting() {
        let claude = assessed(
            "on: push
jobs:
  claude:
    runs-on: ubuntu-latest
    steps:
      - uses: anthropics/claude-code-action@v1
        with:
          claude_args: --allowedTools Bash
",
        );
        assert_eq!(
            claude.tools,
            score(
                Weight::Full,
                "claude_args allows `Bash` through --allowedTools"
            )
        );
        assert_eq!(claude.total, 4);

        let gemini = assessed(
            "on: push
permissions:
  contents: read
jobs:
  gemini:
    runs-on: ubuntu-latest
    steps:
      - uses: google-github-actions/run-gemini-cli@v0
",
        );
        assert_eq!(
            gemini.tools,
            score(
                Weight::Full,
                "no `settings`, so tools.core is unset and every built-in tool runs under --yolo"
            )
        );
        assert_eq!(gemini.level, None);
    }

    #[test]
    fn a_head_checkout_shares_the_allowed_tools_axis_with_broad_tools() {
        let workflow = Workflow::parse(
            Path::new("test.yml"),
            "on: workflow_run
permissions:
  contents: write
jobs:
  fix:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
        with:
          ref: ${{ github.event.workflow_run.head_sha }}
      - uses: anthropics/claude-code-action@v1
        with:
          claude_args: --allowedTools Bash
"
            .to_string(),
        )
        .unwrap();
        let assessments = assess(&workflow);
        let assessment = &assessments[0];
        let rules: Vec<_> = assessment
            .findings()
            .iter()
            .map(|finding| (finding.level, finding.hit.rule))
            .collect();
        assert_eq!(
            rules,
            [
                (Level::Warning, Rule::BroadTools),
                (Level::Warning, Rule::PrHeadCheckout)
            ]
        );
        assert_eq!(
            assessment.allowed_tools,
            score(
                Weight::Full,
                "claude_args allows `Bash` through --allowedTools; actions/checkout@v6 checks out the PR head with `ref: ${{ github.event.workflow_run.head_sha }}`, and on workflow_run claude-code-action keeps the PR's .claude/ and CLAUDE.md"
            )
        );
        assert_eq!(assessment.total_halves(), 5);
    }

    #[test]
    fn tokens_secrets_and_write_access_share_the_permissions_axis() {
        let workflow = Workflow::parse(
            Path::new("test.yml"),
            "on: issue_comment
permissions:
  contents: write
jobs:
  codex:
    runs-on: ubuntu-latest
    env:
      GH_TOKEN: ${{ secrets.ORG_PAT }}
      NPM_TOKEN: ${{ secrets.NPM_TOKEN }}
    steps:
      - uses: openai/codex-action@v1
        with:
          allow-users: '*'
"
            .to_string(),
        )
        .unwrap();
        let assessments = assess(&workflow);
        let assessment = &assessments[0];
        let rules: Vec<_> = assessment
            .findings()
            .iter()
            .map(|finding| finding.hit.rule)
            .collect();
        assert_eq!(
            rules,
            [
                Rule::UnrestrictedTrigger,
                Rule::LongLivedToken,
                Rule::OtherSecrets
            ]
        );
        assert_eq!(
            assessment.permissions_and_secrets,
            score(
                Weight::Full,
                "env GH_TOKEN uses secrets.ORG_PAT, a stored token rather than GITHUB_TOKEN or an app token; env NPM_TOKEN passes secrets.NPM_TOKEN to the agent; contents write"
            )
        );
        assert_eq!(assessment.level(), Some(Level::Warning));
    }

    #[test]
    fn a_step_that_never_runs_is_not_assessed() {
        let workflow = Workflow::parse(
            Path::new("test.yml"),
            "on: issues
jobs:
  gemini:
    runs-on: ubuntu-latest
    if: false
    steps:
      - uses: google-github-actions/run-gemini-cli@v0
"
            .to_string(),
        )
        .unwrap();
        assert!(assess(&workflow).is_empty());
    }

    #[test]
    fn job_permissions_override_the_workflow_and_only_counted_writes_count() {
        let Scores {
            permissions: read_only_job,
            ..
        } = assessed(
            "on: issues
permissions: write-all
jobs:
  claude:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      issues: write
      pull-requests: write
    steps:
      - uses: anthropics/claude-code-action@v1
",
        );
        assert_eq!(read_only_job, None);

        let Scores {
            permissions: inherited,
            ..
        } = assessed(
            "on: issues
permissions:
  contents: write
  id-token: write
jobs:
  claude:
    runs-on: ubuntu-latest
    steps:
      - uses: anthropics/claude-code-action@v1
",
        );
        assert_eq!(inherited, score(Weight::Full, "contents, id-token write"));

        let Scores {
            permissions: write_all,
            ..
        } = assessed(
            "on: issues
jobs:
  claude:
    runs-on: ubuntu-latest
    permissions: write-all
    steps:
      - uses: anthropics/claude-code-action@v1
",
        );
        assert_eq!(write_all, score(Weight::Full, "permissions: write-all"));

        let Scores {
            permissions: read_all,
            ..
        } = assessed(
            "on: issues
permissions: read-all
jobs:
  claude:
    runs-on: ubuntu-latest
    steps:
      - uses: anthropics/claude-code-action@v1
",
        );
        assert_eq!(read_all, None);
    }

    #[test]
    fn levels_follow_the_total_of_the_axes() {
        let workflow = Workflow::parse(
            Path::new("test.yml"),
            "on: push
jobs:
  claude:
    runs-on: ubuntu-latest
    steps:
      - uses: anthropics/claude-code-action@v1
"
            .to_string(),
        )
        .unwrap();
        let axis = |weight: Option<Weight>| {
            weight.map(|weight| AxisScore {
                weight,
                reason: String::new(),
            })
        };
        let full = Some(Weight::Full);
        let half = Some(Weight::Half);
        let cases = [
            ([full, full, full], Some(Level::Error)),
            ([half, full, full], Some(Level::Warning)),
            ([full, full, None], Some(Level::Warning)),
            ([None, full, full], Some(Level::Warning)),
            ([half, full, None], None),
            ([full, None, None], None),
        ];
        for ([trigger, tools, permissions], expected) in cases {
            let assessment = Assessment {
                step: agent_steps(&workflow).remove(0),
                who_can_trigger: axis(trigger),
                allowed_tools: axis(tools),
                permissions_and_secrets: axis(permissions),
                hits: Vec::new(),
            };
            assert_eq!(
                assessment.level(),
                expected,
                "{trigger:?} {tools:?} {permissions:?}"
            );
        }
    }
}
