# 0012. 起動の制限の判定を各社の実装に合わせ、`if:` は式のパーサーで読む

- ステータス: 承認(`needs:` の前のジョブを見ないことは 0026 で置き換え)
- 日付: 2026-11-12

## 文脈

- ADR 0006 は「起動の制限を外した」を各社の設定で決め、`if:` の式は `author_association`、`github.actor`、`github.triggering_actor` を比べているかで判断し、式の読み方は実装で決めるとした
- 最初の実装(PR #5)は、`if:` にこれらの名前が含まれていれば絞っているとみなした。セルフレビューで、`if: github.actor != 'github-actions[bot]'` のようなよくある否定や、`||` の片側だけの比較でも絞っているとみなし、誰でも起動できる設定を見逃すと分かった
- 各社のソースで確かめたこと(2026-11-12)
  - claude-code-action: `allowed_non_write_users` は、入力の `github_token` を渡したときだけ効く(`src/github/validation/permissions.ts` と、`action.yml` の説明の「Only works when github_token input is provided」)。`allowed_bots` が全ての bot を通すのは、値が丸ごと `*` のときだけで、カンマで区切った一覧の中の `*` は効かない
  - codex-action: `allow-users` が全ての人を通すのは、値が丸ごと `*` のときだけ(`src/checkActorPermissions.ts`)
  - GitHub Actions の入力の名前は、大文字と小文字を区別しない
- zizmor は、式を構文木に読む github-actions-expressions を、ADR 0005 の 2 つのクレートと同じ版(1.32.0、MIT)で crates.io に出している。文脈(`github.actor` など)を大文字と小文字を区別せずに比べられ、`github.event.*.author_association` のような形も使える

## 検討した選択肢

1. github-actions-expressions で式を構文木に読み、必ず成り立つ比較があるときだけ絞っているとみなす
2. 文字列の比べ方を細かくし、`!=` や `||` を個別に除く
3. 最初の実装のままにし、トレードオフとして受け入れる

## 決定

高橋さんが判断を任せ(2026-11-12 に「承認取らなくていい」)、Claude が 1 を選んだ。

- `if:` は `${{ }}` を外して github-actions-expressions で読み、外部の人が起こせるトリガーのイベントごとに、そのイベントで起動した人を必ず比べているかを見る。全てのイベントで比べているときだけ、起動した人を絞っているとみなす
  - 起動した人の値は、`github.actor`、`github.triggering_actor`、`github.event.sender.login`、そのイベントで起動した人が書いたものの `author_association`。issue_comment、pull_request_review_comment、discussion_comment では `github.event.comment`、issues では `github.event.issue`、pull_request_target では `github.event.pull_request`、pull_request_review では `github.event.review`、discussion では `github.event.discussion`。issue_comment で `github.event.issue.author_association` を比べても、issue を書いた人を見ているだけなので絞っていない
  - `a || b` を値として使うときは、そのイベントで実在する最初の値を取る。issue_comment では issue も comment も実在するので、`github.event.issue.author_association || github.event.comment.author_association` は issue を書いた人の値になる
  - 起動した人の値を `==` で定数と比べている。相手が `github.event.issue.user.login` のようなイベントの値なら、外部の人が決められるので絞っていない
  - `contains(fromJSON('[...]'), 起動した人の値)` のように、1 つ目の引数が定数の JSON の一覧。1 つ目が文字列や、文字列に読める JSON(`fromJSON('"alice,bob"')`)だと部分文字列の判定になり、`ali` でも通るので絞っていない
  - 比べる定数か一覧に、外部の人の `author_association`(`NONE`、`FIRST_TIMER`、`FIRST_TIME_CONTRIBUTOR`、`CONTRIBUTOR`、`MANNEQUIN`)があれば絞っていない。GitHub の式の文字列の比較は大文字と小文字を区別しないので、ここでも区別しない
  - `github.event_name == 'issues'` は、issues 以外のイベントでは成り立たないので、そのイベントでは動かない(絞っている)とみなす。`!=` はその逆。イベントごとに分けて絞る形(`(github.event_name == 'issues' && ...) || (github.event_name == 'issue_comment' && ...)`)を読める
  - `&&` は、どちらかの側が絞っていれば絞っている。`||` は、両側が絞っているときだけ絞っている
  - `false` は、動かないので絞っているとみなす
  - `!=`、`!`、他の関数は、絞っていないとみなす。式を読めないときも、絞っていないとみなす(指摘を出す側に倒す)
- `if:` で起動した人を絞っていれば、3 社のどれでも、起動の制限を外した設定は当てない。`allow-users: '*'` でも、ジョブの `if:` で 1 人に絞っていれば誰でもは起動できない
- 指摘の理由には、`if:` で絞れていないイベントだけを書く
- ジョブかステップの `if:` が定数として偽(`false`、`${{ false }}`)なら、そのステップは動かないので評価しない
- claude-code-action の `allowed_non_write_users: '*'` は、`github_token` を空でない値で渡しているときだけ当てる。`allowed_bots` は値が丸ごと `*` のときだけ当てる
- codex-action の `allow-users` は、値が丸ごと `*` のときだけ当てる
- `with` の入力の名前は、大文字と小文字を区別せずに探す
- github-actions-expressions は、ADR 0005 の 2 つと同じく `=1.32.0` に固定し、Dependabot の zizmor のグループに入れる

## 理由

- 1 は、否定、`||`、`${{ false }}`、文脈の大文字と小文字を、文字列の場合分けを足さずに正しく扱える。zizmor と同じパーサーなので、寄稿の道も保てる(ADR 0001、0005)
- 2 は、式の形が増えるたびに場合分けが要り、括弧や入れ子で読み違える
- 見逃しは、静的検査の道具として誤検知より重いので、3 は選ばない
- 各社の値の扱いは、文書ではなくソースで確かめた。文書の言い回しだけで規則を作ると、`github_token` の条件のような誤検知が出る

## 受け入れたトレードオフ

- `contains` 以外の関数(`startsWith` など)で起動した人を比べている式は、絞っていないとみなし、誤検知になりうる
- エージェントのジョブが `needs:` で待つ前のジョブの `if:` で絞る形(dispatch のジョブ)は見ないので、誤検知になる。run-gemini-cli の公式の例はこの形を再利用できるワークフローで書いていて、それは ADR 0006 で追わないと決めている
- workflow_run には起動した人の `author_association` が無いので、`github.actor` か `github.triggering_actor` の比較でしか絞れない
- zizmor のクレートが 3 つになり、版を上げる手間が少し増える

## 再検討する条件

- 公開リポジトリに当てて、`if:` の読み違いによる誤検知か見逃しが目立つとき
- 各社が、入力の値の扱い(`*` の意味、`github_token` の条件)を変えたとき(ADR 0003)
