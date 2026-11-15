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
  - codex-action: `sandbox: read-only`、`permission-profile: ":read-only"`、`safety-strategy: read-only` のどれでもない
  - run-gemini-cli: 書く道具(`write_file` か `WriteFileTool`、`replace` か `EditTool`、範囲を絞らないシェルの `run_shell_command` か `ShellTool`。Gemini CLI の `config.ts` はクラス名でも照らす)のどれかが、`tools.core` に無いと止められておらず(`tools.core` が無ければ全て使える)、`tools.exclude` でも外されていない。`settings` が式などで読めないときは、書き換えられるとみなす
- コードを動かすコマンドは、`run` を ADR 0014 と同じくシェルの順で区切り、各コマンドの名前で決める。ヒアドキュメントの本文の行はコマンドとして読まない
  - 作業場所のファイルを直接動かす形。`./`、`$GITHUB_WORKSPACE`、`$PWD` で始まるパス、またはスクリプトの拡張子(`.sh`、`.py`、`.js`、`.ts`、`.rb`、`.pl` など)を持つ相対パス(`scripts/x.sh`)。拡張子の無い相対パスは、URL や `sed` の式と区別できないので数えない
  - シェルとインタプリタ(`bash`、`sh`、`zsh`、`python`、`node`、`ruby`、`perl`)にファイルか、`python -m` でモジュールを渡す形。スクリプトより前のフラグだけを読み、値を取るフラグ(`bash -o`、`python -W`、`ruby -I` など。`-Itest` のようにつなげた形も)の値は飛ばす。`bash < ./x.sh` も読む。その場のコードを渡すフラグ(`-c`、`-e`、`-0pe` のようにまとめた形も)、ヒアドキュメント(`<<`)、作業場所の外のパス(`/`、`~`、`$GITHUB_WORKSPACE` 以外の `$` で始まるもの)は除く
  - `sudo`、`timeout`、`env` などの包むコマンド(ADR 0013 の一覧)の中のコマンド。包むコマンドのフラグとその値(`sudo -u`、`nice -n` など)、変数の代入、`timeout` の時間を飛ばした最初の語で一度だけ決める。`command -v` は動かさないので除く
  - `if`、`then`、`do`、`!` などのシェルの予約語の後の語をコマンドの名前とする
  - `make`、`pytest`、`tox`、`nox`、`pre-commit`、`gradle`、`mvn`、`rake`、`yarn`、`pnpm`、`bun`(この 3 つは引数だけでも `package.json` のスクリプトを動かすので、部分コマンドを問わない)
  - 部分コマンドまで決めるもの(`cargo +nightly` のような版の指定と、`npm --prefix dir`、`cargo --manifest-path x` のような値を取るフラグは飛ばす): `npm` の `run`、`run-script`、`test`、`t`、`install`、`i`、`ci`、`it`、`install-test`、`exec`、`build`、`start`、`publish`、`rebuild`、`pack`。`cargo` の `test`、`build`、`run`、`publish`。`go` の `test`、`build`、`run`、`generate`。`dotnet` の `test`、`build`、`run`。`bundle exec`、`uv run`、`poetry run`
- 秘密情報は、ADR 0027 と同じく、そのステップ、ジョブ、ワークフローの `env` と、`run` の本文の `${{ secrets.* }}`(`GITHUB_TOKEN` を除く)
- 後のステップの `if:` が定数で偽なら数えない
- 秘密情報は、すでにエージェントに届くもの(エージェントの入力と、エージェントに届く `env` の全ての秘密情報。大文字と小文字は区別せず、全ての秘密情報が届くならどれも)を除いて探す。同じ秘密情報で指摘を重ねないため。除いた後に残る秘密情報を持つ最初の後のステップを指す
- 指摘の位置は、コードを動かすコマンドを持ち、秘密情報も届く最初の後のステップの `run`

## 理由

- コマンドを動かす前提がなければ、秘密情報を持つステップは作業場所のコードを読まない(`curl` で API を呼ぶだけなど)。選択肢 2 は誤検知が多い
- 知られたコマンドに絞ると、知らないコマンドは数えない側に倒れる。見逃しは出るが、指摘の正しさを優先する。ADR 0025 の許す形の方式と逆向きなのは、こちらは指摘を出す側の判定だから
- 他のジョブや Action は、作業場所を受け継ぐ経路が多く、件数も分からない(選択肢 3)
- エージェントに秘密情報を渡さない設計をしても、後のステップで抜ける。調査で 37 ファイルが今の判定に現れていない(選択肢 4)

## 受け入れたトレードオフ

- `npx` はレジストリの道具を動かすことが多いので数えない。作業場所の設定ファイル(`eslint.config.js` など)を読む道具もあるが、見逃す側に倒す
- 一覧にないコマンド(`deno`、`php`、独自のビルド道具など)、`uses:` の Action が動かすコード、他のジョブは数えない
- `npm publish` のように、エージェントの書いたコードを動かさず公開するだけのコマンドも数える。公開物そのものがエージェントの書いたものになるため
- claude-code-action で `--disallowedTools` により Edit と Write を禁じても、書き換えられるとみなす。Bash で書き換える道は残ることがあり、確かめていない
- エージェントの後に `git checkout` や `git clean` で作業場所を戻す形は読まない
- `"$GITHUB_WORKSPACE"/x.sh` のように変数だけを引用した形は、引用符で語が分かれるので読まない。区切り方は `pr-head-checkout` と共有していて、変えるとそちらにも響くため

## 再検討する条件

- 一覧にないコマンドの見逃しが調査で多いと分かったとき
- 各 Action の既定の権限が変わったとき
