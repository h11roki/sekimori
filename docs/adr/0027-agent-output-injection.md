# 0027. エージェントの出力を後のステップがコードとして展開する形を、道具の軸の規則 `agent-output-injection` として数える

- ステータス: 承認
- 日付: 2026-11-15

## 文脈

- 本人に次の作業を聞き、2026-11-15 に新しい規則の調査が選ばれた
- 調査のデータ(ADR 0021)で、エージェントの後のステップを数えた。エージェントのモデルが書いた文が、`${{ }}` で後の `run` か `actions/github-script` の `script` に展開されるファイルが 26 あった(`run` 20、github-script 8、重なりあり)。今の判定では、`error` 6、`warning` 10、指摘なし 10
- 各 Action の `action.yml`(2026-11-15 の main)で、モデルの書いた文が入る出力を確かめた
  - claude-code-action: `structured_output`(`--json-schema` を渡したときの JSON)
  - codex-action: `final-message`(`codex exec` の出力そのまま)
  - run-gemini-cli: `summary`(Gemini CLI の応答)と `error`(Gemini CLI のエラー出力)
  - `execution_file`、`branch_name`、`conclusion`、`session_id`、`github_token` は Action が作る値で、モデルの書いた文ではない
- `${{ }}` はシェルや JavaScript の文法を知らずに文字列として埋め込まれる。外部の人の文面がエージェントに届くと、指示の注入で出力を操り、後のステップのシェルでコマンドを動かせる。後のステップはジョブの `GITHUB_TOKEN` と、そのステップの秘密情報を持つ
- zizmor の template-injection の文書(2026-11-15 に確かめた)は、既定では外部の人が操れる文脈の展開を指摘し、pedantic と auditor のときだけ全ての展開を指摘するとしている。エージェントの出力を外部の人が操れるかは、起動できる人の軸を見ないと決まらない

## 検討した選択肢

1. 同じジョブの後のステップの `run` と、`actions/github-script` の `script` に、モデルの書いた文の出力が流れ込むなら、道具の軸を満たす規則として数える
2. 重さの軸と関わらない独立の規則として、見つけたら必ず出す
3. 1 に加えて、`env` を経由する `${{ env.NAME }}`、ジョブの `outputs` を経由する他のジョブ、任意の Action のコードを受け取る入力も読む
4. 何もしない(zizmor の pedantic に任せる)

## 決定

高橋さんが判断を任せ(2026-11-15 に、次の作業として新しい規則の調査を選んだ。進め方は「承認を求めないでどんどん続けていい」)、Claude が 1 を選んだ。

- 規則の ID は `agent-output-injection`。道具の軸を満たす(重さ 1)。`broad-tools` と `pr-head-checkout` と同じく、軸の理由は `; ` でつなぐ
- 対象の出力は、上の 4 つ(claude の `structured_output`、codex の `final-message`、gemini の `summary` と `error`)
- エージェントのステップに `id` があり、同じジョブでそれより後のステップ(`parallel:` の中も含む)が次のどちらかを持つとき
  - `run` の `${{ }}` の値に、`steps.<id>.outputs.<出力>` が流れ込む。値に流れ込むかは、ADR 0015 と同じく式のパーサーの `dataflow_contexts` で決め、`fromJSON(...)` の中もたどる。比べるだけの式(`steps.x.outputs.summary != ''`)は数えない
  - `actions/github-script` の `script` の入力に、同じく流れ込む
- 出力をまとめて `toJSON` で展開する形(`toJSON(steps.<id>.outputs)`、`toJSON(steps.<id>)`、`toJSON(steps)`)も、モデルの文を含むので数える。`toJSON` を通さずにまとめて展開すると `Object` という文字になるだけなので数えない
- 後のステップの `if:` が定数で偽(`if: false` など)なら、動かないので数えない
- 展開する後のステップに届く秘密情報(そのステップ、ジョブ、ワークフローの `env`、式で書いた `env`、`run` の本文や github-script の入力の `${{ secrets.* }}`。`GITHUB_TOKEN` は除く)は、権限と秘密情報の軸の理由に足す。注入されたコードはそのステップの秘密情報で動くため。展開する後のステップが複数あれば、全てから探す
- 指摘の位置は、後のステップの `run` か `script`。複数あれば最初のもの

## 理由

- 出力をシェルに埋め込むのは、エージェントにどのコマンドでも動かせる道具を渡すのと同じ結果になる。道具の軸に入れれば、起動できる人と権限の軸と合わせて、ADR 0007 の重さがそのまま効く(選択肢 2 は、書き込める人しか起動できない内部の道具も `error` 並みに出してしまう)
- 選択肢 3 の経路は、調査ではほとんど使われていなかった
  - `env` を経由して `$NAME` で読む形は安全。同じジョブでモデルの文の出力を `env` に入れるファイルは 290 あったが、`${{ env.NAME }}` で展開する形は 0 だった
  - ジョブの `outputs` で他のジョブに渡すファイルは 308 あったが、受け取る側は 261 が `env` で、github-script の `script` に展開するのが 1、`run` は 0 だった
  - 任意の Action の入力は、たどる先が増える割に、件数が分からない
- エージェントの出力を外部の人が操れるかは、sekimori だけが判定できる。zizmor の既定では出ない(選択肢 4)

## 受け入れたトレードオフ

- `steps.*.outputs.summary` のようにワイルドカードで全てのステップの出力を集める形は読まない
- 後のステップの `shell:` と `working-directory:` も展開されるが、調査で見つからなかったので読まない
- `${{ env.NAME }}` やジョブの `outputs` を経由する展開、他のジョブ、github-script 以外の Action の入力(コマンドを受け取る Action など)は読まない
- エージェントのステップに `id` が無いと、出力は参照できないので数えない
- run-gemini-cli の `error` は、モデルの文がどれだけ入るかを確かめていない。エラー出力に応答や道具の出力が混ざりうるので、数える側に倒す
- claude-code-action の `branch_name` は、PR のイベントでは PR の head の枝の名前になりうる。モデルの書いた文ではないので、この規則では扱わない
- 道具の軸は 1 点のままで、`broad-tools` と同時に当たっても重さは増えない

## 再検討する条件

- `env` やジョブの `outputs` を経由する形の件数が増えたとき
- 各 Action が、モデルの書いた文の入る出力を足したとき
