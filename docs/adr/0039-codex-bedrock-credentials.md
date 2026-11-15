# 0039. Codex でも `AWS_` で始まる `env` を、エージェント自身の資格情報として数えない

- ステータス: 承認
- 日付: 2026-11-15

## 文脈

- ADR 0015 で、`other-secrets` はエージェント自身の資格情報として読む `env` を数えないと決めた。claude は `AWS_*` を含む名前、codex は `OPENAI_API_KEY` だけにした
- openai/codex(`codex-rs/model-provider/src/amazon_bedrock/auth.rs`)は、Amazon Bedrock を使うとき `AWS_BEARER_TOKEN_BEDROCK`、または `AWS_ACCESS_KEY_ID` と `AWS_SECRET_ACCESS_KEY`、地域は `AWS_REGION` か `AWS_DEFAULT_REGION` を `env` から読む
- 公開ワークフローの調査(v0.14.0)で、Codex に `AWS_BEARER_TOKEN_BEDROCK` を渡す形が `other-secrets` として 2 件出ていた。指摘の判定(20 件)でも、Bedrock で動かすための鍵を他の秘密情報と数えた誤検知とされた
- Azure の鍵は codex-action では `openai-api-key` の入力で渡され、中継の proxy が持つ。調査では、Codex の `env` の Azure の鍵が指摘になった例は無かった。Azure の鍵の名前は、利用者が設定(`env_key`)で決める

## 検討した選択肢

1. Codex でも、claude と同じく `AWS_` で始まる名前を資格情報として数えない
2. `AWS_BEARER_TOKEN_BEDROCK` など、codex のソースが読む名前だけを数えない
3. 今のまま

## 決定

高橋さんが判断を任せ(2026-11-15 に「推奨通りに進めて」)、Claude が 1 を選んだ。

## 理由

- AWS の資格情報は、SDK が読む `AWS_SESSION_TOKEN` や `AWS_PROFILE` などにも広がる。claude と同じ線にすれば、2 つのエージェントで同じ `env` の扱いが揃う
- Azure のように名前を利用者が決めるものは、名前だけでは資格情報か分からないので、今のまま数える

## 受け入れたトレードオフ

- Codex の `env` にある、Bedrock と関係のない AWS の秘密情報(例えば配備用の鍵)も数えなくなる。claude で同じことを受け入れている
