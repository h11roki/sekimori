# 0044. claude-code-action の auto モードは、それだけでは広い道具として数えない

- ステータス: 承認
- 日付: 2026-11-16

## 文脈

- claude-code-action は、tag モードと agent モードの既定で `--permission-mode auto` を付ける(`src/modes/permission-mode.ts` の `defaultsToAutoMode`、2026-11-16 に `9fa6885` で確かめた)。auto モードでは、許可の一覧に無い呼び出しを安全の審査が通すかを決める
- claude-code-action の `docs/security.md` は「auto モードは審査であって境界ではない」とし、厳しい一覧が要るなら `acceptEdits` か `default` を勧める。一方で、`allowed_non_write_users` や `allowed_bots` で外の人の文が書き込みの確認なしに届く実行では、auto モードを既定にしない
- 公開ワークフローの調査(v0.15.0 の後の main)では、claude-code-action のステップのうち、auto モードが既定のものが 696 件あり、うち 535 件は今は何も指摘されていない
- sekimori の `broad-tools` は、許可の一覧と権限のモードから「何でも動かせる」ことを読む

## 検討した選択肢

1. auto モードだけでは数えない。README の制限に書き、ビルドの道具などの判定で書き換えを許すことにだけ使う
2. auto モードを `broad-tools` として数える
3. 道具の軸に 0.5 を入れる段階を作り、auto モードを 0.5 とする

## 決定

高橋さんが判断を任せ(2026-11-16 に「全部進めてください」)、Claude が 1 を選んだ。

## 理由

- 2 では、auto モードが既定の 696 件のほぼ全てに道具の軸が付き、権限の軸と合わせて `warning` 以上になる。claude-code-action を使うだけで指摘になり、ほかの指摘が埋もれる
- auto モードは、外の人の文が書き込みの確認なしに届く実行では、メーカーが自分で切っている。sekimori の起動できる人の軸が 1 になる形(`allowed_non_write_users` など)では、もともと auto モードにならない
- 3 は重さの段階を変えることになり、道具の軸が何を意味するかが分かりにくくなる。審査がどこまで止めるかは、ワークフローからは読めない

## 受け入れたトレードオフ

- 書き込みの権限がある人が、外の人の issue や PR で Claude を呼ぶ形(起動できる人の軸が 0.5)では、許可の一覧に無いコマンドも審査が通せば動く。sekimori はこの形を道具の軸で数えない
- メーカーが auto モードの扱いを変えたら、この判断を見直す
