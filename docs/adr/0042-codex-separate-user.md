# 0042. `safety-strategy: unprivileged-user` の Codex には、ステップの env が届かないとみなす

- ステータス: 承認
- 日付: 2026-11-16

## 文脈

- openai/codex-action は `safety-strategy: unprivileged-user` のとき、`sudo -u <codex-user> --` で Codex を動かす(`src/runCodexExec.ts`、2026-11-16 に `13e9c17` で確かめた)。`-E` も `--preserve-env` も付けないので、sudo の既定の `env_reset` で、ステップの `env` は Codex に届かない
- ADR 0040 で、この形は `other-secrets`、`long-lived-token`、`agent-code-with-secrets` のどれも読まず、`env` が届くとみなしていると残した。PR #62 のレビューで、1 つの規則にだけ特例を入れると規則どうしが食い違うと分かった

## 検討した選択肢

1. この形では、`env` をエージェントのプロセスが受け継がないとみなし、3 つの規則が共通に使う `secret_env` に印を持たせて扱う
2. 今のまま、`env` が届くとみなす

## 決定

高橋さんが判断を任せ(2026-11-16 に「全部進めてください」)、Claude が 1 を選んだ。

- `with` の入力(`prompt` など)で渡す秘密情報は、今までどおりエージェントに届くとみなす。入力の中の `${{ env.X }}` は、ランナーが action を呼ぶ前に展開するので、`env` の秘密情報もたどる(`env` の中身は残し、受け継ぐかだけを切り替える)
- `env` の秘密情報は、`agent-code-with-secrets` ではエージェントがまだ持たない秘密情報として、後のステップの指摘に出る

## 理由

- 3 つの規則が同じ `secret_env` を使うので、1 か所で揃う
- 届かない秘密情報を `other-secrets` として出すのは誤検知で、後のステップがそれでエージェントの書き換えたコードを動かすのは本当に新しい道になる

## 受け入れたトレードオフ

- sudoers で `env_keep` を広げたランナーでは `env` が届くが、ワークフローからは見えないので数えない
