# 0025. `prompt` を渡さない run-gemini-cli は、作業場所が既定のブランチのままなら、起動できる人の軸を数えない

- ステータス: 承認
- 日付: 2026-11-13

## 文脈

- ADR 0007 は、起動できる人の軸を、外部の人が起動できるなら 1、起動は絞っていても外部の人の文面がエージェントに届くなら 0.5 とした。どちらも、外部の人の文面がエージェントに届くことを前提にしている
- 調査(docs/survey/2026-11.md)では、`error` から選んだ 15 件のうち 2 件が、エージェントに外部の人の文面が届かない形の誤検知だった。`prompt` を渡していないか、入力の名前を間違えている
- PR #19 の後に同じリポジトリのファイルをまとめて検査すると、`unrestricted-trigger` が `error` のファイル 50 件のうち 9 件で、run-gemini-cli のステップに `prompt` の入力が無かった。調査の全てのファイルでは、`prompt` の無い run-gemini-cli のステップが 34 あり、代わりに `command`、`custom_prompt`、`prompt-file` など、run-gemini-cli に無い名前を渡しているものがあった
- run-gemini-cli の `action.yml` で確かめたこと(2026-11-13、main と v0.1.3、v0.1.5、v0.1.15、v0.1.20、v0.1.22)
  - `prompt` の既定は `You are a helpful assistant.` で、どの版でも同じ
  - CLI は `gemini --yolo --prompt "${PROMPT}"` で動き、`PROMPT` は `inputs.prompt` だけから作る。ステップの `env` の `PROMPT` は、中のステップの `env` で上書きされる
  - `github_issue_number` と `github_pr_number` は、イベントの番号を既定に持つが、使い道は遠隔測定の記録とインストールの環境変数だけで、本文は読まない
  - GitHub は、Action に無い名前の入力を警告するだけで、Action には渡さない
- Gemini CLI は、作業場所の `.gemini/settings.json` と、作業場所とその下のディレクトリの `GEMINI.md` を読む。`issues`、`issue_comment`、`discussion`、`discussion_comment` では、既定の checkout は既定のブランチで、外部の人は書けない。PR の head をどこかのディレクトリに取り出したときだけ、外部の人が書いたものが混ざる。`pull_request_review` と `pull_request_review_comment` では、既定の checkout が PR の merge ref を取り出す。`workflow_run` では、PR の実行の成果物を展開する形がある。ADR 0014 は、`.gemini/settings.json` を読むのは作業場所を信頼したときだけと確かめて `pr-head-checkout` を当て、`GEMINI.md` のような指示のファイルは起動できる人の軸で数えるとした。`GEMINI.md` を信頼の無い作業場所でも読むかは、確かめていない
- codex-action は、`prompt` も `prompt-file` も無いと `codex exec` を動かさない。claude-code-action は、`prompt` が無いとコメントを読んで動く。この 2 つは今回の対象にしない

## 検討した選択肢

1. run-gemini-cli のステップに `prompt` の入力が無く、作業場所が既定のブランチのまま(上の 4 つのイベントだけで動き、前のステップで PR の head を取り出していない)なら、起動できる人の軸を数えない
2. 1 と同じ形で、起動できる人の軸を 0.5 に下げる
3. プロンプトの文に `github.event` などの外部の値が含まれないときも、軸を数えない

## 決定

高橋さんが判断を任せ(2026-11-13 に「承認を求めないでどんどん続けていい」)、Claude が 1 を選んだ。

- run-gemini-cli のステップで、`with` に `prompt` の入力(大文字と小文字を区別しない)が無いか、値が空白だけなら、エージェントには決まったプロンプトだけが届くとみなす
- そのうえで、外部の人が起こせるイベントが `issues`、`issue_comment`、`discussion`、`discussion_comment` だけで、同じジョブの前のステップが PR の head を取り出していなければ、起動できる人の軸を数えず、`unrestricted-trigger` も出さない
  - `actions/checkout` は、`path` を問わず、ADR 0014 で base とみなす取り出し(base のリポジトリの、読める `ref`)でなければ、PR の中身を取り出しうるとみなす。`steps.pr.outputs.sha` のような正体の分からない `ref` や、`with` を式で丸ごと渡したものも含む。下のディレクトリの `GEMINI.md` も読まれるため
  - `run` は、ADR 0014 の PR の head の checkout に当たるか、PR の head のコミットを指す値(`pull/<番号>/head` など)が書かれていれば、PR の中身を取り出しうるとみなす。`git worktree add` や `git clone` で下のディレクトリに取り出す形を拾うため
  - 取り出しうるときは、これまでどおりに数え、理由に、そのステップが PR から持ち込みうる `GEMINI.md` が届くことを書く。作業場所の信頼や `settings` は問わない
- 他のイベント(`pull_request_target`、`pull_request_review`、`pull_request_review_comment`、`workflow_run`)を含むときは、作業場所に PR の中身が入りうるので、これまでどおりに数える
- `with` を式で丸ごと渡すなど、入力を読めないときは、これまでどおり文面が届くとみなす

## 理由

- 起動できる人の軸は、外部の人がエージェントに指示を混ぜられるかを測るもので、ADR 0007 の 1 も 0.5 も文面が届くことが前提になっている。文面が届かないなら、どちらにも当たらない
- 2 は、0.5 を「起動は絞っているが文面は届く」と決めた ADR 0007 の意味と食い違う
- 3 は、run-gemini-cli の公式の例のように、プロンプトを `/gemini-review` のような作業場所のコマンドにし、そのコマンドが環境変数から issue の本文を読む形を見逃す。プロンプトに外部の値が無くても、エージェントが自分で issue を読みに行く指示もありうる
- PR の head を取り出すと、外部の人が書いた `GEMINI.md` がプロンプトに加わりうるので、その場合は数え続ける。信頼の無い作業場所で `GEMINI.md` を読むかは確かめていないので、指摘を出す側に倒し、`pr-head-checkout` の条件(根への取り出し、信頼と `settings`)は使わない
- 作業場所に PR の中身が入りうるイベントを、取り出し方ごとに読み分けるのは、成果物の展開のように経路が多く、見逃しやすい。既定のブランチのままと言えるイベントだけを軽くする

## 受け入れたトレードオフ

- 作業場所の `GEMINI.md` が、外部の人の issue を読みに行くよう指示している形は見逃す。作業場所は書き込める人が書いたものなので、その指示も書き込める人の判断とみなす
- 前のステップが issue の本文をファイルに書き出し、それを `GEMINI.md` として置くような形は見逃す
- `pull_request_target` などで `prompt` を渡さず、PR の中身も取り出さないワークフローは、これまでどおり数えるので誤検知が残る
- 正体の分からない `ref` の checkout があると、PR を取り出していなくても数えるので、誤検知が残る
- `pull_request` を一緒に持つワークフローでも軽くする。`pull_request` は、これまでどおり外部の人が起こせるトリガーに数えない(フォークの PR の実行には秘密情報が渡らない)ので、その実行の作業場所は見ない
- `prompt` を渡していない設定は、多くが入力の名前の間違いで、エージェントは利用者の意図どおりに動いていない。sekimori はその間違いを知らせない
- 外部の人がエージェントを起動して API の料金を使わせる経路は、起動できる人の軸に数えない

## 再検討する条件

- run-gemini-cli が、`prompt` 以外から外部の文面を読む入力(issue の番号から本文を読むなど)を足したとき
- codex-action や claude-code-action の、プロンプトの無いときの動きが調査で誤検知の原因として目立ったとき
