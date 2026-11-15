# 0045. crates.io には、リリースのワークフローから Trusted Publishing で出す

- ステータス: 承認
- 日付: 2026-11-16

## 文脈

- ADR 0017 では crates.io に出さないとしたが、v0.15.0 から出している。最初の版は手元で `cargo publish` を動かし、トークンは手で渡した
- 版を出すたびに手で `cargo publish` するのは忘れやすく、タグと crates.io の中身がずれるおそれがある
- crates.io の Trusted Publishing は、GitHub Actions の OIDC のトークンを、短い間だけ使える crates.io のトークンに換える。`rust-lang/crates-io-auth-action` がこれを行い、ジョブの終わりに失効させる。crates.io の設定で、リポジトリとワークフローのファイルを登録しておく必要がある

## 検討した選択肢

1. リリースのワークフローに、Trusted Publishing で `cargo publish` するジョブを足す
2. crates.io のトークンを GitHub の秘密情報に置き、`cargo publish` するジョブを足す
3. 今のまま、手で出す

## 決定

高橋さんが判断を任せ(2026-11-16 に「全部進めてください」)、Claude が 1 を選んだ。

- `release` のジョブは GitHub のリリースを下書きで作り、`publish` のジョブがタグの commit を取り出して `cargo publish --locked --no-verify` し、最後の `announce` のジョブが下書きを公開する。crates.io に出せなかったときは、GitHub のリリースも下書きのまま残る
- `--no-verify` で、`cargo publish` のときに依存を組み立て直さない。依存の build.rs が、OIDC のトークンや crates.io のトークンのあるジョブで動かないようにするため(ADR 0017 で組み立てと署名のジョブを分けたのと同じ理由)。同じ commit は `build` のジョブで 4 つの環境で組み立てて試している
- `publish` のジョブは `environment: crates-io` で動かし、crates.io の Trusted Publishing にもこの環境を登録する。環境の保護の規則で、`v*` のタグからしか動かないようにする。環境が無いと、同じリポジトリの枝からの PR がワークフローを書き換えて OIDC のトークンを得られる
- 権限は `contents: read` と `id-token: write` だけにする
- CI でも `cargo package --locked` を動かし、パッケージに入れるファイルの抜けをタグの前に見つける
- `rust-lang/crates-io-auth-action` は SHA で固定する(v1.0.6)

## 理由

- 期限の無いトークンを GitHub に置かずに済む。sekimori 自身が `long-lived-token` の規則で勧めない形を、自分のリリースでも避ける
- GitHub のリリース(バイナリと証明)が出来てから crates.io に出すので、片方だけが出ることが減る

## 受け入れたトレードオフ

- crates.io の設定で Trusted Publishing を登録するのは、高橋さんの手の操作になる。登録するまで `publish` のジョブは失敗する
- 環境の作成と保護の規則(`v*` のタグだけ)は、GitHub の設定の手の操作になる
