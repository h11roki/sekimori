# 0012. 起動の制限の判定を各社の実装に合わせ、`if:` は式のパーサーで読む

- ステータス: 承認
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

- `if:` は `${{ }}` を外して github-actions-expressions で読み、次のどれかのときだけ、起動した人を絞っているとみなす
  - 起動した人の文脈(`github.actor`、`github.triggering_actor`、`github.event.*.author_association`)を `==` で比べている
  - `contains(一覧, 起動した人の文脈)` で、2 つ目の引数に起動した人の文脈がある
  - `&&` は、どちらかの側が絞っていれば絞っている。`||` は、両側が絞っているときだけ絞っている
  - `false` は、動かないので絞っているとみなす
  - `!=`、`!`、他の関数は、絞っていないとみなす。式を読めないときも、絞っていないとみなす(指摘を出す側に倒す)
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

- `author_association == 'NONE'` のような、外部の人を通す比較も絞っているとみなす
- `contains` 以外の関数(`startsWith` など)で起動した人を比べている式は、絞っていないとみなし、誤検知になりうる
- zizmor のクレートが 3 つになり、版を上げる手間が少し増える

## 再検討する条件

- 公開リポジトリに当てて、`if:` の読み違いによる誤検知か見逃しが目立つとき
- 各社が、入力の値の扱い(`*` の意味、`github_token` の条件)を変えたとき(ADR 0003)
