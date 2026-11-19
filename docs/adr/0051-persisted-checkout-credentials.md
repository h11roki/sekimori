# 0051. 前のステップの actions/checkout に残した保存済みのトークンと鍵も、エージェントに届く期限の無いトークンとして数える

- ステータス: 承認
- 日付: 2026-11-19

## 文脈

- ADR 0015 は、期限の無いトークン(`long-lived-token`)を、エージェントの入力か `env` に GitHub のトークンとして渡した保存済みの秘密情報で見ると決めた
- 2026-11-19 の正誤の確かめ(ADR 0049)で、判定者が規則の外の危険として次の形を挙げた。同じジョブの前のステップの `actions/checkout` に、`token: ${{ secrets.PAT }}` で個人のアクセストークンを渡し、`persist-credentials` を `false` にしないまま、後でエージェントを動かす形。標本の 30 ファイルのうち 2 ファイルにあった
- `actions/checkout` は、`persist-credentials` が既定の `true` のとき、`token` か `ssh-key` を作業場所の git の設定に残し、ジョブの終わりに消す。v5 までは `.git/config` に直接書き、v6 からは `$RUNNER_TEMP` の別のファイルに書いて `.git/config` から読み込む(2026-11-19 に actions/checkout の README で確かめた)。どちらでも、エージェントが動かす `git` はそのトークンで認証し、ファイルを読む道具があれば中身も読める
- 期限の無いトークンは、エージェントを操った人がジョブの外で使い続けられる。`GITHUB_TOKEN` と違い、ジョブが終わっても失効しない

## 検討した選択肢

1. エージェントのステップより前の、同じジョブの `actions/checkout` に、`token` か `ssh-key` として保存済みの秘密情報(`secrets.GITHUB_TOKEN` 以外)を渡し、`persist-credentials` が `false` でなければ、`long-lived-token` に数える
2. 新しい規則を足す
3. 数えない

## 決定

高橋さんが判断を任せ(2026-11-19 に「改善すすめて」)、Claude が 1 を選んだ。

- `actions/checkout` の `token` か `ssh-key` の値に流れ込む `secrets.*` を、ADR 0015 と同じ読み方で探す。`secrets.GITHUB_TOKEN` と、`steps.*.outputs.*` の App のトークンは数えない
- `persist-credentials` が `false`(YAML の真偽値か文字列)のときだけ残さないとみなす。式で決まるときは残すとみなす
- `path:` で別の場所に取り出しても、エージェントは作業場所の下を読めるので数える
- 理由には、checkout のステップが残したことと秘密情報の名前を書き、指摘は checkout の `with` の鍵を指す
- エージェントの入力か `env` に GitHub のトークンを渡していれば、これまでどおりそちらを先に書く
- codex-action を `safety-strategy: unprivileged-user` で別のユーザーとして動かすときも数える。actions/checkout(`src/git-auth-helper.ts`、2026-11-19 の main)は、`$RUNNER_TEMP` の資格情報のファイルをモードを指定せずに書くので、既定の umask では別のユーザーからも読める。`0600` で書くのは SSH の鍵のファイルだけで、`ssh-key` も git の設定からは鍵のパスが分かる

## 理由

- 規則が見たいのは、エージェントの手に届く期限の無い GitHub の資格情報で、どこから届くかは問わない。新しい規則にすると、同じ危険が 2 つの規則に分かれる
- `persist-credentials: false` は、actions/checkout の README と多くのセキュリティの手引きが勧める直し方で、利用者が直しやすい

## 受け入れたトレードオフ

- checkout とエージェントの間で、トークンを消すステップ(`git config --unset` など)があっても読まない
- checkout のステップの `if:` が、エージェントのステップと同じ実行で真にならない形も読まない
- codex-action の別のユーザーが SSH の鍵(`0600`)を読めないときも、`ssh-key` を数える。鍵のファイルは読めなくても、ランナーのファイルの権限の設定しだいで変わるため、区別しない
- `ssh-key` に渡した鍵が、デプロイ鍵のように 1 つのリポジトリにしか効かないものでも、期限の無い資格情報として数える
