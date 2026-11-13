# 0022. workflow_run は、上流の実行のイベントと head のリポジトリを `if:` で絞っていれば、書き込める人だけが起動できるとみなす

- ステータス: 承認(上流のイベントの段の分け方は 0024 で置き換え)
- 日付: 2026-11-13

## 文脈

- ADR 0012 は、workflow_run には起動した人の `author_association` が無いので、`github.actor` か `github.triggering_actor` の比較でしか絞れないとした
- ADR 0021 の調査で、無作為に選んだ 30 件の指摘のうち 3 件が誤検知だった。3 件とも workflow_run の `unrestricted-trigger`(`error`)で、うち 1 件は次の形だった
  - `github.event.workflow_run.event == 'pull_request' || github.event.workflow_run.event == 'workflow_dispatch'`
  - かつ `github.event.workflow_run.head_repository.full_name == github.repository`
- workflow_run の `github.event.workflow_run` は上流の実行を表す。`event` は上流を起動したイベントの名前、`head_repository` は上流の実行の head のリポジトリ
- 外部の人が上流を起動できるかは、上流のイベントで決まる
  - `workflow_dispatch` は、リポジトリに書き込める人しか起こせない。`schedule` は人が起こさない
  - `push` も書き込める人しか起こせないが、Dependabot も同じリポジトリに枝を push する。その commit のメッセージには、依存先の作者が書いたリリースノートが入る
  - `pull_request` は fork からも起こせるが、そのときの head のリポジトリは fork になる。同じリポジトリの枝からの `pull_request` は、書き込める人しか起こせない。GitHub の文書(Creating a pull request、2026-11-13 に確かめた)は、公開リポジトリで PR を開くには head の枝への書き込み権限が要ると書いている
  - `pull_request_target`、`issues`、`issue_comment` などは外部の人が起こせ、しかも head のリポジトリはこのリポジトリになる。head のリポジトリを比べるだけでは絞れない
- 残りの 2 件は、上流のワークフローが `push: tags` だけで動くものだった。上流は別のファイルにあり、1 つのファイルの検査では分からない

## 検討した選択肢

1. `if:` が上流のイベントを書き込める人だけが起こせる種類に絞り、`pull_request` を許すときは head のリポジトリがこのリポジトリであることも求めていれば、workflow_run を絞っているとみなす
2. head のリポジトリの比較だけで、絞っているとみなす
3. 上流のワークフローのファイルを探して、そのトリガーで決める

## 決定

高橋さんが判断を任せ(2026-11-13 に「どんどんやっていいよ。承認求めずに」)、Claude が 1 を選んだ。

- workflow_run の `if:` を、`||` で分かれる経路ごとに読む。`&&` は両側の経路を組み合わせ、`||` は経路を並べる。ジョブとステップの両方に `if:` があれば、`&&` として組み合わせる
- 各経路で、次の 2 つの事実を読む
  - 上流のイベントの候補:`github.event.workflow_run.event == '<名前>'` と、`contains(fromJSON('[...]'), github.event.workflow_run.event)` の一覧。`&&` で組み合わせると候補は狭まる。比べていなければ、どのイベントもありうる
  - 同じリポジトリ:`github.event.workflow_run.head_repository.full_name == github.repository`(左右はどちらでもよい)
- `github.event_name` の比較で workflow_run では通らない経路と、`false` は除く
- ADR 0012 の起動した人の比較で絞った経路は、絞った経路として残す。起動した人を絞っても、Dependabot のような許した起動者の文面には外部の人が書いたものが入りうるため
- 経路ごとに、上流を起こせる人を 3 つに分け、最も広いものをそのステップの上流とする
  - 書き込める人:上流のイベントの候補が `workflow_dispatch`、`schedule` だけ
  - 絞っている:候補がそれらと `push`、同じリポジトリであることが成り立つときの `pull_request` だけ。または、起動した人の比較で絞っている
  - 誰でも:それ以外
- 上流が「書き込める人」なら、そのステップの外部の人が起こせるイベントから workflow_run を外してから、全ての規則に渡す。`unrestricted-trigger` も、起動できる人の軸の 0.5 も、`pr-head-checkout` の workflow_run の条件も効かない
- 上流が「絞っている」なら、`unrestricted-trigger` は出さないが、workflow_run は外部の人の文面が届くイベントとして残す(起動できる人の軸は 0.5、`pr-head-checkout` の条件も残る)。同じリポジトリの枝からの PR と push には Dependabot のものも含まれ、その本文や commit のメッセージには依存先の作者が書いたリリースノートが入るため
- これまでの `github.actor` などの比較による絞り方は、そのまま保つ
- 上流のファイルを探すこと(選択肢 3)は、今回はしない

## 理由

- head のリポジトリの比較だけでは、`pull_request_target` や `issue_comment` で外部の人が起こした上流を通してしまう(選択肢 2)
- 経路ごとに読むので、`||` で外部の人の経路が混ざれば絞っていないと分かり、どのイベントに同じリポジトリの確かめが付いているかも失わない
- 上流のファイルを探すには、1 つのファイルを検査する今の作りを、ディレクトリ全体を見る作りに変える必要がある。上流が別のリポジトリにあることもある(選択肢 3)

## 受け入れたトレードオフ

- 上流が `push` だけで動くことを、`if:` に書かずにトリガーの側で決めているワークフローは、これまでどおり誤検知になる。調査では 30 件のうち 2 件がこの形だった
- `github.event.workflow_run.head_repository.fork == false` や、`head_repository.id` と `github.repository_id` の比較のような、別の書き方の同じリポジトリの確かめは読まない
- 上流のイベントを `!=` で除く書き方は読まない
- `release`、`merge_group` など、他の書き込める人しか起こせない上流のイベントは「誰でも」として扱う
- 上流のイベントを比べる `if:` は、workflow_run 以外のイベントでは常に偽になるが、その事実は workflow_run の扱いにしか使わない。`on: [workflow_run, issue_comment]` で上流のイベントだけを比べると、issue_comment では動かないのに指摘が出る
- `||` の経路を全て組み合わせるので、`&&` と `||` を深く入れ子にした `if:` では経路の数が増える。ワークフローの `if:` は短いので、時間は問題にならない

## 再検討する条件

- 上流のトリガーで決まる誤検知が多く、上流のファイルを探す価値が出たとき
- 調査で、ここで読まない書き方が多く見つかったとき
