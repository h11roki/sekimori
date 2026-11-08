# 0006. 最初の版の規則は、起動の制限を外した、広い道具、期限の無いトークン、PR の head の checkout、他の秘密情報の 5 つにする

- ステータス: 承認
- 日付: 2026-11-08

## 文脈

- ADR 0001 は、判定を 3 つの軸(起動できる人、許した道具、権限と秘密情報)の重なりで行うと決め、ADR 0003 は、最初の版の規則を 5 個ほどにすると決めた
- 各社の既定は次のとおり(2026-11-08 に読んだ文書による。版は ADR 0003 の表と、Gemini CLI の 62bab5b)
  - claude-code-action は、書き込み権限のある人だけが起動できる。シェル(Bash)は、`claude_args` の `--allowedTools` か `settings` の `permissions.allow` で許さない限り使えない
  - codex-action は、書き込み権限のある人だけが起動できる。`safety-strategy` の既定は `drop-sudo`、`sandbox` の既定は作業場所に書ける `workspace-write`
  - run-gemini-cli は、Gemini CLI を常に `--yolo`(道具の使用を全て自動で承認)で動かす。Gemini CLI の `tools.core`(組み込みの道具の許可リスト)は既定で未設定で、未設定なら組み込みの道具に制限はかからない。公式の例は `tools.core` を空にし、GitHub の MCP の道具のうち、読み取りとコメントの追加(`add_issue_comment`)だけを `includeTools` で許している。Action の入力に起動できる人を絞る項目は無く、公式の例はワークフローの `if:` に書く `author_association` の条件で絞っている
- 既定のままなら、claude-code-action と codex-action は起動できる人と道具が絞られている。危なくなるのは、使う側が制限を外したときと、run-gemini-cli で道具や起動できる人を絞り忘れたとき
- GitHub は、`GITHUB_TOKEN` で起こしたイベントでは、`workflow_dispatch` と `repository_dispatch` を除いて新しいワークフローを動かさない

## 検討した選択肢

1. 3 つの軸の重なりを作る 5 つ: 起動の制限を外した、広い道具 × 書き込み権限か秘密情報、期限の無いトークン、PR の head の checkout、他の秘密情報
2. 各社の文書の警告に忠実な 5 つ: 1 の起動の制限、期限の無いトークン、PR の head の checkout に、`show_full_output: true` と `allowed_bots: '*'` を個別の規則として足す
3. まず 3 つ: 1 の起動の制限、広い道具、PR の head の checkout

## 決定

高橋さんが 1 を選んだ。各規則が満たす軸と、何を見て当てるかは次のとおり。重さの決め方は ADR 0007 で決める。

| 規則 | 満たす軸 | 当てる設定 |
|---|---|---|
| 起動の制限を外した | 起動できる人 | 外部の人が起こせるトリガーで動き、かつ次のどれかに当たる。claude-code-action の `allowed_non_write_users: '*'` か `allowed_bots: '*'`。codex-action の `allow-users: '*'`。run-gemini-cli を動かすジョブとステップの `if:` に、起動した人を絞る条件が無い |
| 広い道具 | 許した道具 | claude-code-action で、`claude_args` の `--allowedTools` か `settings` の `permissions.allow` に、範囲を絞らない `Bash` がある。codex-action が `sandbox: danger-full-access` か `safety-strategy: unsafe`。run-gemini-cli の `settings` で、`tools.core` が未設定か、範囲を絞らないシェル(`run_shell_command`)を許している |
| 期限の無いトークン | 権限と秘密情報 | エージェントに渡す GitHub のトークンが、`secrets.GITHUB_TOKEN` でも GitHub App で作ったトークンでもない(個人のアクセストークンなど) |
| PR の head の checkout | 許した道具 | `pull_request_target` か `workflow_run` で、エージェントより前のステップが PR の head を作業場所に checkout している。PR の中の設定(`.claude/settings.json` の hooks など)をエージェントが読み込みうる |
| 他の秘密情報 | 権限と秘密情報 | エージェントのステップかジョブの `env`、またはステップの `with` に、エージェント自身の API キーと GitHub のトークン以外の `secrets.*` がある |

- 外部の人が起こせるトリガーは、最初の版では `issues`、`issue_comment`、`pull_request_target`、`pull_request_review`、`pull_request_review_comment`、`discussion`、`discussion_comment`、`workflow_run` とする。`pull_request` は、フォークからの PR には秘密情報が渡らず、トークンも読み取りだけになるので含めない
- 権限と秘密情報の軸は、上の 2 つの規則のほかに、エージェントを動かすジョブの権限に `contents`、`actions`、`packages`、`deployments`、`id-token` の書き込みか `write-all` があるときと、権限がジョブにもワークフローにも書かれていないときにも満たす。ジョブに `permissions` が無ければ、ワークフローの `permissions` を受け継いだものとして見る。どちらにも無い権限は、リポジトリの設定しだいで書き込みになり、静的には分からないため
- `issues` と `pull-requests` の書き込みは数えない。エージェントが結果をコメントで返すのにほぼ必ず要り、数えるとほとんどのワークフローが当たるため
- 選択肢 1 では、広い道具を「広い道具 × 書き込み権限か秘密情報」としていた。掛け合わせは ADR 0007 の重さで表せるので、規則としては許した道具の軸だけを見る。広い道具だけでは軸が 1 つなので指摘は出ず、書き込み権限か秘密情報がそろうと出るので、振る舞いは選択肢の説明と同じになる
- 選択肢の説明で挙げた codex-action の `allow-bots` は、最初の版では含めない。通すのは github-actions[bot] だけで、`GITHUB_TOKEN` で起こしたイベントは原則としてワークフローを動かさないため。claude-code-action の `allowed_non_write_users` も、特定の人を並べた一覧なら起動できる人が決まっているので、`'*'` のときだけ当てる

## 理由

- 5 つで 3 つの軸を全て覆える。起動できる人は 1 つ、許した道具は 2 つ、権限と秘密情報は 2 つの規則と権限の設定で満たす
- 各社の既定が安全側にあるので、規則は「既定から外した設定」と「run-gemini-cli で絞り忘れた設定」に集まる。どちらも各社の文書が警告している
- 選択肢 2 の `show_full_output: true` は 1 行の設定の検査で、ADR 0001 の重なりの考えから外れる。`allowed_bots: '*'` は、1 の起動の制限を外した規則に含めた

## 受け入れたトレードオフ

- `if:` の式は、最初の版では `author_association`、`github.actor`、`github.triggering_actor` を比べているかで判断する。複雑な式は読み違えうる。式の読み方は実装で決める
- 予定(`schedule`)や手動(`workflow_dispatch`)で動き、外部の人の issue を読むワークフローは、トリガーからは外部の入力と分からない
- 再利用できるワークフローや複合 Action の中でエージェントを動かすものは、最初の版では追わない
- エージェントは `uses` の Action の名前で見分けるので、フォークや自作の包みは見逃す

## 再検討する条件

- 公開リポジトリに当てて、誤検知か見逃しが目立つとき
- 各社が既定や設定の名前を変えたとき(ADR 0003)
