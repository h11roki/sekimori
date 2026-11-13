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
        uses: h11roki/sekimori@de939eae8f92c2d9d97204ce4f7a1332415ad818 # v0.1.0
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
gh release download v0.1.0 --repo h11roki/sekimori --pattern sekimori-aarch64-apple-darwin
gh attestation verify sekimori-aarch64-apple-darwin \
  --repo h11roki/sekimori \
  --signer-workflow h11roki/sekimori/.github/workflows/release.yml \
  --source-ref refs/tags/v0.1.0
chmod +x sekimori-aarch64-apple-darwin
```

Or build it from source with Rust 1.99.1:

```sh
cargo install --locked --git https://github.com/h11roki/sekimori --tag v0.1.0
```

Then run it in your repository:

```sh
sekimori                         # checks .github/workflows
sekimori path/to/workflow.yml    # files or directories
sekimori --format sarif > sekimori.sarif
```

```text
.github/workflows/codex.yml:8:11: error[unrestricted-trigger]: openai/codex-action@v1 in job `codex` can be started without write access
  who can trigger      1    allow-users: '*', on issue_comment
  allowed tools        1    safety-strategy: unsafe
  permissions/secrets  1    no permissions are set, so GITHUB_TOKEN gets the repository default
  total                3 of 3
```

| Exit code | Meaning |
|---|---|
| 0 | No findings |
| 1 | At least one finding |
| 2 | A file could not be read, or the arguments are wrong |

## Rules

| ID | Axis | What it reports |
|---|---|---|
| `unrestricted-trigger` | who can trigger | The agent runs on an event that outside contributors can cause, and the workflow lets anyone start it: a wildcard user or bot allow-list, or no `if:` that checks who triggered it (a trusted actor, author association or label). On `workflow_run`, an `if:` that limits the upstream run to `push`, `workflow_dispatch`, `schedule` or a same-repository `pull_request` also counts as a check. |
| `broad-tools` | allowed tools | The agent may run any shell command or tool: an unscoped Bash allow rule, a bypassed permission prompt, a sandbox with full access, or Gemini CLI without a `tools.core` allow-list under `--yolo`. |
| `pr-head-checkout` | allowed tools | A step before the agent checks out the pull request head into the workspace on a privileged event, and the agent then loads that head's settings, hooks or MCP servers. |
| `long-lived-token` | permissions/secrets | The GitHub token handed to the agent comes from a stored secret, such as a personal access token, instead of `GITHUB_TOKEN` or a short-lived app token. |
| `other-secrets` | permissions/secrets | A secret other than the agent's own credentials and the GitHub token reaches the agent through its inputs or environment. |

## Severity

For each agent step, sekimori adds up the three axes. Each axis scores 1 when it is met, and the who-can-trigger axis scores 0.5 when only outside contributors' text can reach the agent (for example, a maintainer mentions the agent on an outsider's issue).

| Total | Level |
|---|---|
| 3 | `error` |
| 2 or more | `warning` |
| below 2 | not reported |

Every rule that matched the step is reported with the step's level. In SARIF, results for the same rule at the same place are merged into one, with the heavier level and every step's breakdown.

## Limitations

- It is a static check. It cannot see what the agent reads at run time; if the agent may run `gh`, it can read issues no matter what triggered it.
- It only knows the settings of the three actions above, as read from their source. New inputs or changed defaults need new releases of sekimori.
- `if:` conditions on earlier jobs in `needs:` are not taken into account.

## Design

The reasons behind each decision are recorded in Japanese in [docs/adr](docs/adr/README.md).

## License

[MIT](LICENSE)
