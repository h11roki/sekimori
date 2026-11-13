# 0022. workflow_run は、上流の実行のイベントと head のリポジトリを `if:` で絞っていれば、書き込める人だけが起動できるとみなす

- ステータス: 承認
- 日付: 2026-11-13

## 文脈

- ADR 0012 は、workflow_run には起動した人の `author_association` が無いので、`github.actor` か `github.triggering_actor` の比較でしか絞れないとした
- ADR 0021 の調査で、無作為に選んだ 30 件の指摘のうち 3 件が誤検知だった。3 件とも workflow_run の `unrestricted-trigger`(`error`)で、うち 1 件は次の形だった
  - `github.event.workflow_run.event == 'pull_request' || github.event.workflow_run.event == 'workflow_dispatch'`
  - かつ `github.event.workflow_run.head_repository.full_name == github.repository`
- workflow_run の `github.event.workflow_run` は上流の実行を表す。`event` は上流を起動したイベントの名前、`head_repository` は上流の実行の head のリポジトリ
- 外部の人が上流を起動できるかは、上流のイベントで決まる
  - `push` と `workflow_dispatch` は、リポジトリに書き込める人しか起こせない。`schedule` は人が起こさない
  - `pull_request` は fork からも起こせるが、そのときの head のリポジトリは fork になる。同じリポジトリの枝からの `pull_request` は、枝を push できる人、つまり書き込める人しか起こせない
  - `pull_request_target`、`issues`、`issue_comment` などは外部の人が起こせ、しかも head のリポジトリはこのリポジトリになる。head のリポジトリを比べるだけでは絞れない
- 残りの 2 件は、上流のワークフローが `push: tags` だけで動くものだった。上流は別のファイルにあり、1 つのファイルの検査では分からない

## 検討した選択肢

1. `if:` が上流のイベントを書き込める人だけが起こせる種類に絞り、`pull_request` を許すときは head のリポジトリがこのリポジトリであることも求めていれば、workflow_run を絞っているとみなす
2. head のリポジトリの比較だけで、絞っているとみなす
3. 上流のワークフローのファイルを探して、そのトリガーで決める

## 決定

高橋さんが判断を任せ(2026-11-13 に「どんどんやっていいよ。承認求めずに」)、Claude が 1 を選んだ。

- workflow_run の `if:` から、どの経路でも成り立つ 2 つの事実を読む
  - 上流のイベントの候補:`github.event.workflow_run.event == '<名前>'` の比較。`&&` は候補を狭め、`||` は候補を合わせる。比べていなければ、どのイベントもありうる
  - 同じリポジトリ:`github.event.workflow_run.head_repository.full_name == github.repository`(左右はどちらでもよい)。`&&` はどちらかで成り立てばよく、`||` は両方で成り立つ必要がある
- 次のどちらかなら、workflow_run を絞っているとみなす
  - 上流のイベントの候補が、`push`、`workflow_dispatch`、`schedule` だけ
  - 上流のイベントの候補が、それらと `pull_request` だけで、同じリポジトリであることも成り立つ
- ジョブとステップの両方に `if:` があれば、両方が成り立つ(`&&` と同じ)として読む
- これまでの `github.actor` などの比較による絞り方は、そのまま保つ
- 上流のファイルを探すこと(選択肢 3)は、今回はしない

## 理由

- head のリポジトリの比較だけでは、`pull_request_target` や `issue_comment` で外部の人が起こした上流を通してしまう(選択肢 2)
- イベントの候補と同じリポジトリの 2 つを、どの経路でも成り立つかで読むので、`||` で外部の人の経路が混ざれば絞っていないと分かる
- 上流のファイルを探すには、1 つのファイルを検査する今の作りを、ディレクトリ全体を見る作りに変える必要がある。上流が別のリポジトリにあることもある(選択肢 3)

## 受け入れたトレードオフ

- 上流が `push` だけで動くことを、`if:` に書かずにトリガーの側で決めているワークフローは、これまでどおり誤検知になる。調査では 30 件のうち 2 件がこの形だった
- `github.event.workflow_run.head_repository.fork == false` や、`head_repository.id` と `github.repository_id` の比較のような、別の書き方の同じリポジトリの確かめは読まない
- 上流のイベントを `!=` で除く書き方は読まない

## 再検討する条件

- 上流のトリガーで決まる誤検知が多く、上流のファイルを探す価値が出たとき
- 調査で、ここで読まない書き方が多く見つかったとき
