# 0040. Codex の資格情報は、エージェントがすでに持つ秘密情報として後のステップの指摘から除き、資格情報の名前は Bedrock の鍵に絞る

- ステータス: 承認
- 日付: 2026-11-15

## 文脈

- ADR 0039 で、Codex の `AWS_` で始まる `env` を `other-secrets` で数えないことにした。そのとき、`agent-code-with-secrets`(ADR 0028)が「エージェントがすでに持つ秘密情報」として除く一覧には入れなかった。理由は「Codex のシェルは既定で、名前に `KEY`、`SECRET`、`TOKEN` を含む `env` を消す」だった
- この理由は誤りだった。openai/codex の `codex-rs/config/src/shell_environment_policy.rs` では、`ignore_default_excludes` の既定は `true` で、名前で消す処理は既定では動かない。`inherit` の既定は `all` で、Codex のシェルは `env` をすべて受け継ぐ。openai/codex-action はこの設定を変えていない(2026-11-15 に、openai/codex の `7442f5f` と openai/codex-action の `13e9c17` で確かめた)
- そのため、Codex に渡した資格情報はエージェントの手にあり、後のステップが同じ鍵でエージェントの書き換えたコードを動かしても、新しく鍵に届く道にはならない

## 検討した選択肢

1. Codex の資格情報(`OPENAI_API_KEY` と `AWS_` で始まる名前)を、後のステップの指摘から除く一覧に入れる
2. 今のまま、後のステップの指摘で数える

## 決定

高橋さんが判断を任せ(2026-11-15 に「プロダクトとして使えるレベルまで進めていい」)、Claude が 1 を選んだ。

- Codex のステップの `env` は、資格情報に限らず全てエージェントの手にあるとみなす。資格情報もそこに入れる
- ただし `safety-strategy: unprivileged-user` では、codex-action は `sudo -u <ユーザー> --` で Codex を動かし(`-E` が無く、sudo が `env` を消す)、`env` は Codex に届かない。このときは何も手にないとみなす
- ADR 0039 の `AWS_` の接頭辞をやめ、Codex の資格情報は `OPENAI_API_KEY` と `AWS_BEARER_TOKEN_BEDROCK` だけにする。`env` が全て手にあるので、接頭辞のままだと Bedrock と関係のない AWS の鍵(配備用など)がどの規則にも出なくなる。`AWS_ACCESS_KEY_ID` と `AWS_SECRET_ACCESS_KEY` は Bedrock の鍵にも使えるが、配備用の鍵と見分けられないので数える
- claude と gemini の資格情報は今のまま除かない。claude-code-action は `allowed_non_write_users` のときに資格情報をシェルから消し、run-gemini-cli は `settings` が参照しない `env` をシェルに渡さないため

## 理由

- ADR 0028 の「すでにエージェントに届く秘密情報は数えない」に、Codex の実際の動きを合わせる
- 同じ鍵を `other-secrets` で数えず、後のステップの指摘でも数えないので、ADR 0039 で消した誤検知が別の規則に移らない

## 受け入れたトレードオフ

- `AWS_ACCESS_KEY_ID` と `AWS_SECRET_ACCESS_KEY` で Bedrock を使う Codex は、`other-secrets` に出る
- Codex の設定(`shell_environment_policy`)でシェルに渡す `env` を絞っていても、全て手にあるとみなす。codex-action は既定の `safety-strategy` では `codex-args` でこの設定を変えさせない(`runCodexExec.ts` の `validateProtectedExtraArgs`)が、設定ファイルで変えた形は読まない
