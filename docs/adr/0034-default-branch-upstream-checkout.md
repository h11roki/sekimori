# 0034. workflow_run で、上流が同じリポジトリの既定の枝で動いたと確かめていれば、その commit の取り出しは PR の head としない

- ステータス: 承認
- 日付: 2026-11-15

## 文脈

- Claude Code の base action のワークフローを集めた調査(PR #46)で、`pr-head-checkout` の 2 件が誤検知だった。どちらも次の形だった
  - `on: workflow_run`
  - ジョブの `if:` に、`github.event.workflow_run.head_repository.full_name == github.repository` と `github.event.workflow_run.head_branch == github.event.repository.default_branch` が「かつ」でつながる
  - `actions/checkout` の `ref: ${{ github.event.workflow_run.head_sha }}`
- 上流の実行が同じリポジトリの既定の枝で動いたなら、`head_sha` は既定の枝の commit になる
  - push の上流では、書き込める人が既定の枝に入れた commit
  - `issue_comment` や `schedule` の上流では、既定の枝の先頭
  - fork の PR の上流は、リポジトリの比較で外れる。同じリポジトリの PR の上流は、`head_branch` が PR の枝なので外れる
- ADR 0014 は、workflow_run の `head_sha` を PR の head として扱っている

## 検討した選択肢

1. エージェントのステップの条件(ジョブ、ステップ、ADR 0026 の前のジョブ)が、上の 2 つの等しさを「かつ」の項に持てば、workflow_run の `head_sha` と `head_repository` の取り出しを、base の枝の取り出しとみなす。2 つの等しさは workflow_run 以外のイベントでは偽になるので、他の特権のイベントが一緒にあっても、そのジョブは workflow_run でしか動かない
2. 2 つのうち、同じリポジトリの比較だけで、PR の head としない
3. 何もしない

## 決定

高橋さんが判断を任せ(2026-11-15 に「止まらないで。開発改善を進めて」)、Claude が 1 を選んだ。

- 2 つの等しさは、左右どちらの向きでもよい。同じリポジトリは `github.repository` か `github.event.repository.full_name` と比べる形を読む。`!=` や `||` の中は読まない
- `actions/checkout` で base の取り出しとみなすのは、`ref` か `repository` の値がそのまま `${{ github.event.workflow_run.head_sha }}` か `${{ github.event.workflow_run.head_repository.full_name }}` のときだけ。他の値と `||` などで混ぜた式は、これまでどおり読む
- 除くのは workflow_run の `head_sha` と `head_repository` だけ。`gh pr checkout`、`refs/pull/N/head`、`workflow_run.pull_requests[0].head.sha` のように PR を名指しする取り出しは、これまでどおり PR の head とする
- 起動できる人の軸(ADR 0022、0024)は変えない。上流が `issue_comment` で動くなら、外部の人の実行も既定の枝で動き、この条件を通るため

## 理由

- 取り出すのが既定の枝の commit なら、作業場所の設定は書き込める人のもので、PR の中身ではない
- 同じリポジトリの比較だけ(選択肢 2)では、同じリポジトリの PR の枝の commit が残る。枝は書き込める人しか作れないが、PR の中身はレビュー前のことがある
- 調査で見つかった誤検知で、指摘の理由に出ていた(選択肢 3)。調べた 2 件は道具の軸を `broad-tools` でも満たしていたので、重さは変わらない

## 受け入れたトレードオフ

- 既定の枝の名前を定数で比べる形(`head_branch == 'main'`)と、`||` の両側で 2 つの等しさを確かめる形は読まない
- GitHub の `==` は大小を区別しないので、既定の枝と大小だけ違う枝の名前で上流が動けば通る。GitHub がそうした枝を作らせるかは確かめていない
- 上流の `head_sha` が既定の枝の commit でも、上流の実行の間に既定の枝へ別の commit が入る場合は考えない
