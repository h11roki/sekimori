# 0014. PR の head の checkout は、エージェントが PR の設定を読み込むときだけ当てる

- ステータス: 承認
- 日付: 2026-11-13

## 文脈

- ADR 0006 は、規則「PR の head の checkout」(`pr-head-checkout`)で許した道具の軸を満たすとし、`pull_request_target` か `workflow_run` で、エージェントより前のステップが PR の head を作業場所に checkout しているときに当てるとした。理由は、PR の中の設定(`.claude/settings.json` の hooks など)をエージェントが読み込みうるため
- ADR 0013 と同じく、各社の振る舞いはソースで確かめた(2026-11-13)
  - actions/checkout(f548e57): `ref` が無ければ、そのイベントの `GITHUB_REF` と `GITHUB_SHA` を checkout する。`pull_request_target` では base の枝、`workflow_run` では既定の枝になり、PR の head は `ref` か `repository` で指したときだけ来る。`path` は作業場所からの相対で、既定は作業場所の根
  - claude-code-action(4d75e5f): Claude Code は作業場所の根で動き、`settingSources` の既定は `user`、`project`、`local` なので、`.claude/settings.json`(hooks)、`.mcp.json`、`CLAUDE.md` を作業場所から読む。ただし、PR に関わるイベント(`issues`、`issue_comment`、`pull_request`、`pull_request_target`、`pull_request_review`、`pull_request_review_comment`)で PR を対象にするときは、Claude Code を動かす前に `.claude`、`.mcp.json`、`CLAUDE.md` などを base の枝から戻す(`src/github/operations/restore-config.ts` の `restoreConfigFromBase`)。`workflow_run` では戻さない
  - codex-action(13e9c17)と Codex(16280e1): 作業場所の `.codex/config.toml` と hooks は、信頼したプロジェクトでなければ効かない。codex-action は信頼の設定を書かず、`codex-args` で信頼を足すことも拒む。`AGENTS.md` は信頼に関わらず読む
  - run-gemini-cli(387c8dd)と Gemini CLI(b977656): `settings` が空でなければ `.gemini/settings.json` を上書きし、空なら checkout したものが残る。Gemini CLI は、作業場所を信頼していなければ作業場所の設定を捨てる。信頼は `GEMINI_CLI_TRUST_WORKSPACE=true` か信頼したフォルダの一覧で決まり、CI では一覧が無い。公式の例の 6 つのうち 5 つが `GEMINI_CLI_TRUST_WORKSPACE: 'true'` を書いている
- `issue_comment` も、`pull_request_target` と同じく base の枝の権限と秘密情報で動き、`gh pr checkout` で PR の head を取り出すワークフローがよくある
- Anthropic の例の `ci-failure-auto-fix.yml` は、`workflow_run` で `ref: ${{ github.event.workflow_run.head_branch }}` を checkout してから claude-code-action を動かす

## 検討した選択肢

1. PR の head を checkout していれば、エージェントの種類に関わらず当てる
2. 各社のソースで、checkout した PR の設定をエージェントが読み込むと確かめた組み合わせだけ当てる
3. 2 に加え、`AGENTS.md` や `GEMINI.md` のような指示のファイルを読むときも当てる

## 決定

高橋さんが判断を任せ(2026-11-12 に「承認取らなくていい」)、Claude が 2 を選んだ。

- ワークフローのトリガーに `pull_request_target`、`workflow_run`、`issue_comment` のどれかがあり、同じジョブでエージェントより前のステップが PR の head を作業場所の根に checkout していて、エージェントが次に当たるときに当てる
  - claude-code-action: トリガーに `workflow_run` がある。他のイベントでは action が設定を base から戻す
  - run-gemini-cli: `settings` が無いか空で、ステップ、ジョブ、ワークフローの順で最初に見つかる `env` の `GEMINI_CLI_TRUST_WORKSPACE` が `true`
  - codex-action: 当てない。作業場所の設定は信頼が無いと効かないため
