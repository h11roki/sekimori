# 0013. 広い道具は、各社のソースで確かめた設定で決め、読めない設定は当てない

- ステータス: 承認
- 日付: 2026-11-12

## 文脈

- ADR 0006 は、規則「広い道具」(`broad-tools`)で許した道具の軸を満たすとし、claude-code-action の `--allowedTools` か `settings` の `permissions.allow` の範囲を絞らない `Bash`、codex-action の `sandbox: danger-full-access` か `safety-strategy: unsafe`、run-gemini-cli の `tools.core` が未設定か範囲を絞らないシェル、を挙げた
- ADR 0012 の教訓で、各社の値の扱いは文書だけでなくソースで確かめる
- 確かめたこと(2026-11-12)
  - claude-code-action(d5f7970): `claude_args` は shell と同じ規則で区切り(shell-quote)、`#` で始まる行を除く。`--allowedTools` と `--allowed-tools` は続く値を全て集め、カンマで分ける(`src/modes/agent/parse-tools.ts`)。他の引数は Claude Code にそのまま渡る(`base-action/src/parse-sdk-options.ts` の `extraArgs`)ので、`--dangerously-skip-permissions` と `--permission-mode bypassPermissions` も効く。`settings` は JSON の文字列か、設定ファイルのパス(`docs/configuration.md`)
  - Claude Code の道具の書き方は `Bash` で全てのコマンド、`Bash(gh issue view:*)` のように括弧で範囲を絞る
  - codex-action(13e9c17): `sandbox` は `workspace-write`、`read-only`、`danger-full-access`。`permission-profile` の組み込みは `:read-only`、`:workspace`、`:danger-full-access` の 3 つで、`:danger-full-access` はサンドボックスの制限を外す(Codex の permissions の文書)。名前付きの独自の profile は `codex-home/config.toml` に書く。`safety-strategy: unsafe` 以外では、権限を広げる `codex-args` を拒む(`src/runCodexExec.ts`)
  - run-gemini-cli(387c8dd): `settings` が空でなければ `.gemini/settings.json` に書き、Gemini CLI を常に `--yolo` で動かす。Gemini CLI(aeb6c04)の `tools.core` は組み込みの道具の許可リストで、既定は未設定。シェルは `run_shell_command`(`ShellTool` とも書く)で、`run_shell_command(git)` のように括弧で範囲を絞る

## 検討した選択肢

1. ワークフローに書かれた値だけで決め、パスや式のように読めない値は当てない
2. 1 に加え、`settings` のパスや `.gemini/settings.json` をリポジトリから読む
3. 読めない値は広い道具とみなす

## 決定

高橋さんが判断を任せ(2026-11-12 に「承認取らなくていい」)、Claude が 1 を選んだ。各社で次のどれかに当たれば、広い道具とする。

- claude-code-action
  - 許可の一覧は、`claude_args` の `--allowedTools` か `--allowed-tools`(`=` でつないだ形も含む)の値、`settings` と `claude_args` の `--settings` の JSON の `permissions.allow`、`@beta` や `@v0` の入力の `allowed_tools`。値はカンマか空白で区切る。括弧の中では区切らない(base-action はカンマで分けた値を SDK に渡し、Claude Code の CLI は空白でも区切るため)。`claude_args` は action と同じく、`#` で始まる行を除き、行の途中の `#` から後ろは文字列の最後まで捨てる(shell-quote は途中の `#` から後ろを全てコメントにする)。引用符とバックスラッシュの中の `#` は捨てない。引用符の対応が取れずに区切れないときは、shell-quote と同じく止まらずに、空白で区切って読む
  - 許可の一覧に、どのコマンドでも動かせる `Bash` がある。`Bash` だけ、括弧の中が `*` と `:` だけのもの(`Bash(*)` など)、ワイルドカードの前が、シェルやインタプリタ(`bash`、`python`、`node`、`powershell`、`php` など)の名前と `-` で始まるフラグだけのもの(`Bash(bash:*)`、`Bash(bash -c:*)`、`Bash(python *)` など)、または別のコマンドを動かす包み(`env`、`sudo`、`xargs`、`timeout`、`nice` など)で始まるもの(`Bash(timeout 600:*)`、`Bash(sudo -u root:*)` など)。名前は、パス(`/bin/bash`)と末尾の版(`python3.12`)を除いて比べる。`Bash(bun test:*)` や `Bash(python -m pytest:*)` のように、プログラムや部分コマンドまで決めたものは範囲を絞った許可とする
  - ただし、`--disallowedTools`、`--disallowed-tools`、`permissions.deny`、`disallowed_tools` のどれかで、`Bash` を丸ごと禁じているか、その許可と同じ書き方を禁じていれば当てない。Claude Code では禁止が許可より先に効くため。`permissions.ask` も、確認できない CI では拒まれるので、禁止と同じに扱う
  - `claude_args` に `--dangerously-skip-permissions` がある、または効いている権限のモードが `bypassPermissions`。モードは `claude_args` の `--permission-mode` の最後の値、無ければ `permissions.defaultMode`。禁止の一覧とは組み合わせずに当てる
  - ただし、`permissions.disableBypassPermissionsMode` が `disable` なら、権限の確認を外す設定は効かないので当てない
