# 0035. `--format github` で GitHub の注釈を出し、Action は注釈とジョブの要約に指摘を出す

- ステータス: 承認
- 日付: 2026-11-15

## 文脈

- Action は平文と SARIF で sekimori を動かす(ADR 0017)。平文の指摘はログにしか出ないので、見るにはログを開く必要がある。SARIF はコードスキャンに送れるが、fork の PR では送れない(ADR 0019)
- GitHub Actions の workflow command `::error file=...,line=...,col=...,title=...::本文` は、ログの中で注釈になり、PR の「Files changed」の該当する行にも出る。actions/toolkit(`packages/core/src/command.ts`、`utils.ts`)で、次のことを確かめた
  - 項目は `title`、`file`、`line`、`endLine`、`col`、`endColumn`
  - 本文は `%`、`\r`、`\n` を `%25`、`%0D`、`%0A` に、項目の値はさらに `:` と `,` を `%3A`、`%2C` に置き換える
- `$GITHUB_STEP_SUMMARY` に書いた Markdown は、実行の要約の画面に出る

## 検討した選択肢

1. CLI に `--format github` を足し、指摘ごとに注釈を 1 行出す。Action は平文の代わりにこれで動かし、平文の指摘をジョブの要約にも書く
2. Action の中で、平文の出力を shell で注釈に組み直す
3. 何もしない

## 決定

高橋さんが判断を任せ(2026-11-15 に「止まらないで。開発改善を進めて」)、Claude が 1 を選んだ。

- 注釈は `::error` か `::warning`(指摘の重さ)。`file` はリポジトリの根からの相対のパス(SARIF の `uri` と同じ求め方)、`line` と `col` は平文と同じ位置、`title` は `sekimori 規則の ID`、本文は平文の見出しと軸の内訳
- ADR 0033 の抑える指示の警告は、`--format github` では `::warning` の注釈として標準出力に出す
- 終了コードは平文と同じ
- Action は `--format github` で動かし(注釈と終了コード)、指摘があれば平文の出力を `$GITHUB_STEP_SUMMARY` に書き、SARIF も作る

## 理由

- 注釈なら、PR の差分の上で、どのステップが危ないかが見える。fork の PR でもログの注釈として出る
- shell で組み直すと、置き換えの規則とパスの求め方を 2 か所に持つことになる(選択肢 2)
- ジョブの要約は、ログを開かずに全体を見るための場所になる

## 受け入れたトレードオフ

- Action は sekimori を 3 回動かす(注釈、要約、SARIF)。検査は速いので、時間の差は小さい
- GitHub が画面に出す注釈の数には上限があり、指摘が多いと一部は要約と SARIF でしか見えない
- 同じ場所の指摘は、SARIF と違って 1 つの注釈にまとめず、規則ごとに別の注釈にする。規則ごとに直し方が違うため
- 絶対パスで渡したファイルは、リポジトリの中の位置が分からないので、`file` を付けずにログだけに出す
- 読めないワークフローは `::error title=sekimori::` の注釈にし、要約にも平文のエラーとして書く
