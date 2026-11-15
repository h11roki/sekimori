# 0041. ビルドとテストのコマンドは、書き換えと組むと広い道具として数え、パッケージを取って動かすコマンドは単独で数える

- ステータス: 承認
- 日付: 2026-11-16

## 文脈

- `broad-tools` は、範囲の無いシェル、シェルやインタープリターを引数ごと許す形(`Bash(python *)` など)、権限の確認を飛ばす形を数える(ADR 0006 ほか)。`Bash(npm test:*)` のようにコマンドを絞った形は数えない。README も、これを範囲を絞った安全な例として勧めている
- `npm test`、`make`、`./gradlew build`、`cargo test`、`pytest` などは、リポジトリにある定義(`package.json` の scripts、`Makefile`、`build.gradle`、`build.rs`、`conftest.py` など)をそのまま動かす。エージェントがファイルを書き換えられるなら、定義を書き換えてから実行すれば、任意のコードを動かせる
- `npx`、`uvx`、`pnpm dlx`、`uv run`、`go run`、`cargo run` などは、書き換えが無くても、引数で選んだパッケージやスクリプトを取ってきて動かせる
- 公開ワークフローの調査(2,065 ファイル)で、Claude にビルドやパッケージ管理のコマンドを許すステップは 108 件あった。うち 77 件は何も指摘されていない。77 件のうち 38 件は `Edit` か `Write` を設定で許している
- claude-code-action の tag モードは、`--permission-mode` を `auto` か `acceptEdits` にして、作業場所の書き換えを既定で許す(`src/modes/tag/index.ts`、2026-11-16 に `9fa6885` で確かめた)

## 検討した選択肢

1. ビルドとテストのコマンドは、エージェントがファイルを書き換えられるときに `broad-tools` として数える。パッケージやスクリプトを取って動かすコマンドは、書き換えが無くても数える
2. パッケージやスクリプトを取って動かすコマンドだけを数える
3. 規則は変えず、README の制限と勧め方だけを直す

## 決定

高橋さんが 1 を選んだ(2026-11-15)。

- ビルドとテストのコマンドは、`npm`、`pnpm`、`yarn`、`bun`、`make`、`gradle`、`gradlew`、`mvn`、`mvnw`、`cargo`、`go`、`pytest`、`tox`、`nox`、`rake`、`bundle`、`composer`、`dotnet`、`mix`、`just`、`task`、`uv`、`poetry`、`pip`、`swift`、`xcodebuild`、`sbt`、`jest`、`vitest`、`phpunit`、`rspec` と、`./` か `../` で始まるリポジトリのスクリプト(`./build.sh` など)とする
- 取って動かすコマンドは、`npx`、`pnpx`、`bunx`、`uvx`、`pipx` と、`npm exec`、`npm x`、`pnpm dlx`、`pnpm exec`、`yarn dlx`、`yarn exec`、`uv run`、`poetry run`、`go run`、`cargo run`、`dotnet run`、`bundle exec` とする。`Bash(uv:*)` のように下位のコマンドを絞らない形も含む
- 書き換えられるかは次のとおり見る
  - claude-code-action: 常に書き換えられるとみなす(tag モードの既定。ADR 0028 と同じ)
  - Claude Code の base action: 許可の一覧に `Edit`、`MultiEdit`、`Write` があるか、`acceptEdits` か、すでに広い道具があるとき
  - run-gemini-cli: `tools.core` が無いか、`write_file` か `replace` を許すとき(ADR 0028 の `gemini_may_edit` と同じ)
- `--permission-mode auto` だけでは数えない。auto モードは、許可の一覧に無い呼び出しを安全の審査で通すかを決めるもので、境界ではない(claude-code-action の `docs/security.md`)が、tag モードの既定なので、数えるとほとんどの claude-code-action が指摘になる。別に決める

## 理由

- 書き換えとビルドのコマンドが揃うと、エージェントが動かすコードを選べるので、範囲の無いシェルと同じことができる
- 取って動かすコマンドは、それだけでインタープリターを引数ごと許すのと同じ
- 書き換えられないエージェントでビルドのコマンドを許すのは、リポジトリのコードを動かすだけで、勧めてよい形として残せる

## 受け入れたトレードオフ

- 指摘が増える。書き換えを許す claude-code-action で `Bash(npm test:*)` を許す形は、全て `broad-tools` になる。README の勧め方を、書き換えの無い読むだけの仕事に絞るか、決まったスクリプトを動かす形に変える
- 一覧に無いビルドの道具と、`scripts/build.sh` のように `./` で始まらないリポジトリのスクリプトは数えない
- Codex は、サンドボックスの中で任意のコマンドを動かせるので、コマンドの一覧が無く、この規則の対象にならない