- PR の head の checkout は次のどれか
  - `actions/checkout` のステップで、`ref` か `repository` の値が PR の head を指し、`path` が作業場所の根(無い、空、`.`、`./`、`${{ github.workspace }}` など、`.` と空の区切りだけからなるもの)。PR の head を指す値は、`github.event.pull_request.head.`、`github.event.pull_request.merge_commit_sha`、`github.head_ref`、`github.event.workflow_run.head_`、`github.event.workflow_run.pull_requests[...].head.` を含むもの、または `pull/<番号>/head` や `pull/<番号>/merge` の形。大文字と小文字と空白を無視して比べる
  - `run` のステップで、`gh pr checkout` があるか、`git` の `checkout`、`switch`、`fetch`、`pull`、`reset` があり、スクリプトか、そのステップ、ジョブ、ワークフローの `env` の値が PR の head を指すもの。コマンドは空白とシェルの区切り(`;`、`&`、`|`、括弧、`` ` ``)で語に分けて読み、`git -C <dir>` や `git -c <name>=<value>` のような前に置くオプションは飛ばす
- エージェントより前のステップを順に見て、作業場所の根への `actions/checkout` は作業場所を入れ替えるものとして扱う。PR の head を取り出した後に、PR の head を指さない checkout が根にあれば、base に戻ったとみなして当てない。`run` は、まだ PR の head を取り出していないときだけ見る
- エージェントのステップが `parallel:` の中にあるときは、その `parallel:` より前のステップを見る
- 指摘は checkout のステップの `ref`、`repository`、`run` のどれかを指す。広い道具と両方に当たったときは、許した道具の軸の理由に両方を書く

## 理由

- ADR 0006 が規則の理由にしたのは「PR の設定をエージェントが読み込むこと」で、各社はそれぞれ守りを入れている。守りが効く組み合わせで当てると、利用者が確かめても危なさを見つけられない指摘になる
- 1 は、claude-code-action の `pull_request_target` と codex-action の全てで誤検知になる
- 3 の指示のファイルは、外部の人の文面がエージェントに届く経路で、起動できる人の軸(ADR 0007 の半分)ですでに数えている。道具や権限を変えるものではない
- `issue_comment` を足すのは、`gh pr checkout` を使うワークフローが `pull_request_target` と同じ形で PR の head を取り出すため

## 受け入れたトレードオフ

- 各社の守りは、確かめた版の振る舞いとする。claude-code-action の古い版や、Gemini CLI の版を固定した古い版では、守りが無くても当てない
- claude-code-action が base から戻すのは作業場所の根の決まったファイルだけで、`package.json` のスクリプトや `Makefile` は PR の head のまま残る。許した道具でそれらを動かせると PR のコードが走るが、それは広い道具の範囲として、この規則では見ない
- claude-code-action の `claude_args` の `--setting-sources` で `project` を外していても、`workflow_run` なら当てる
- run-gemini-cli の `settings` が空でなくても、`.gemini/` の他のファイル(`commands` など)は PR の head のまま残るが、見ない
- `GEMINI_CLI_TRUST_WORKSPACE` を前のステップで `$GITHUB_ENV` に書くときや、値が式のときは、信頼していないとみなす
- checkout の `ref` が前のステップの出力(`${{ steps.pr.outputs.sha }}`)のように読めないときは、PR の head と分からないので当てない
- 部分文字列で比べるので、PR の head を指す値を条件にだけ使う式(`head.repo.fork && 'a' || 'b'`)も PR の head とみなす
- `run` の `git checkout -b <名前>` のように、PR の head の名前で枝を作るだけのコマンドも、PR の head を取り出したとみなす。`git checkout origin/main -- .claude` のように一部のファイルだけ base に戻すコマンドは、戻したとみなさない
- 同じ checkout の後に複数のエージェントがあれば、エージェントのステップごとに同じ場所を指す指摘が出る
- checkout のステップの `if:` は見ない。`if: false` の checkout も数える
- `run` のステップの `working-directory` は見ず、作業場所の根に取り出したとみなす

## 再検討する条件

- 各社が、PR の設定を読み込む振る舞いや守りを変えたとき(ADR 0003)
- 公開リポジトリで、`steps.*.outputs` を通して PR の head を checkout するワークフローが目立つとき
