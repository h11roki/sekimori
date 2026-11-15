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

- ビルドとテストのコマンドは、`npm`、`pnpm`、`yarn`、`bun`、`make`、`gradle`、`gradlew`、`mvn`、`mvnw`、`cargo`、`go`、`pytest`、`tox`、`nox`、`rake`、`bundle`、`composer`、`dotnet`、`mix`、`just`、`task`、`uv`、`poetry`、`pip`、`swift`、`xcodebuild`、`sbt`、`jest`、`vitest`、`phpunit`、`rspec`、`pre-commit` と、引数を付けたインタープリター(`python scripts/check.py` など)と、`./` か `../` で始まるリポジトリのスクリプト(`./build.sh` など)とする
- 取って動かすコマンドは、`npx`、`pnpx`、`bunx`、`uvx`、`pipx` と、`npm exec`、`npm x`、`npm init`、`npm create`、`pnpm dlx`、`pnpm exec`、`pnpm create`、`yarn dlx`、`yarn exec`、`yarn create`、`bun x`、`bun create`、`bun install`、`deno run`、`deno eval`、`pipx run`、`uv run`、`poetry run`、`go run`、`cargo run`、`dotnet run`、`bundle exec` とする。下位のコマンドの後に取ってくるものを決めた形(`Bash(uv run pytest:*)`、`Bash(bundle exec rspec:*)`)と、引数を決めて許した形(`Bash(npx tsx scripts/build.ts)`)は、リポジトリのコードを動かすものとして扱う。ただし決めたものがインタープリター(`Bash(uv run python:*)`、`Bash(npx tsx:*)`)なら、取って動かすものとする。`npm install`、`pnpm add`、`yarn add`、`bun add`、`pip install` も、引数でパッケージを選べる形は取って動かすものとする。コマンドの頭の `VAR=値` は読み飛ばす。`Bash(uv:*)` のように下位のコマンドを絞らない形も含む
- 書き換えられるかは次のとおり見る
  - claude-code-action: `prompt` の入力が無いか空か、`track_progress: true` の tag モードは、`--permission-mode` を `auto` か `acceptEdits` にするので、書き換えられる。`prompt` がある agent モードは、`auto` を既定にするので書き換えられる。ただし `allowed_non_write_users`、`allowed_bots`、`path_to_claude_code_executable` を設定したときは権限のモードを付けず、書き換えは拒否される(`src/modes/permission-mode.ts` の `defaultsToAutoMode`、2026-11-16 に `9fa6885` で確かめた)。`claude_args` でモードを選んでいれば、そのモードに従う。`settings` のモードは agent モードでだけ効く(tag モードは `claude_args` の前に `--permission-mode` を付けるので、`settings` より強い)
  - Claude Code の base action: 許可の一覧に範囲を絞らない `Edit`、`MultiEdit`、`Write` があるか、モードが `acceptEdits` か `auto` のとき
  - どちらも、範囲を絞らない `Edit`(`Edit`、`Edit(*)`、`Edit(**)` など)を拒否の一覧に入れていれば書き換えられないとみなす(Claude Code の `Edit` の規則は、ファイルを書き換える道具の全てに効く)
  - `agent-code-with-secrets`(ADR 0028)の書き換えの判定にも同じものを使う
  - run-gemini-cli: `tools.core` が無いか、`write_file` か `replace` を許すとき(ADR 0028 の `gemini_may_edit` と同じ)
- `--permission-mode auto` だけでは数えない。auto モードは、許可の一覧に無い呼び出しを安全の審査で通すかを決めるもので、境界ではない(claude-code-action の `docs/security.md`)が、tag モードの既定なので、数えるとほとんどの claude-code-action が指摘になる。別に決める

## 理由

- 書き換えとビルドのコマンドが揃うと、エージェントが動かすコードを選べるので、範囲の無いシェルと同じことができる
- 取って動かすコマンドは、それだけでインタープリターを引数ごと許すのと同じ
- 書き換えられないエージェントでビルドのコマンドを許すのは、リポジトリのコードを動かすだけで、勧めてよい形として残せる

## 受け入れたトレードオフ

- 指摘が増える。書き換えを許す claude-code-action で `Bash(npm test:*)` を許す形は、全て `broad-tools` になる。README の勧め方を、書き換えの無い読むだけの仕事に絞るか、決まったスクリプトを動かす形に変える
- 一覧に無いビルドの道具と、`scripts/build.sh` のように `./` で始まらないリポジトリのスクリプトは数えない
- 引数を決めずに許したビルドの道具(`Bash(make:*)`、`Bash(go test:*)`)は、`make --eval` や `go test -exec` のような引数で任意のコマンドを動かせるが、書き換えが無いときは数えない
- `sed -i`、`tee`、`git apply` のように、シェルのコマンドでファイルを書き換える許可は、書き換えとみなさない
- `Bash(timeout 600 npm test)` のように、引数を決めた形でラッパーを前に付けたものは読まない
- `uv pip install` や `python -m pip install` のように、2 段目の下位のコマンドで取ってくる形は、リポジトリのコードを動かすものとして扱う
- 範囲を絞った `Edit(docs/**)` は書き換えとみなさない。絞った範囲がコードを含んでいても数えない
- 古い版に固定した claude-code-action は、今の版と既定の権限のモードが違うことがある(公開ワークフローの判定では、agent モードの古い版で書き換えが拒否される例が 1 件あった)。sekimori は今の版の動きで判定する
- `ANTHROPIC_BASE_URL` を `env` に置いて auto モードにならない形は、読んでいない
- Codex は、サンドボックスの中で任意のコマンドを動かせるので、コマンドの一覧が無く、この規則の対象にならない
