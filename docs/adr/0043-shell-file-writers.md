# 0043. ファイルを書き換えるシェルのコマンドを許すことも、書き換えとして数える

- ステータス: 承認(`sed`、`rsync`、`git pull`、`git clone` は 0050 で広い道具にし、書き換えの一覧から外した)
- 日付: 2026-11-16

## 文脈

- ADR 0041 で、ビルドとテストのコマンドは、エージェントが書き換えられるときに `broad-tools` とした。書き換えられるかは、`Edit`、`MultiEdit`、`Write` の許可と権限のモード(Claude)、`write_file` と `replace`(Gemini)だけで見ている
- `Bash(sed -i:*)`、`Bash(tee:*)`、`Bash(cp:*)`、`Bash(git apply:*)` のような許可でも、`package.json` やテストを書き換えられる。これとビルドのコマンドを組めば、`Edit` を許すのと同じことができる
- ADR 0041 のトレードオフとして、この形を数えていないと残した

## 検討した選択肢

1. ファイルを書き換えるシェルのコマンドを許すことも、書き換えとして数える
2. 今のまま、ファイルを書き換える道具だけを数える

## 決定

高橋さんが判断を任せ(2026-11-16 に「全部進めてください」)、Claude が 1 を選んだ。

- 書き換えとみなすコマンドは、`tee`、`cp`、`mv`、`install`、`rsync`、`patch`、`dd`、`truncate`、`sed`、`curl`、`wget` と、`git` の `apply`、`am`、`checkout`、`switch`、`pull`、`merge`、`cherry-pick`、`clone`、`reset`、`restore`、`stash`、`rebase`、`revert`、`worktree`、`submodule`、`config`、`gh` の `pr checkout`、`run download`、`release download`、`repo clone`、`npm pkg`、`npm config`、`pnpm pkg`、`pnpm config`、`yarn config`(下位のコマンドを決めない `Bash(git:*)` や `Bash(gh pr:*)` も含む。`-C`、`-c`、`--git-dir`、`--work-tree`、`gh` の `-R`、`npm` の `--prefix` などの値は読み飛ばす)。`Bash(sed -n:*)` のように頭を決めても、後ろに `-i` を足せるので `sed` は全て数える。`curl` と `wget` は外の中身をファイルに書け、`gh pr checkout` や `git pull` は外のコードを作業場所に持ち込める
- 引数を決めて許した形(`Bash(cp a.txt b.txt)`)は、書くファイルが決まっているので数えない
- これらの許可は、`Edit` の許可とは別に持つ。`Edit` を拒否しても止まらず、シェルの拒否で止まる。拒否は Claude Code と同じく頭で合わせる(`Bash(sed:*)` の拒否は `Bash(sed -i:*)` の許可を止める)。この直しで、ほかのシェルの許可と拒否の照らし合わせも頭で合わせるようになった
- 指摘の文には、どの許可で書き換えられるかを書く(Claude だけ。Gemini は書かない)
- `Bash(git:*)` と、`git submodule`、`git rebase`、`git bisect` を引数ごと許す形は、`-c alias.x=!cmd`、`submodule foreach`、`rebase --exec`、`bisect run` で任意のコマンドを動かせるので、書き換えが無くても ADR 0041 の取って動かすコマンドと同じく数える
- Claude の許可と、Gemini の `tools.core` の `run_shell_command` の両方で読む。Gemini の `tools.exclude` に、同じ道具か頭が合う書き方があれば数えない。`agent-code-with-secrets`(ADR 0028)の書き換えの判定にも効く

## 理由

- 書き換えの道具を許さなくても、これらのコマンドでコードを変えられる
- 一覧は、引数で選んだファイルの中身を変えるか作るコマンドに絞り、`touch` や `mkdir` のように中身を書かないものは入れない

## 受け入れたトレードオフ

- `git stash list` や `git worktree list` のように、書き換える下位のコマンドの後に読むだけの語を決めた形も数える

- シェルのリダイレクト(`echo x > file`)は、許可の一覧の形からは読めないので数えない
- 一覧に無いコマンド(`perl -i` は引数を許すインタープリターとして別に数える)は数えない
