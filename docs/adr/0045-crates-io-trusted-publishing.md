# 0045. crates.io には、リリースのワークフローから Trusted Publishing で出す

- ステータス: 承認
- 日付: 2026-11-16

## 文脈

- v0.15.0 から crates.io に出している。最初の版は手元で `cargo publish` を動かし、トークンは手で渡した
- 版を出すたびに手で `cargo publish` するのは忘れやすく、タグと crates.io の中身がずれるおそれがある
- crates.io の Trusted Publishing は、GitHub Actions の OIDC のトークンを、短い間だけ使える crates.io のトークンに換える。`rust-lang/crates-io-auth-action` がこれを行い、ジョブの終わりに失効させる。crates.io の設定で、リポジトリとワークフローのファイルを登録しておく必要がある

## 検討した選択肢

1. リリースのワークフローに、Trusted Publishing で `cargo publish` するジョブを足す
2. crates.io のトークンを GitHub の秘密情報に置き、`cargo publish` するジョブを足す
3. 今のまま、手で出す

## 決定

高橋さんが判断を任せ(2026-11-16 に「全部進めてください」)、Claude が 1 を選んだ。

- `publish` のジョブは、GitHub のリリースを作るジョブの後に動き、タグの commit を取り出して `cargo publish --locked` する
- 権限は `contents: read` と `id-token: write` だけにする
- `rust-lang/crates-io-auth-action` は SHA で固定する(v1.0.6)

## 理由

- 期限の無いトークンを GitHub に置かずに済む。sekimori 自身が `long-lived-token` の規則で勧めない形を、自分のリリースでも避ける
- GitHub のリリース(バイナリと証明)が出来てから crates.io に出すので、片方だけが出ることが減る

## 受け入れたトレードオフ

- crates.io の設定で Trusted Publishing を登録するのは、高橋さんの手の操作になる。登録するまで `publish` のジョブは失敗する
- `environment` で守っていないので、タグを押せる人なら crates.io に出せる。タグを押せるのは書き込みの権限がある人だけ
