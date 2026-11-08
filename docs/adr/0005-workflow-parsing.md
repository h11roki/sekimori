# 0005. ワークフローの YAML は zizmor の github-actions-models と yamlpath で読む

- ステータス: 承認
- 日付: 2026-11-08

## 文脈

- sekimori は、ワークフローの YAML からトリガー、ジョブの権限、ステップの `uses` と `with`、`env`、`if:` を読んで判定する(ADR 0001、0003)。結果は SARIF で出すので、指摘した設定の行と列が要る
- ワークフローの書き方には揺れが多い。`on:` は文字列、配列、マップのどれでも書け、`permissions` も `write-all` のような文字列とマップの両方で書ける
- Rust の YAML のクレートの事情(2026-11-08 に crates.io で確認)
  - serde_yaml は 2024-03 に開発を終えている(版に `+deprecated` が付いている)
  - zizmor は、ワークフローの型の github-actions-models と、YAML の中の位置を求める yamlpath を、zizmor のリポジトリから crates.io に出している。どちらも MIT で、1.32.0、2026-11-02 の更新。yamlpath は tree-sitter と tree-sitter-yaml を使う
  - saphyr(0.3.3、MIT か Apache-2.0)は、位置つきで読める汎用の YAML のクレート。yaml-rust2(0.13.0)は、最も使われている汎用の YAML のクレート

## 検討した選択肢

1. zizmor の部品(github-actions-models と yamlpath)を使う
2. saphyr で読み、ワークフローの型を自分で書く
3. yaml-rust2 で読み、ワークフローの型を自分で書く

## 決定

高橋さんが判断を任せ、Claude が 1 を選んだ。

- 判定はワークフローの型(github-actions-models)の上で行い、SARIF に出す位置は yamlpath で求める
- 2 つのクレートは同じ版に固定し(`=1.32.0` の形)、上げるときは変更点を読んでから上げる

## 理由

- `on:` や `permissions` の書き方の揺れを型が吸収するので、規則の実装に集中できる。2 と 3 では、この型と揺れの試験を自分で用意することになり、ADR 0002 で受け入れた Rust を覚える手間に重なって、最初の版が遅れる
- zizmor と同じ土台なので、ADR 0001 の「規則を zizmor に寄稿する」道がさらに近くなる
- yamlpath は、型で見つけた設定を YAML の中の行と列に戻せるので、GitHub のコードスキャンに正しい行を出せる

## 受け入れたトレードオフ

- 2 つのクレートは zizmor の版と一緒に出るので、zizmor の都合で API が変わりうる。版を固定し、上げるのを作業として行う
- tree-sitter は C で書かれているので、ビルドに C のコンパイラが要る。GitHub の runner と macOS には入っている
- 型と位置を別々に扱うので、型で見つけた設定から位置を引く手間が規則ごとにかかる

## 再検討する条件

- 版を上げるたびに、API の変更で手直しが続くとき
- zizmor が 2 つのクレートを crates.io に出さなくなったとき
- 型が読めない書き方のワークフローが、公開リポジトリで目立つとき
