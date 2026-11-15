use std::fmt;

use github_actions_models::common::{BasePermission, Permission, Permissions};
use github_actions_models::workflow::job::NormalJob;
use yamlpath::Route;

use crate::agent::{AgentStep, agent_steps};
use crate::facts::{
    AgentCode, BroadTool, ExternalEvent, HeadCheckout, OpenTrigger, OutputInjection, SecretUse,
    UpstreamOrigin, Upstreams, agent_code_with_secrets, agent_secrets, broad_tool,
    condition_origin, external_events, head_checkout, long_lived_token, never_runs, open_trigger,
    other_secret, output_injection, outside_events, outside_text_cannot_reach, secret_env,
};
use crate::workflow::Workflow;

const COUNTED_WRITE_SCOPES: [&str; 5] =
    ["contents", "actions", "packages", "deployments", "id-token"];
pub const MAX_TOTAL: u8 = 3;
const WARNING_TOTAL: u8 = 2;
const IGNORE_DIRECTIVE: &str = "sekimori: ignore";
const USES_KEY: &str = "uses";
pub const HALVES_PER_POINT: u8 = 2;
const ERROR_HALVES: u8 = MAX_TOTAL * HALVES_PER_POINT;
const WARNING_HALVES: u8 = WARNING_TOTAL * HALVES_PER_POINT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    UnrestrictedTrigger,
    BroadTools,
    PrHeadCheckout,
    AgentOutputInjection,
    LongLivedToken,
    OtherSecrets,
    AgentCodeWithSecrets,
}

impl Rule {
    pub const ALL: [Rule; 7] = [
        Rule::UnrestrictedTrigger,
        Rule::BroadTools,
        Rule::LongLivedToken,
        Rule::PrHeadCheckout,
        Rule::OtherSecrets,
        Rule::AgentOutputInjection,
        Rule::AgentCodeWithSecrets,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Rule::UnrestrictedTrigger => "unrestricted-trigger",
            Rule::BroadTools => "broad-tools",
            Rule::PrHeadCheckout => "pr-head-checkout",
            Rule::AgentOutputInjection => "agent-output-injection",
            Rule::AgentCodeWithSecrets => "agent-code-with-secrets",
            Rule::LongLivedToken => "long-lived-token",
            Rule::OtherSecrets => "other-secrets",
        }
    }

    pub fn summary(self) -> &'static str {
        match self {
            Rule::UnrestrictedTrigger => "can be started without write access",
            Rule::BroadTools => "can run unrestricted tools",
            Rule::PrHeadCheckout => "loads settings from a checked-out PR head",
            Rule::AgentOutputInjection => "has its output run as code by a later step",
            Rule::AgentCodeWithSecrets => "can edit code that a later step runs with secrets",
            Rule::LongLivedToken => "gets a long-lived GitHub token",
            Rule::OtherSecrets => "can read other secrets",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Rule::UnrestrictedTrigger => {
                "The agent runs on an event that outside contributors can cause, and the workflow lets anyone start it: a wildcard user or bot allow-list, or no `if:` that checks who triggered it (a trusted actor or author association, or the label that was just added). On `workflow_run`, the upstream run also counts as checked when an `if:` limits its event, or when the upstream workflow, found by its `name:` among the files checked together in the same directory, has only triggers that outside contributors cannot cause, such as `workflow_dispatch`, `schedule`, `release` or `push`. Gemini CLI given no `prompt` input runs only a fixed prompt, so it does not fill this axis when it runs only on `issues`, `issue_comment`, `discussion` or `discussion_comment` and every earlier step in the job is an `actions/checkout` of this repository at a fixed ref. Fills the who-can-trigger axis."
            }
            Rule::BroadTools => {
                "The agent may run any shell command or tool: an unscoped Bash allow rule, a bypassed permission prompt, a sandbox with full access, or Gemini CLI without a tools.core allow-list under --yolo. Fills the allowed-tools axis."
            }
            Rule::LongLivedToken => {
                "The GitHub token handed to the agent comes from a stored secret, such as a personal access token, instead of GITHUB_TOKEN or a short-lived app token. Fills the permissions/secrets axis."
            }
            Rule::PrHeadCheckout => {
                "A step before the agent checks out the pull request head into the workspace on a privileged event, and the agent then loads that head's settings, hooks or MCP servers. Fills the allowed-tools axis."
            }
            Rule::AgentOutputInjection => {
                "A later step in the job expands text the model wrote into code with `${{ }}`: Claude Code's structured_output, Codex's final-message, or Gemini CLI's summary or error in a `run` script or an actions/github-script `script`. Whoever steers the agent can then run commands in that step, and secrets that reach that step count on the permissions/secrets axis. Fills the allowed-tools axis."
            }
            Rule::AgentCodeWithSecrets => {
                "The agent can edit files in the workspace, and a later step in the job runs project code with a secret other than GITHUB_TOKEN: a `./` script or a relative path to a script file, a script given to a shell or interpreter, or a known build or test command such as `npm test`, `make` or `pytest`. Whoever steers the agent can change that code and read the secret. Fills the permissions/secrets axis."
            }
            Rule::OtherSecrets => {
                "A secret other than the agent's own credentials and the GitHub token reaches the agent through its inputs or environment. Fills the permissions/secrets axis."
            }
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
    pub ignored: Vec<Rule>,
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
            .filter(|hit| !self.ignored.contains(&hit.rule))
            .map(|hit| Finding {
                level,
                hit,
                assessment: self,
            })
            .collect()
    }
}