- codex-action: `sandbox: danger-full-access`、`permission-profile: ":danger-full-access"`、`safety-strategy: unsafe` のどれか
- run-gemini-cli: `settings` が無い、または JSON として読めて `tools.core` が無い。または `tools.core` に、範囲を絞らない `run_shell_command` か `ShellTool`、またはインタプリタの名前と `-` で始まるフラグだけに絞ったもの(`run_shell_command(bash)` など)がある。Gemini CLI は前置きで照らすので、インタプリタの名前だけなら何でも渡せる。`settings` は、Gemini CLI(strip-json-comments)と同じく `//` と `/* */` のコメントだけを除いてから読む。`tools.exclude` でシェルを外していれば、`tools.core` のシェルは当てない
- 1 つのステップで複数に当たっても、指摘は 1 つの設定を指し、理由にはその設定を書く。claude は権限の確認を外す設定、次に `claude_args`、`settings`、`allowed_tools` の順で最初に `Bash` を許した設定。codex は `sandbox`、`permission-profile`、`safety-strategy` の順
- 値が式(`${{ vars.SETTINGS }}`)、パス、JSON として読めない文字列のとき、`with:` 全体が式のときは、当てない

## 理由

- ワークフローの中だけで決まるので、どこで動かしても同じ結果になり、利用者が理由をワークフローの中で確かめられる
- 2 は、sekimori がワークフローのファイルしか受け取らない今の作り(ADR 0010)では、リポジトリの根を推測することになる
- 3 は、ほとんどの `settings: ${{ ... }}` を指摘し、使われなくなる

## 受け入れたトレードオフ

- `settings` をパスで渡す claude-code-action と、チェックアウトした `.gemini/settings.json` に頼る run-gemini-cli は見ない。後者は `settings` が無いので広い道具と判定し、誤検知になりうる
- codex-action の名前付きの独自の profile は、`config.toml` を読まないので見ない
- `--permission-mode auto` は、許可リストの外の道具を Claude Code の安全の確認で通すが、確認があるので広い道具とはみなさない
- Gemini CLI の `tools.exclude` でシェルを外していても、`tools.core` が無ければ他の組み込みの道具が全て使えるので、広い道具とみなす
- `Bash(npm:*)`、`Bash(npx:*)`、`Bash(git:*)` のようなパッケージの管理やバージョン管理の道具も、使い方しだいで任意のコードを動かせるが、範囲を絞った許可とみなす。インタプリタは、何でも渡すための道具なので広い道具に含める
- `@v1` で使われない `allowed_tools` を残していると、効かない設定を指摘することになる
- `--allowedTools` の値は、action と同じく次の `--` で始まる引数まで集める。`-p` のような 1 つの `-` の引数が続くと、それも道具の名前として読む。別のフラグの値が `--` で始まると、action と同じくフラグとして読む
- インタプリタと包みの一覧は、よく使われるものを並べたもので、全てではない
- Gemini CLI の古い形の `coreTools` は読まない。今の Gemini CLI(aeb6c04)は `tools.core` だけを読み(`config.ts`)、古い形を移す処理も無いため

## 再検討する条件

- `settings` をパスで渡すワークフローや、チェックアウトした `.gemini/settings.json` に頼るワークフローが、公開リポジトリで目立つとき
- 各社が、道具の書き方や既定を変えたとき(ADR 0003)
