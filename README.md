# sekimori

Static analysis for GitHub Actions workflows that run AI coding agents.

sekimori reads your workflow YAML and reports where an agent step combines three things:

- **who can trigger** the agent (including outside contributors),
- **which tools** the agent may use, and
- **which permissions and secrets** reach the agent.

Any one of these alone depends on how you use it. When all three overlap, text written by anyone who can open an issue or a pull request can steer an agent that holds your tokens.

It checks the official agent actions:

- [`anthropics/claude-code-action`](https://github.com/anthropics/claude-code-action)
- [`openai/codex-action`](https://github.com/openai/codex-action)
- [`google-github-actions/run-gemini-cli`](https://github.com/google-github-actions/run-gemini-cli)

sekimori only reads YAML. It does not run your workflows, call an LLM, or send your code anywhere.

## Use it as a GitHub Action

```yaml
name: sekimori

on:
  push:
    branches: [main]
  pull_request:

permissions: {}

jobs:
  sekimori:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      security-events: write
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          persist-credentials: false
      - id: sekimori
        uses: h11roki/sekimori@d3b4a6011e27a73c630abc55062a66bdeffe402e # v0.6.0
      - if: >-
          !cancelled()
          && steps.sekimori.outputs.sarif-file != ''
          && (github.event_name != 'pull_request' || github.event.pull_request.head.repo.full_name == github.repository)
        uses: github/codeql-action/upload-sarif@33d7b85da1a04171e211befe2c739303b9a4c066 # v4.39.3
        with:
          sarif_file: ${{ steps.sekimori.outputs.sarif-file }}
          category: sekimori
```

The sekimori step fails when it finds something, so the upload step runs with `!cancelled()`. Pull requests from forks cannot get `security-events: write`, so the upload is skipped for them; the findings are still printed in the log.

The action downloads the release binary that matches its own version and checks its [artifact attestation](https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations) before running it. It works on GitHub-hosted Linux (x64, ARM64), macOS (ARM64) and Windows (x64) runners, and needs the GitHub CLI (`gh`) on self-hosted runners.

| Input | Default | Description |
|---|---|---|
| `paths` | `.github/workflows` | Workflow files or directories to check, separated by whitespace or newlines |
| `token` | `${{ github.token }}` | Token used to download the release and verify its attestation |

| Output | Description |
|---|---|
| `sarif-file` | Path to the findings in SARIF 2.1.0 |

## Use it from the command line

Download a binary from [Releases](https://github.com/h11roki/sekimori/releases) and verify it:

```sh
gh release download v0.6.0 --repo h11roki/sekimori --pattern sekimori-aarch64-apple-darwin
gh attestation verify sekimori-aarch64-apple-darwin \
  --repo h11roki/sekimori \
  --signer-workflow h11roki/sekimori/.github/workflows/release.yml \
  --source-ref refs/tags/v0.6.0
chmod +x sekimori-aarch64-apple-darwin
```

Or build it from source with Rust 1.99.1:

```sh
cargo install --locked --git https://github.com/h11roki/sekimori --tag v0.6.0
```

Then run it in your repository:

```sh
sekimori                         # checks .github/workflows
sekimori path/to/workflow.yml    # files or directories
sekimori --format sarif > sekimori.sarif
```

For this workflow, which lets anyone start Gemini CLI with a comment:

```yaml
on:
  issue_comment:
    types: [created]

jobs:
  gemini:
    if: contains(github.event.comment.body, '@gemini-cli')
    runs-on: ubuntu-latest
    steps:
      - uses: google-github-actions/run-gemini-cli@v0
        with:
          prompt: ${{ github.event.comment.body }}
```

sekimori prints:

```text
.github/workflows/gemini.yml:10:9: error[unrestricted-trigger]: google-github-actions/run-gemini-cli@v0 in job `gemini` can be started without write access
  who can trigger      1    the `if:` does not check who triggered it, on issue_comment
  allowed tools        1    no `settings`, so tools.core is unset and every built-in tool runs under --yolo
  permissions/secrets  1    no permissions are set, so GITHUB_TOKEN gets the repository default
  total                3 of 3
.github/workflows/gemini.yml:10:9: error[broad-tools]: google-github-actions/run-gemini-cli@v0 in job `gemini` can run unrestricted tools
  who can trigger      1    the `if:` does not check who triggered it, on issue_comment
  allowed tools        1    no `settings`, so tools.core is unset and every built-in tool runs under --yolo
  permissions/secrets  1    no permissions are set, so GITHUB_TOKEN gets the repository default
  total                3 of 3
```

Each rule that matched is printed with the step's breakdown. [Fixing findings](#fixing-findings) shows how to change this workflow.

| Exit code | Meaning |
|---|---|
| 0 | No findings |
| 1 | At least one finding |
| 2 | A file could not be read, or the arguments are wrong |

## Rules

| ID | Axis | What it reports |
|---|---|---|
| `unrestricted-trigger` | who can trigger | The agent runs on an event that outside contributors can cause, and the workflow lets anyone start it: a wildcard user or bot allow-list, or no `if:` that checks who triggered it (a trusted actor or author association, or the label that was just added). On `workflow_run`, the upstream run also counts as checked when an `if:` limits its event, or when the upstream workflow, found by its `name:` among the files checked together in the same directory, has only triggers that outside contributors cannot cause, such as `workflow_dispatch`, `schedule`, `release` or `push`. Gemini CLI given no `prompt` input runs only a fixed prompt, so it does not count here when it runs only on `issues`, `issue_comment`, `discussion` or `discussion_comment` and every earlier step in the job is an `actions/checkout` of this repository at a fixed ref. |
| `broad-tools` | allowed tools | The agent may run any shell command or tool: an unscoped Bash allow rule, a bypassed permission prompt, a sandbox with full access, or Gemini CLI without a `tools.core` allow-list under `--yolo`. |
| `pr-head-checkout` | allowed tools | A step before the agent checks out the pull request head into the workspace on a privileged event, and the agent then loads that head's settings, hooks or MCP servers. |
| `agent-output-injection` | allowed tools | A later step in the job expands text the model wrote into code with `${{ }}`: Claude Code's `structured_output`, Codex's `final-message`, or Gemini CLI's `summary` or `error` in a `run` script or an `actions/github-script` `script`. Whoever steers the agent can then run commands in that step, and secrets that reach that step count on the permissions/secrets axis. |
| `long-lived-token` | permissions/secrets | The GitHub token handed to the agent comes from a stored secret, such as a personal access token, instead of `GITHUB_TOKEN` or a short-lived app token. |
| `other-secrets` | permissions/secrets | A secret other than the agent's own credentials and the GitHub token reaches the agent through its inputs or environment. |
| `agent-code-with-secrets` | permissions/secrets | The agent can edit the workspace, and a later step in the job runs project code with a secret other than `GITHUB_TOKEN`: a `./` script or a relative path to a script file, a script file given to a shell or interpreter, or a known build or test command such as `npm test`, `make` or `pytest`. Whoever steers the agent can change that code and read the secret. |

## Severity

For each agent step, sekimori adds up the three axes. Each axis scores 1 when it is met, and the who-can-trigger axis scores 0.5 when only outside contributors' text can reach the agent (for example, a maintainer mentions the agent on an outsider's issue).

| Total | Level |
|---|---|
| 3 | `error` |
| 2 or more | `warning` |
| below 2 | not reported |

Every rule that matched the step is reported with the step's level. In SARIF, results for the same rule at the same place are merged into one, with the heavier level and every step's breakdown.

## Fixing findings

Removing any one axis takes a step below `error`; removing two leaves nothing to report.

### Who can trigger

Let only people with write access start the agent.

- `claude-code-action` and `openai/codex-action`: list user names in `allowed_non_write_users`, `allowed_bots` or `allow-users` instead of `'*'`.
- `run-gemini-cli`: check who triggered the run in the job's or step's `if:`.

```yaml
on:
  issue_comment:
    types: [created]

jobs:
  gemini:
    if: >-
      contains(github.event.comment.body, '@gemini-cli')
      && contains(fromJSON('["OWNER", "MEMBER", "COLLABORATOR"]'), github.event.comment.author_association)
```

A check on `github.actor`, or `github.event.label.name == '<label>'` on `types: [labeled]`, also counts. Outside contributors' text can still reach the agent when a maintainer starts it on their issue, so this axis scores 0.5 instead of 0.

### Allowed tools

Allow only the commands the task needs.

- `claude-code-action`: allow scoped commands such as `--allowedTools "Bash(npm test:*)"` in `claude_args`. Do not allow `Bash` alone, a shell or interpreter with any arguments (`Bash(bash:*)`, `Bash(python *)`), or `--permission-mode bypassPermissions`.
- `openai/codex-action`: do not set `sandbox: danger-full-access`, `permission-profile: ":danger-full-access"` or `safety-strategy: unsafe`.
- `run-gemini-cli`: pass `settings` with a `tools.core` allow-list, and scope the shell tool to the commands the task needs.

```yaml
      - uses: google-github-actions/run-gemini-cli@v0
        with:
          settings: |-
            {
              "tools": {
                "core": ["read_file", "glob", "grep_search", "run_shell_command(npm test)"]
              }
            }
```

For `agent-output-injection`, pass the output through `env:` and read it as a variable, so it is never parsed as code. In `actions/github-script`, read it with `process.env.SUMMARY`.

```yaml
      - run: echo "$SUMMARY" >> "$GITHUB_STEP_SUMMARY"
        env:
          SUMMARY: ${{ steps.gemini.outputs.summary }}
```

For `pr-head-checkout`, do not check out the pull request head before the agent on a privileged event. Check out the base branch, or run the agent on `pull_request`, where it gets no secrets from forks.

### Permissions and secrets

- `long-lived-token`: give the agent `${{ github.token }}` or a short-lived token from [`actions/create-github-app-token`](https://github.com/actions/create-github-app-token), not a personal access token stored as a secret.
- `other-secrets`: use other secrets in separate steps instead of the agent's inputs or `env`.
- `agent-code-with-secrets`: run build and test steps that need secrets in a separate job that checks out the repository again, or before the agent step, so they never run files the agent edited.
- Set `permissions:` on the job, and give write access to `contents`, `actions`, `packages`, `deployments` or `id-token` only when the agent needs it. Without `permissions:`, `GITHUB_TOKEN` gets the repository default.

## Limitations

- It is a static check. It cannot see what the agent reads at run time; if the agent may run `gh`, it can read issues no matter what triggered it.
- It only knows the settings of the three actions above, as read from their source. New inputs or changed defaults need new releases of sekimori.
- `if:` conditions on earlier jobs in `needs:` count when the agent job waits for them to succeed, directly or through other jobs: a waiting job's `if:` has no status function, or has `success()` or `needs.<id>.result == 'success'` joined with `&&`. `needs:` names must match job IDs exactly, including case. Checks done in an earlier job's steps, such as a script that sets an output the agent job compares, are not read.
- `agent-output-injection` reads only steps later in the agent's job: `run` scripts and `actions/github-script` `script`, directly or through `${{ env.NAME }}`. It does not follow job `outputs` into other jobs, wildcards such as `steps.*.outputs.summary`, other actions that take code, or `shell:` and `working-directory:`.
- `agent-code-with-secrets` knows only the commands listed above. It does not read other build tools, actions that run workspace code, or later jobs that receive the workspace through artifacts or caches.

## Survey

In November 2026 sekimori was run on 2,044 public workflows found by GitHub code search (up to 1,000 per action). Checking each file alone, 375 of them (18.3%) had at least one finding and 55 (2.7%) were `error`. With v0.7.0, checking each repository's files together so that `workflow_run` upstreams could be found, no longer counting Gemini CLI steps that get no `prompt`, reading `if:` on earlier jobs in `needs:`, and adding `agent-output-injection` and `agent-code-with-secrets`, 369 (18.1%) had a finding and 43 (2.1%) were `error`. Almost all `error` findings were `run-gemini-cli` workflows that start on issues or comments without checking who triggered them.

In random samples checked by reading the workflows, no false positives were found for `broad-tools` or the permissions/secrets axis. All false positives were on the who-can-trigger axis: about half of the sampled `error` findings depended on something outside the workflow file, such as an upstream workflow, an earlier job in `needs:`, or an agent that gets no outside text. The counts and the method are in [docs/survey/2026-11.md](docs/survey/2026-11.md) (in Japanese); repository names are not published.

## Design

The reasons behind each decision are recorded in Japanese in [docs/adr](docs/adr/README.md).

## License

[MIT](LICENSE)
