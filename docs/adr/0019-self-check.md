# 0019. このリポジトリのワークフローを、リリースした sekimori の Action で検査し、結果をコードスキャンに送る

- ステータス: 承認
- 日付: 2026-11-13

## 文脈

- ADR 0017 で Action を作り、v0.1.0 を出した。Action は、自分と同じ ref の `Cargo.toml` の版のリリースを取ってくる
- PR #10 のレビューで、Action 自身(リリースを取ってきて証明を確かめる手順、`paths` の分け方、終了コードの合わせ方)を CI で動かしていないと指摘された
- このリポジトリには、エージェントを動かすワークフローが無い。検査の結果は、ふつう指摘なしになる
- `uses: ./` でこのリポジトリの Action を使うと、`Cargo.toml` の版を上げた PR では、まだ無いリリースを探して失敗する
- `github/codeql-action/upload-sarif` は、`security-events: write` の権限が要る。fork からの PR では、この権限は与えられない

## 検討した選択肢

1. リリースした版の Action を SHA で固定して使い、Dependabot で上げる。結果を `upload-sarif` で送る
2. `uses: ./` でこのリポジトリの Action を使う
3. Action を使わず、CI でビルドした CLI で検査する

## 決定

高橋さんが判断を任せ(2026-11-12 に「承認取らなくていい」)、Claude が 1 を選んだ。

- `.github/workflows/sekimori.yml` を足す。main への push と PR で動く
- `h11roki/sekimori` を v0.1.0 のコミットの SHA で固定して使い、`.github/workflows` を検査する。ADR 0008 の Dependabot が、新しいリリースのタグに合わせて SHA を上げる
- ワークフローの権限は空にし、ジョブに `contents: read` と `security-events: write` だけを与える
- `upload-sarif` は `if: always()` で動かす。ただし fork からの PR では送らない
- `upload-sarif` の `category` は `sekimori` にする。CodeQL などの他の解析と分けるため

## 理由

- 利用者と同じ使い方(リリースを取ってきて、証明を確かめて、SARIF を送る)を毎回通せる
- `uses: ./` は、版を上げてからタグを押すまでの間、CI が落ちる(選択肢 2)
- CLI で検査するだけでは、Action の手順を通らない(選択肢 3)
- 自分のワークフローに sekimori を当て続けると、将来エージェントを動かすワークフローを足したときに、自分で気付ける

## 受け入れたトレードオフ

- まだリリースしていない action.yml の変更は、この検査では通らない。タグを押した後に、Dependabot の PR で初めて通る
- fork からの PR では、結果がコードスキャンに載らない。検査の結果はログに出る
- 結果がふつう指摘なしなので、指摘がコードスキャンにどう出るかは、このワークフローでは確かめられない(ADR 0018 で、使い捨ての枝で確かめた)

## 再検討する条件

- action.yml の変更が増え、リリースの前に Action を試したくなったとき
- このリポジトリに、エージェントを動かすワークフローを足したとき
