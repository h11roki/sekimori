# 0032. Claude Code の base action も、4 つ目のエージェントとして検査する

- ステータス: 承認
- 日付: 2026-11-15

## 文脈

- Anthropic は、claude-code-action の `base-action` の下の階層を、`anthropics/claude-code-base-action` として鏡にしている(2026-11-13 にも更新されている)。`anthropics/claude-code-action/base-action@v1` と、下の階層を指して使う書き方もある
- GitHub のコード検索で、ワークフローの中の `anthropics/claude-code-base-action` は 325 件、`anthropics/claude-code-action/base-action` は 51 件あった。`anthropics/claude-code-action` の 24,736 件の約 1.5%
- `anthropics/claude-code-base-action` のタグは `beta` と `v0.0.x`(最新は `v0.0.63`)で、`v1` は無い。公開版(`beta`、`src/run-claude.ts`)と、claude-code-action の `base-action@v1`(main)では入力が違う
  - 公開版:`prompt`、`prompt_file`、`allowed_tools`、`disallowed_tools`、`mcp_config`、`settings`、`system_prompt`、`append_system_prompt`、`claude_env`。Claude には `process.env` に `claude_env` の `名前: 値` の行を足して渡す
  - main:`prompt`、`settings`、`claude_args` など。Claude には `process.env` を渡す
- base action のソース(`base-action/action.yml`、`src/parse-sdk-options.ts`、2026-11-15 の main と `beta`)で確かめた、両方に共通の振る舞い
  - `github_token`、`allowed_non_write_users`、`allowed_bots` は無く、起動した人を確かめない
  - Claude には `process.env` をそのまま渡す。claude-code-action と違い、`GITHUB_TOKEN` や `GH_TOKEN` を Action のトークンで上書きしない。OIDC の変数だけを消す
  - 設定は `user`、`project`、`local` の全てから読む。作業場所の `.claude/` の設定も読む。claude-code-action のように base の枝の設定に戻す処理は無い
  - 権限のモードは既定のままで、`claude_args` か `settings` で許さない限り、確認の要る道具(編集やシェル)は使えない
  - 出力の `structured_output` は、`claude_args` に `--json-schema` があるときのモデルの文

## 検討した選択肢

1. 4 つ目のエージェントとして足し、上の振る舞いに合わせて各規則を決める
2. claude-code-action と同じエージェントとして扱う
3. 対象にしない

## 決定

高橋さんが判断を任せ(2026-11-15 に「止まらないで。開発改善を進めて」)、Claude が 1 を選んだ。

- `anthropics/claude-code-base-action` と、`anthropics/claude-code-action` の下の階層 `base-action` を、Claude Code の base action とする
- 起動できる人:run-gemini-cli と同じく、ジョブかステップの `if:` が起動した人を確かめなければ、誰でも起動できる
- 道具:claude-code-action と同じく、`claude_args`、`settings`、`allowed_tools`、`disallowed_tools` の許可と権限のモードを読む(ADR 0013)
- 秘密情報:`prompt`、`settings`、`claude_args`、`mcp_config`、`system_prompt`、`append_system_prompt` に流れ込む秘密情報と、`env` の全ての秘密情報が届く。公開版(`anthropics/claude-code-base-action`)では、`claude_env` の行(`#` で始まる行を除く)をエージェントの `env` として読み、同じ名前の `env` より優先する。main の `base-action` には `claude_env` が無いので読まない。Claude 自身の認証情報(`ANTHROPIC_API_KEY` など)は除く。`claude_env` か `env` の `GITHUB_TOKEN` か `GH_TOKEN` が保存した秘密情報なら、期限の無いトークンとする(codex と run-gemini-cli と同じ)
- PR の head の取り出し:特権のイベント(`pull_request_target`、`workflow_run`、`issue_comment`)で前のステップが PR の head を取り出すと、その `.claude/` の設定を読むので当てる(ADR 0014)
- 出力の注入:`structured_output` を数える(ADR 0027)。まとめた出力は `--json-schema` のときだけ
- 書いたコードを後で動かす形(ADR 0028):広い道具(`broad-tools` に当たる形)があるか、権限のモードが `acceptEdits` のときだけ、作業場所を書き換えられるとみなす。既定のモードでは編集を確認なしでは使えないため

## 理由

- 起動した人を確かめない、トークンを上書きしない、作業場所の設定を読む、の 3 つが claude-code-action と違い、同じエージェントとして扱うと危険を軽く見積もる(選択肢 2)
- 件数は少ないが、守りの無い使い方なので、見逃すと重い(選択肢 3)

## 受け入れたトレードオフ

- `Edit` や `Write` だけを許した形は、作業場所を書き換えられるが、`agent-code-with-secrets` では数えない
- `prompt_file` の中身は読まない
- 調査のデータ(ADR 0021)は 3 つの Action だけを集めたので、base action の件数と誤検知の割合は測っていない

## 再検討する条件

- base action に起動した人を確かめる入力が足されたとき
- 調査で base action を集め直したとき
