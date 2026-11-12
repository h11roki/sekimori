# 0008. CI は Rust の版と Action を固定し、Dependabot で週に 1 回上げる

- ステータス: 承認
- 日付: 2026-11-12

## 文脈

- ADR 0002 は、Rust の安定版を rustup で入れ(2026-11-08 時点で 1.99.1)、CI で clippy と rustfmt を流すと決めた。clippy は警告でも CI を落とす(`-D warnings`)
- Rust は約 6 週ごとに新しい版が出て、clippy の既定の lint が増えることがある
- sekimori はワークフローの安全性を検査する道具なので、自分の CI も、Action を SHA で固定し、権限を `contents: read` に絞り、checkout に認証情報を残さない形にした(zizmor 1.32.0 で指摘なし)。SHA で固定すると、修正版が出ても自動では上がらない
- ADR 0005 は、github-actions-models と yamlpath を同じ版(`=1.32.0`)に固定し、変更点を読んでから上げると決めた
- Dependabot(2026-11-12 に GitHub の文書で確認)
  - 版の更新は、`github-actions`、`cargo`、`rust-toolchain`(rust-toolchain.toml の `channel = "1.xx.yy"`)に対応している
  - `cooldown` の `default-days` で、出てから決めた日数がたった版だけを提案させられる。3 つとも対応していて、設定しなければ 3 日。`cooldown` は版の更新にだけ効き、脆弱性の修正版への PR(security updates)には効かない
  - このリポジトリでは、脆弱性の通知(alerts)と security updates が、どちらも設定で無効だった

## 検討した選択肢

Rust の版

1. rust-toolchain.toml で 1.99.1 に固定する
2. stable に追従する

Action とクレートの更新

1. Dependabot で両方を上げる
2. Dependabot で Action だけを上げ、クレートは手で上げる
3. Dependabot を入れず、どちらも手で上げる

脆弱性の修正版

1. alerts と security updates を両方有効にする
2. alerts だけ有効にする
3. どちらも無効のままにする

## 決定

高橋さんが、版は 1、更新は 1、脆弱性の修正版は 1 を選んだ。

- rust-toolchain.toml で 1.99.1 に固定し、clippy と rustfmt も入れる。手元と CI が同じ版を使う。ADR 0002 の「安定版を rustup で入れる」は保ったまま、どの版を使うかを固定するもので、ADR 0002 を覆すものではない
- CI は rustup でこのファイルの版を入れ、`cargo fmt --check`、`cargo clippy --all-targets --locked -- -D warnings`、`cargo test --locked` を流す。外部の toolchain の Action は使わない。Cargo.lock はリポジトリに含める
- Dependabot で `github-actions`、`cargo`、`rust-toolchain` を週に 1 回見て、`cooldown` を 7 日にする
- 選択肢を出した時点では Rust の版は手で上げるつもりだったが、Dependabot が rust-toolchain.toml に対応していると分かったので、3 つ目として含めた
- `cargo` では github-actions-models と yamlpath を 1 つのグループにまとめ、1 つの PR で同じ版に上げる。ADR 0005 の固定はそのまま保ち、PR が来たら zizmor の変更点を読んでからマージする
- リポジトリの設定で alerts と security updates を有効にした

## 理由

- 版を固定すると、clippy の新しい lint で関係の無い PR の CI が落ちない。新しい lint は版を上げる PR にまとめて出るので、直す範囲がはっきりする
- Action を SHA で固定するなら、上げる仕組みとセットでないと古い版に留まる。Dependabot は GitHub に組み込まれていて、外部のサービスを足さずに済む
- ランナーには rustup が入っているので、外部の toolchain の Action に頼らず、依存する Action を checkout の 1 つに保てる
- `cooldown` を 7 日にすると、乗っ取られて出た版が見つかるまでのあいだに、それを取り込みにくくなる。security updates は `cooldown` を待たないので、脆弱性の修正版はすぐに PR になる

## 受け入れたトレードオフ

- 版を上げる PR が週に何本か来る。zizmor のクレートは版が頻繁に出るので、`cargo` の PR は多めになる
- 新しい Rust の lint や改善に気づくのが、Dependabot の PR が来るまで遅れる
- 脆弱性として登録されていない修正は、`cooldown` の 7 日のあいだ上がらない
- ビルドのキャッシュは持たない。依存がまだ無いので効かず、キャッシュの Action も増える

## 再検討する条件

- Dependabot の PR が多すぎて、読まれずに溜まるとき
- 版を上げる PR のたびに、clippy の直しが大きくなるとき
- 依存が増えて CI が遅くなり、キャッシュが要るとき