pub fn assess<'w>(workflow: &'w Workflow, upstreams: &Upstreams) -> Vec<Assessment<'w>> {
    let events = external_events(&workflow.model.on.events);
    let from_files = upstreams.origin_of(workflow);
    agent_steps(workflow)
        .into_iter()
        .filter(|step| !never_runs(step))
        .map(|step| {
            let upstream = if events.contains(&ExternalEvent::WorkflowRun) {
                condition_origin(&step).min(from_files)
            } else {
                UpstreamOrigin::Anyone
            };
            let events = outside_events(&events, upstream);
            assess_step(workflow, step, &events, upstream)
        })
        .collect()
}

fn assess_step<'w>(
    workflow: &'w Workflow,
    step: AgentStep<'w>,
    events: &[ExternalEvent],
    upstream: UpstreamOrigin,
) -> Assessment<'w> {
    let mut hits = Vec::new();
    let mut who_can_trigger = None;
    if !events.is_empty() && !outside_text_cannot_reach(&step, events) {
        who_can_trigger = Some(match open_trigger(&step, events, upstream) {
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
    let injection = output_injection(workflow, &step);
    if let Some(OutputInjection { setting, route, .. }) = &injection {
        hits.push(Hit {
            rule: Rule::AgentOutputInjection,
            route: route.clone(),
        });
        tool_reasons.push(setting.clone());
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
    if let Some(AgentCode { setting, route }) =
        agent_code_with_secrets(workflow, &step, &agent_secrets(&step, &env))
    {
        hits.push(Hit {
            rule: Rule::AgentCodeWithSecrets,
            route,
        });
        secret_reasons.push(setting);
    }
    if let Some(secret) = injection.and_then(|injection| injection.secret) {
        secret_reasons.push(format!(
            "{secret} reaches a later step that runs the agent's output"
        ));
    }
    secret_reasons.extend(write_permission(workflow, step.job));
    let permissions_and_secrets = (!secret_reasons.is_empty()).then(|| AxisScore {
        weight: Weight::Full,
        reason: secret_reasons.join("; "),
    });
    let ignored = if hits.is_empty() {
        Vec::new()
    } else {
        let directives: Vec<Option<Vec<&str>>> = workflow
            .comments(&step.route.with_key(USES_KEY))
            .into_iter()
            .flat_map(ignore_directives)
            .collect();
        Rule::ALL
            .into_iter()
            .filter(|rule| {
                directives.iter().any(|directive| {
                    directive
                        .as_ref()
                        .is_none_or(|ids| ids.contains(&rule.id()))
                })
            })
            .collect()
    };
    Assessment {
        step,
        who_can_trigger,
        allowed_tools,
        permissions_and_secrets,
        hits,
        ignored,
    }
}

fn ignore_directives(comment: &str) -> Vec<Option<Vec<&str>>> {
    comment
        .split('#')
        .filter_map(|part| {
            let directive = part.trim().strip_prefix(IGNORE_DIRECTIVE)?;
            match directive.strip_prefix('[') {
                Some(list) => {
                    let (ids, _) = list.split_once(']')?;
                    Some(Some(ids.split(',').map(str::trim).collect()))
                }
                None => directive.is_empty().then_some(None),
            }
        })
        .collect()
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
        let mut assessments = assess(&workflow, &Upstreams::default());
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
    fn gemini_without_a_prompt_input_gets_no_outside_text() {
        let gemini = |with: &str| {
            assessed(&format!(
                "on: [issues, discussion]
jobs:
  gemini:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
      - uses: google-github-actions/run-gemini-cli@v0
{with}"
            ))
        };
        let open = score(
            Weight::Full,
            "no `if:` on the job or step, on issues, discussion",
        );

        let missing = gemini("");
        assert_eq!(missing.trigger, None);
        assert_eq!(missing.total, 4);
        assert_eq!(missing.level, Some(Level::Warning));

        for with in [
            "        with:
          custom_prompt: ${{ github.event.issue.body }}
",
            "        with:
          prompt: ' '
",
            "        with:
          prompt:
",
        ] {
            assert_eq!(gemini(with).trigger, None, "{with}");
        }

        let given = gemini(
            "        with:
          PROMPT: ${{ github.event.issue.body }}
",
        );
        assert_eq!(given.trigger, open);
        assert_eq!(given.level, Some(Level::Error));

        let unreadable = gemini(
            "        with: ${{ fromJSON(vars.GEMINI_INPUTS) }}
",
        );
        assert_eq!(unreadable.trigger, open);
    }

    #[test]
    fn gemini_without_a_prompt_counts_unless_only_the_default_branch_is_checked_out() {
        let trigger = |on: &str, steps: &str| {
            assessed(&format!(
                "on: {on}
jobs:
  gemini:
    runs-on: ubuntu-latest
    steps:
{steps}"
            ))
            .trigger
        };
        let gemini = "      - uses: google-github-actions/run-gemini-cli@v0
        with:
          settings: '{\"tools\": {\"core\": [\"read_file\"]}}'
";
        let open = |on: &str| {
            score(
                Weight::Full,
                &format!("no `if:` on the job or step, on {on}"),
            )
        };

        for before in [
            "      - uses: actions/checkout@v6
        with:
          path: pr
          ref: refs/pull/7/head
",
            "      - uses: actions/checkout@v6
        with:
          ref: ${{ steps.pr.outputs.sha }}
",
            "      - uses: actions/checkout@v6
        with: ${{ fromJSON(vars.CHECKOUT) }}
",
            "      - uses: actions/checkout@v6
        with:
          repository: someone/prompts
",
            "      - run: gh pr checkout ${{ github.event.issue.number }}
",
            "      - uses: ./.github/actions/checkout-pr
",
        ] {
            assert_eq!(
                trigger("issue_comment", &format!("{before}{gemini}")),
                open("issue_comment"),
                "{before}"
            );
        }
        assert_eq!(
            trigger(
                "issue_comment",
                &format!(
                    "      - parallel:
          - run: gh pr checkout ${{{{ github.event.issue.number }}}}
    {}",
                    gemini.replace("\n      ", "\n          ")
                )
            ),
            open("issue_comment")
        );
        assert_eq!(
            trigger(
                "pull_request_review",
                &format!("      - uses: actions/checkout@v6\n{gemini}")
            ),
            open("pull_request_review")
        );

        for before in [
            "",
            "      - uses: actions/checkout@v6
        with:
          fetch-depth: 0
",
            "      - uses: actions/checkout@v6
        with:
          path: docs
          ref: gh-pages
          repository: ${{ github.repository }}
",
        ] {
            assert_eq!(
                trigger("issue_comment", &format!("{before}{gemini}")),
                None,
                "{before}"
            );
        }
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
        let assessments = assess(&workflow, &Upstreams::default());
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
        let assessments = assess(&workflow, &Upstreams::default());
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
    fn model_output_run_as_code_fills_the_allowed_tools_axis_even_with_scoped_tools() {
        let workflow = Workflow::parse(
            Path::new("test.yml"),
            "on: issue_comment
permissions:
  contents: read
jobs:
  triage:
    runs-on: ubuntu-latest
    steps:
      - id: gemini
        uses: google-github-actions/run-gemini-cli@v0
        with:
          prompt: ${{ github.event.comment.body }}
          settings: '{\"tools\": {\"core\": [\"read_file\"]}}'
      - run: echo \"${{ steps.gemini.outputs.summary }}\" >> \"$GITHUB_STEP_SUMMARY\"
        env:
          DEPLOY_KEY: ${{ secrets.DEPLOY_KEY }}
"
            .to_string(),
        )
        .unwrap();
        let assessments = assess(&workflow, &Upstreams::default());
        let assessment = &assessments[0];
        let rules: Vec<_> = assessment
            .findings()
            .iter()
            .map(|finding| (finding.level, finding.hit.rule))
            .collect();
        assert_eq!(
            rules,
            [
                (Level::Error, Rule::UnrestrictedTrigger),
                (Level::Error, Rule::AgentOutputInjection)
            ]
        );
        assert_eq!(
            assessment.allowed_tools,
            score(
                Weight::Full,
                "a later step expands `steps.gemini.outputs.summary` into its `run`"
            )
        );
        assert_eq!(
            assessment.permissions_and_secrets,
            score(
                Weight::Full,
                "secrets.DEPLOY_KEY reaches a later step that runs the agent's output"
            )
        );
    }

    #[test]
    fn project_code_run_with_secrets_after_the_agent_fills_the_permissions_axis() {
        let workflow = Workflow::parse(
            Path::new("test.yml"),
            "on: issue_comment
permissions:
  contents: read
jobs:
  fix:
    runs-on: ubuntu-latest
    steps:
      - uses: anthropics/claude-code-action@v1
        with:
          allowed_non_write_users: '*'
          github_token: ${{ github.token }}
      - run: npm test
        env:
          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}
"
            .to_string(),
        )
        .unwrap();
        let assessments = assess(&workflow, &Upstreams::default());
        let assessment = &assessments[0];
        let rules: Vec<_> = assessment
            .findings()
            .iter()
            .map(|finding| (finding.level, finding.hit.rule))
            .collect();
        assert_eq!(
            rules,
            [
                (Level::Warning, Rule::UnrestrictedTrigger),
                (Level::Warning, Rule::AgentCodeWithSecrets)
            ]
        );
        assert_eq!(
            assessment.permissions_and_secrets,
            score(
                Weight::Full,
                "`npm test` in a later step runs files the agent can edit, with secrets.NPM_TOKEN"
            )
        );
    }

    #[test]
    fn a_secret_that_already_reaches_the_agent_is_not_reported_again_for_later_code() {
        let workflow = Workflow::parse(
            Path::new("test.yml"),
            "on: issue_comment
permissions:
  contents: read
jobs:
  fix:
    runs-on: ubuntu-latest
    env:
      NPM_TOKEN: ${{ secrets.NPM_TOKEN }}
    steps:
      - uses: anthropics/claude-code-action@v1
      - run: npm test
"
            .to_string(),
        )
        .unwrap();
        let assessments = assess(&workflow, &Upstreams::default());
        let rules: Vec<_> = assessments[0].hits.iter().map(|hit| hit.rule).collect();
        assert_eq!(rules, [Rule::OtherSecrets]);
        let another_secret = Workflow::parse(
            Path::new("test.yml"),
            "on: issue_comment
jobs:
  fix:
    runs-on: ubuntu-latest
    env:
      NPM_TOKEN: ${{ secrets.NPM_TOKEN }}
    steps:
      - uses: anthropics/claude-code-action@v1
      - run: npm test
      - run: ./deploy.sh --key ${{ secrets.DEPLOY_KEY }}
"
            .to_string(),
        )
        .unwrap();
        let assessments = assess(&another_secret, &Upstreams::default());
        assert!(
            assessments[0]
                .permissions_and_secrets
                .as_ref()
                .unwrap()
                .reason
                .contains("`./deploy.sh` in a later step runs files the agent can edit, with secrets.DEPLOY_KEY")
        );
        let every_secret = Workflow::parse(
            Path::new("test.yml"),
            "on: issue_comment
jobs:
  fix:
    runs-on: ubuntu-latest
    steps:
      - uses: anthropics/claude-code-action@v1
        env:
          ALL: ${{ toJSON(secrets) }}
      - run: npm test
        env:
          TOKEN: ${{ secrets.TOKEN }}
"
            .to_string(),
        )
        .unwrap();
        let assessments = assess(&every_secret, &Upstreams::default());
        let rules: Vec<_> = assessments[0].hits.iter().map(|hit| hit.rule).collect();
        assert_eq!(rules, [Rule::OtherSecrets]);
        let two_secrets = Workflow::parse(
            Path::new("test.yml"),
            "on: issue_comment
jobs:
  fix:
    runs-on: ubuntu-latest
    env:
      A: ${{ secrets.A }}
      B: ${{ secrets.B }}
    steps:
      - uses: anthropics/claude-code-action@v1
      - run: npm test
"
            .to_string(),
        )
        .unwrap();
        let assessments = assess(&two_secrets, &Upstreams::default());
        let rules: Vec<_> = assessments[0].hits.iter().map(|hit| hit.rule).collect();
        assert_eq!(rules, [Rule::OtherSecrets]);
    }

    #[test]
    fn ignore_comments_drop_the_named_findings_without_changing_the_level() {
        let rules_with = |agent_comment: &str, args_comment: &str| {
            let workflow = Workflow::parse(
                Path::new("test.yml"),
                format!(
                    "on: issue_comment
jobs:
  claude:
    runs-on: ubuntu-latest
    steps:
      - uses: anthropics/claude-code-action@v1 {agent_comment}
        with:
          github_token: ${{{{ github.token }}}}
          allowed_non_write_users: '*'
          claude_args: --allowedTools Bash {args_comment}
"
                ),
            )
            .unwrap();
            let assessments = assess(&workflow, &Upstreams::default());
            assessments[0]
                .findings()
                .iter()
                .map(|finding| (finding.level, finding.hit.rule))
                .collect::<Vec<_>>()
        };
        let both = vec![
            (Level::Error, Rule::UnrestrictedTrigger),
            (Level::Error, Rule::BroadTools),
        ];
        assert_eq!(rules_with("", ""), both);
        assert_eq!(rules_with("# sekimori: ignore", ""), []);
        assert_eq!(
            rules_with("# sekimori: ignore[broad-tools] reviewed", ""),
            [(Level::Error, Rule::UnrestrictedTrigger)]
        );
        assert_eq!(
            rules_with("# sekimori: ignore[unrestricted-trigger, broad-tools]", ""),
            []
        );
        assert_eq!(rules_with("", "# sekimori: ignore[broad-tools]"), both);
        assert_eq!(
            rules_with("# v1.0.5 # sekimori: ignore[broad-tools]", ""),
            [(Level::Error, Rule::UnrestrictedTrigger)]
        );
        for comment in [
            "# sekimori: ignore broad-tools",
            "# sekimori: ignore reviewed",
            "# sekimori: ignored",
            "# sekimori: ignore[pr-head]",
            "# sekimori: ignore [broad-tools]",
            "# sekimori: ignore[broad-tools",
            "# TODO remove sekimori: ignore once tools are scoped",
        ] {
            assert_eq!(rules_with(comment, ""), both, "{comment}");
        }
    }

    #[test]
    fn the_ignore_comment_is_read_on_the_uses_line_wherever_the_step_starts() {
        for source in [
            "on: issue_comment
jobs:
  claude:
    runs-on: ubuntu-latest
    steps:
      - name: Run Claude
        uses: anthropics/claude-code-action@v1 # sekimori: ignore
        with:
          claude_args: --allowedTools Bash
",
            "on: issue_comment
jobs:
  claude:
    runs-on: ubuntu-latest
    steps:
      - parallel:
          - run: true
          - uses: anthropics/claude-code-action@v1 # sekimori: ignore
            with:
              claude_args: --allowedTools Bash
",
        ] {
            let workflow = Workflow::parse(Path::new("test.yml"), source.to_string()).unwrap();
            let assessments = assess(&workflow, &Upstreams::default());
            assert!(!assessments[0].hits.is_empty(), "{source}");
            assert!(assessments[0].findings().is_empty(), "{source}");
        }
    }

    #[test]
    fn the_base_action_checks_no_actor_passes_env_tokens_and_loads_the_checked_out_settings() {
        let rules_of = |source: &str| {
            let workflow = Workflow::parse(Path::new("test.yml"), source.to_string()).unwrap();
            let assessments = assess(&workflow, &Upstreams::default());
            assessments[0]
                .findings()
                .iter()
                .map(|finding| (finding.level, finding.hit.rule))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            rules_of(
                "on: issue_comment
permissions:
  contents: read
jobs:
  claude:
    runs-on: ubuntu-latest
    steps:
      - uses: anthropics/claude-code-base-action@beta
        with:
          prompt: ${{ github.event.comment.body }}
          allowed_tools: Bash
          claude_env: |
            GH_TOKEN: ${{ secrets.BOT_PAT }}
"
            ),
            [
                (Level::Error, Rule::UnrestrictedTrigger),
                (Level::Error, Rule::BroadTools),
                (Level::Error, Rule::LongLivedToken),
            ]
        );
        assert_eq!(
            rules_of(
                "on: issue_comment
jobs:
  claude:
    if: github.event.comment.author_association == 'OWNER'
    runs-on: ubuntu-latest
    steps:
      - uses: anthropics/claude-code-action/base-action@v1
        with:
          prompt: ${{ github.event.comment.body }}
        env:
          GH_TOKEN: ${{ github.token }}
"
            ),
            []
        );
        assert_eq!(
            rules_of(
                "on: pull_request_target
jobs:
  claude:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
        with:
          ref: ${{ github.event.pull_request.head.sha }}
      - uses: anthropics/claude-code-base-action@v0.0.63
        with:
          prompt: Review this pull request.
"
            ),
            [
                (Level::Error, Rule::UnrestrictedTrigger),
                (Level::Error, Rule::PrHeadCheckout),
            ]
        );
        let secrets_of = |inputs: &str| {
            let workflow = Workflow::parse(
                Path::new("test.yml"),
                format!(
                    "on: push
permissions:
  contents: read
jobs:
  claude:
    runs-on: ubuntu-latest
    steps:
      - uses: anthropics/claude-code-base-action@beta
        with:
          prompt: Fix the tests.
{inputs}"
                ),
            )
            .unwrap();
            assess(&workflow, &Upstreams::default())[0]
                .permissions_and_secrets
                .as_ref()
                .map(|score| score.reason.clone())
        };
        assert_eq!(
            secrets_of(
                "          mcp_config: '{\"mcpServers\": {\"x\": {\"env\": {\"KEY\": \"${{ secrets.MCP_KEY }}\"}}}}'\n"
            )
            .as_deref(),
            Some("mcp_config passes secrets.MCP_KEY to the agent")
        );
        assert_eq!(
            secrets_of(
                "          claude_env: |\n            DEPLOY_KEY: ${{ secrets.DEPLOY_KEY }}\n"
            )
            .as_deref(),
            Some("env DEPLOY_KEY passes secrets.DEPLOY_KEY to the agent")
        );
        assert_eq!(
            secrets_of(
                "          claude_env: |\n            # DEPLOY_KEY: ${{ secrets.DEPLOY_KEY }}\n"
            ),
            None
        );
        assert_eq!(
            secrets_of(
                "          claude_env: |\n            ANTHROPIC_API_KEY: ${{ secrets.KEY }}\n"
            ),
            None
        );
    }

    #[test]
    fn claude_env_overrides_env_only_for_the_published_base_action() {
        let rules_of = |uses: &str| {
            let workflow = Workflow::parse(
                Path::new("test.yml"),
                format!(
                    "on: push
permissions:
  contents: read
jobs:
  claude:
    runs-on: ubuntu-latest
    env:
      GH_TOKEN: ${{{{ secrets.BOT_PAT }}}}
    steps:
      - uses: {uses}
        with:
          prompt: Fix the tests.
          allowed_tools: Bash
          claude_args: --allowedTools Bash
          claude_env: |
            GH_TOKEN: ${{{{ github.token }}}}
            ANTHROPIC_API_KEY: ${{{{ secrets.KEY }}}}
      - run: npm test
        env:
          KEY: ${{{{ secrets.KEY }}}}
"
                ),
            )
            .unwrap();
            assess(&workflow, &Upstreams::default())[0]
                .hits
                .iter()
                .map(|hit| hit.rule)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            rules_of("anthropics/claude-code-base-action@beta"),
            [Rule::BroadTools, Rule::AgentCodeWithSecrets]
        );
        assert_eq!(
            rules_of("anthropics/claude-code-action/base-action@v1"),
            [
                Rule::BroadTools,
                Rule::LongLivedToken,
                Rule::AgentCodeWithSecrets
            ]
        );
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
        let assessments = assess(&workflow, &Upstreams::default());
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
    fn workflow_run_started_only_by_writers_reaches_no_rule_about_outsiders() {
        let workflow = Workflow::parse(
            Path::new("test.yml"),
            "on: workflow_run
jobs:
  fix:
    runs-on: ubuntu-latest
    if: github.event.workflow_run.event == 'workflow_dispatch'
    steps:
      - uses: actions/checkout@v5
        with:
          ref: ${{ github.event.workflow_run.head_sha }}
      - uses: anthropics/claude-code-action@v1
"
            .to_string(),
        )
        .unwrap();
        let assessments = assess(&workflow, &Upstreams::default());
        assert_eq!(assessments[0].who_can_trigger, None);
        assert!(assessments[0].hits.is_empty());
    }

    #[test]
    fn workflow_run_restricted_to_an_actor_still_counts_outside_text_and_the_head_checkout() {
        let workflow = Workflow::parse(
            Path::new("test.yml"),
            "on: workflow_run
jobs:
  fix:
    runs-on: ubuntu-latest
    if: github.actor == 'dependabot[bot]'
    steps:
      - uses: actions/checkout@v5
        with:
          ref: ${{ github.event.workflow_run.head_sha }}
      - uses: anthropics/claude-code-action@v1
"
            .to_string(),
        )
        .unwrap();
        let assessments = assess(&workflow, &Upstreams::default());
        assert_eq!(
            assessments[0]
                .who_can_trigger
                .as_ref()
                .map(|score| score.weight),
            Some(Weight::Half)
        );
        assert_eq!(
            assessments[0]
                .hits
                .iter()
                .map(|hit| hit.rule)
                .collect::<Vec<_>>(),
            [Rule::PrHeadCheckout]
        );
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
        assert!(assess(&workflow, &Upstreams::default()).is_empty());
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
                ignored: Vec::new(),
            };
            assert_eq!(
                assessment.level(),
                expected,
                "{trigger:?} {tools:?} {permissions:?}"
            );
        }
    }
}
