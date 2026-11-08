# 0003. 最初の版は公式の 3 つの Action を対象にし、各社の公式文書の警告を規則の根拠にする

- ステータス: 承認
- 日付: 2026-11-08

## 文脈

2026-11-08 に、3 つの Action の action.yml とセキュリティの文書を読んだ。読んだ版は次のとおり。

| Action | 読んだ版(既定ブランチの commit) |
|---|---|
| anthropics/claude-code-action | 115e0b2(2026-11-06) |
| openai/codex-action | 13e9c17(2026-10-30) |
| google-github-actions/run-gemini-cli | 387c8dd(2026-08-21) |

3 つとも、起動できる人、許した道具、権限の考え方を持つが、設定の名前と既定値はばらばら。

### claude-code-action

- 既定では、書き込み権限のある人だけが起動できる。issue、PR、コメント、レビュー、`workflow_run` で確かめる
- `allowed_non_write_users` は、書き込み権限のない人にも起動を許す。文書は「重大なリスク」とし、権限を最小にしたワークフロー(issue のラベル付けなど)に限るよう書く。使うときは `github_token` に `GITHUB_TOKEN` を渡し、個人のアクセストークンは渡さない(期限が無く、指示の注入で少しずつ盗まれうる)。`claude_args` の `--allowedTools` で道具を最小に絞るよう勧める
- `allowed_bots` に `'*'` を入れると、bot についてはリポジトリの権限を確かめないので、公開リポジトリでは誰が作った GitHub App でも、その App が書いた指示で起動できる
- `pull_request_target` と `workflow_run` は、基のリポジトリの秘密情報で動く。PR の head を作業場所の根に checkout してから Action を動かしてはいけない
- `show_full_output` は、秘密情報を含みうる全ての出力を公開のログに出す。`display_report` は、信頼できる入力だけのときに使う
- `include_comments_by_actor` と `exclude_comments_by_actor` で、エージェントに渡すコメントの書き手を絞れる

### openai/codex-action

- 書き込み権限のある人は既定で起動できる。`allow-users` で他の人を足せ、`'*'` で全員を許す
- `allow-bots` と `allow-bot-users` は、bot について書き込み権限の確認を飛ばす
- `safety-strategy` の既定は `drop-sudo`。ほかに `unprivileged-user`、`read-only`、`unsafe`(勧めない)がある
- `sandbox`(`workspace-write`、`read-only`、`danger-full-access`)と、その後継の `permission-profile` で、Codex が触れる範囲を決める

### run-gemini-cli

- Action の入力に、起動できる人を絞る項目が無い。絞るのはワークフローの側(`if:` の条件など)になる
- 道具は、`settings` に書く Gemini CLI の設定で絞る。設定のどの項目を読むかは、実装の前に Gemini CLI の文書で確かめる
- 公式の Trust Guidance は、信頼できる入力とできない入力を分け、それに合わせた境界を置くよう求める

## 検討した選択肢

1. 公式の 3 つ
2. claude-code-action だけ
3. 3 つと gh-aw(GitHub Agentic Workflows)

## 決定

高橋さんが 1 を選んだ。

- 3 つの Action の設定を、ADR 0001 の 3 つの軸(起動できる人、許した道具、権限と秘密情報)に読み替えて判定する。規則は軸の重なりで書き、Action ごとの違いは読み替えの層に閉じ込める
- 規則の根拠は、各社の公式文書の警告と報告された事件に置き、規則ごとに出典(読んだ文書の版)を残す
- 最初の版の規則は 5 個ほどにする。どれを選ぶかは、実装の前に別の ADR で決める

## 理由

- 3 社をまたいで同じ物差しで見られることが、各社が自社の Action の中で行う確認との違いになる
- claude-code-action だけにすると、1 社専用の道具に見え、共通の形を後から作り直すことになる
- gh-aw は設定の形(Markdown から Actions を作る)が別物で、最初の版が大きくなる

## 受け入れたトレードオフ

- 追う設定が 3 社分になる。読んだ版を記録し、各社の更新を定期的に見直す
- run-gemini-cli は起動できる人をワークフローの側で絞るので、`if:` の条件を読む必要があり、他の 2 つより判定が難しい
- gh-aw を使う人は、最初の版では守れない

## 再検討する条件

- 3 社のどれかが、設定の名前や既定値を大きく変えたとき
- 公開リポジトリで、gh-aw を使うワークフローが、3 つの Action を直接使うものに並ぶほど増えたとき
- 3 つ以外のエージェントの公式の Action が、公開リポジトリで広く使われるようになったとき
