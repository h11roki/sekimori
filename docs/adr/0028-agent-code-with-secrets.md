# 0028. エージェントが書き換えられるコードを、後のステップが秘密情報を持って動かす形を、権限と秘密情報の軸の規則 `agent-code-with-secrets` として数える

- ステータス: 承認
- 日付: 2026-11-15

## 文脈

- 本人に次の作業を聞き、2026-11-15 に「エージェントが書いたコードを後で動かす形」が選ばれた
- 調査のデータ(ADR 0021)で、エージェントと同じジョブの後のステップが、プロジェクトのコードを動かすコマンド(`npm test`、`make`、`pytest`、`./script.sh` など)を持つファイルは 155 あった。そのうち 46 は、そのステップに秘密情報(`GITHUB_TOKEN` 以外)が届き、37 は今の判定で指摘が出ていない
- 同じジョブのステップは作業場所を共有する。エージェントが `package.json` の scripts、テスト、`Makefile`、スクリプトを書き換えると、後のステップはそれを自分の秘密情報を持って動かす。エージェントに秘密情報を渡さない守りが、ここで抜ける
- 各 Action の既定で、エージェントが作業場所を書き換えられるかを確かめた(2026-11-15 の main)
  - claude-code-action: `src/create-prompt/index.ts` は、Edit と Write を許可の一覧に入れない理由を「権限のモードが `$GITHUB_WORKSPACE` の中の編集を許すため」としている。既定で書き換えられる
  - codex-action: `sandbox` を空にすると `workspace-write` になる。`sandbox: read-only` か `permission-profile: ":read-only"` のときだけ書き換えられない
  - run-gemini-cli: 常に `--yolo` で動く。道具の名前は `write_file`、`replace`、`run_shell_command`(gemini-cli の `base-declarations.ts`)。`tools.core` が無ければ全ての道具が使える

## 検討した選択肢

1. エージェントが作業場所を書き換えられ、同じジョブの後の `run` が、知られたコードを動かすコマンドを持ち、そのステップに秘密情報が届くなら、権限と秘密情報の軸を満たす規則として数える
2. コマンドを問わず、後のステップに秘密情報が届けば数える
3. 1 に加えて、成果物やキャッシュで作業場所を受け継ぐ他のジョブ、作業場所のコードを動かす Action(`pypa/gh-action-pypi-publish` など)も読む
4. 何もしない

## 決定

高橋さんが判断を任せ(2026-11-15 に、次の作業としてこの形を選んだ。進め方は「承認を求めないでどんどん続けていい」)、Claude が 1 を選んだ。

- 規則の ID は `agent-code-with-secrets`。権限と秘密情報の軸を満たす(重さ 1)。軸の理由は他の秘密情報の理由と `; ` でつなぐ
- エージェントが書き換えられるとみなす条件
  - claude-code-action: 常に
  - codex-action: `sandbox` が `read-only` でなく、`permission-profile` が `:read-only` でない
  - run-gemini-cli: `settings` が無いか、読めて `tools.core` が無いか、`tools.core` に `write_file`、`replace`、`run_shell_command` のどれかがあり `tools.exclude` で外していない。`settings` が式などで読めないときは、書き換えられるとみなす
- コードを動かすコマンドは、`run` を ADR 0014 と同じくシェルの順で区切り、各コマンドの名前で決める
  - `./` で始まるパス、`bash` などのシェルとインタプリタ(`python`、`node`、`ruby`、`perl`)にファイルか `-m` を渡す形
  - `make`、`pytest`、`tox`、`nox`、`pre-commit`、`gradle`、`mvn`、`rake`、`npx`
  - 部分コマンドまで決めるもの: `npm`、`pnpm`、`yarn`、`bun` の `run`、`test`、`install`、`ci`、`exec`、`build`、`start`、`publish`。`cargo` の `test`、`build`、`run`、`publish`。`go` の `test`、`build`、`run`、`generate`。`dotnet` の `test`、`build`、`run`。`bundle exec`、`uv run`、`poetry run`
- 秘密情報は、ADR 0027 と同じく、そのステップ、ジョブ、ワークフローの `env` と、`run` の本文の `${{ secrets.* }}`(`GITHUB_TOKEN` を除く)
- 後のステップの `if:` が定数で偽なら数えない
- 指摘の位置は、最初に当たった後のステップの `run`。秘密情報は、当たった後のステップ全てから探す

## 理由

- コマンドを動かす前提がなければ、秘密情報を持つステップは作業場所のコードを読まない(`curl` で API を呼ぶだけなど)。選択肢 2 は誤検知が多い
- 知られたコマンドに絞ると、知らないコマンドは数えない側に倒れる。見逃しは出るが、指摘の正しさを優先する。ADR 0025 の許す形の方式と逆向きなのは、こちらは指摘を出す側の判定だから
- 他のジョブや Action は、作業場所を受け継ぐ経路が多く、件数も分からない(選択肢 3)
- エージェントに秘密情報を渡さない設計をしても、後のステップで抜ける。調査で 37 ファイルが今の判定に現れていない(選択肢 4)

## 受け入れたトレードオフ

- 一覧にないコマンド(`deno`、`php`、独自のビルド道具など)、`uses:` の Action が動かすコード、他のジョブは数えない
- `npm publish` のように、エージェントの書いたコードを動かさず公開するだけのコマンドも数える。公開物そのものがエージェントの書いたものになるため
- claude-code-action で `--disallowedTools` により Edit と Write を禁じても、書き換えられるとみなす。Bash で書き換える道は残ることがあり、確かめていない
- エージェントの後に `git checkout` や `git clean` で作業場所を戻す形は読まない

## 再検討する条件

- 一覧にないコマンドの見逃しが調査で多いと分かったとき
- 各 Action の既定の権限が変わったとき
