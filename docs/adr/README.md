# ADR(設計判断の記録)

設計上の判断を、判断した時点の文脈と一緒に残す。後から「なぜこうなっているか」を読めるようにし、前提が変わったときに見直せるようにする。

## 書く契機

- 選択肢が2つ以上あり、後から戻しにくい判断をしたとき
- 実装より先に書く

## ルール

- 置き場は `docs/adr/NNNN-短いタイトル.md`。番号は4桁の連番で欠番を作らない
- 一度承認した ADR は書き換えない。覆すときは新しい ADR を書き、古い方のステータスを「廃止(Superseded by NNNN)」にする
- 1ファイル1テーマ。区切りは見出し階層で表し、水平線は使わない。図は Mermaid
- 項目は下のテンプレートに従う

## テンプレート

```markdown
# NNNN. 判断を一文で

- ステータス: 提案 / 承認 / 廃止(Superseded by NNNN)
- 日付: YYYY-MM-DD

## 文脈

## 検討した選択肢

## 決定

## 理由

## 受け入れたトレードオフ

## 再検討する条件
```

## 一覧

| 番号 | タイトル | ステータス |
|---|---|---|
| [0001](0001-agent-workflow-audit.md) | AI コーディングエージェントを動かす GitHub Actions のワークフローを、起動できる人・許した道具・権限の重なりで静的に検査する | 承認 |
| [0002](0002-rust.md) | Rust で書く | 承認 |
| [0003](0003-official-agent-actions.md) | 最初の版は公式の 3 つの Action を対象にし、各社の公式文書の警告を規則の根拠にする | 承認 |
| [0004](0004-mit-license.md) | ライセンスは MIT にする | 承認 |
| [0005](0005-workflow-parsing.md) | ワークフローの YAML は zizmor の github-actions-models と yamlpath で読む | 承認 |
| [0006](0006-first-rules.md) | 最初の版の規則は、起動の制限を外した、広い道具、期限の無いトークン、PR の head の checkout、他の秘密情報の 5 つにする | 承認 |
| [0007](0007-severity.md) | 指摘の重さは満たした軸の数で決め、外部の人の文面が届くだけのときは起動できる人の軸を半分に数える | 承認 |
| [0008](0008-ci-pinning.md) | CI は Rust の版と Action を固定し、Dependabot で週に 1 回上げる | 承認 |
| [0009](0009-pipeline.md) | ライブラリと CLI に分け、読む、エージェントのステップを見つける、軸の事実に写す、規則と重さ、出すの段に分ける | 承認 |
| [0010](0010-cli.md) | CLI はパスを受け取り、読めないファイルがあっても残りを検査し、終了コードを 0、1、2 に分け、英語で出す | 承認 |
| [0011](0011-findings-output.md) | 指摘は規則の ID と位置と軸の内訳で出し、軸の値は半分を単位に数える | 承認 |
| [0012](0012-trigger-gate-details.md) | 起動の制限の判定を各社の実装に合わせ、`if:` は式のパーサーで読む | 承認(`needs:` の前のジョブを見ないことは 0026 で置き換え) |
| [0013](0013-broad-tools.md) | 広い道具は、各社のソースで確かめた設定で決め、読めない設定は当てない | 承認 |
| [0014](0014-pr-head-checkout.md) | PR の head の checkout は、エージェントが PR の設定を読み込むときだけ当てる | 承認 |
| [0015](0015-tokens-and-secrets.md) | 期限の無いトークンと他の秘密情報は、エージェントの手に届く値だけを見る | 承認(codex の資格情報の名前は 0039 で足した) |
| [0016](0016-sarif-output.md) | SARIF は `--format sarif` で 1 つの run として出し、重さは結果の level で表す | 承認(結果のまとめ方と message の形と重複の見込みは 0018、送るステップの条件は 0019 で置き換え) |
| [0017](0017-action-distribution.md) | Action は、Releases に置いたビルド済みのバイナリを、来歴の証明を確かめてから取ってくる composite Action にする | 承認(利用者のワークフローの例は 0019 で、crates.io に出さないことは 0045 で置き換え) |
| [0018](0018-sarif-merge-same-place.md) | SARIF では、同じ規則と位置の結果を 1 つにまとめ、重い方の level と全ての内訳を書く | 承認 |
| [0019](0019-self-check.md) | このリポジトリのワークフローを、リリースした sekimori の Action で検査し、結果をコードスキャンに送る | 承認(Action を試すジョブの注釈の確かめは 0046 で足した) |
| [0020](0020-readme.md) | 利用者向けの使い方は、リポジトリの根の README に英語で書き、設計の理由は ADR に任せる | 承認 |
| [0021](0021-public-workflow-survey.md) | 公開リポジトリのエージェントのワークフローに sekimori を当て、集計だけを公開する | 承認 |
| [0022](0022-workflow-run-origin.md) | workflow_run は、上流の実行のイベントと head のリポジトリを `if:` で絞っていれば、書き込める人だけが起動できるとみなす | 承認(上流のイベントの段の分け方は 0024 で置き換え) |
| [0023](0023-label-gate.md) | 付けたラベルの名前で絞った `if:` は、起動した人を絞ったものとして扱う | 承認 |
| [0024](0024-upstream-workflow-files.md) | workflow_run の上流は、同じ検査に渡されたワークフローから名前で探し、そのトリガーで起こせる人を決める | 承認 |
| [0025](0025-gemini-without-prompt.md) | `prompt` を渡さない run-gemini-cli は、作業場所が既定のブランチのままと言えるときだけ、起動できる人の軸を数えない | 承認 |
| [0026](0026-needs-gate.md) | エージェントのジョブが `needs:` で待つ前のジョブの `if:` も、エージェントのステップの条件として読む | 承認 |
| [0027](0027-agent-output-injection.md) | エージェントの出力を後のステップがコードとして展開する形を、道具の軸の規則 `agent-output-injection` として数える | 承認 |
| [0028](0028-agent-code-with-secrets.md) | エージェントが書き換えられるコードを、後のステップが秘密情報を持って動かす形を、権限と秘密情報の軸の規則 `agent-code-with-secrets` として数える | 承認 |
| [0029](0029-no-push-rule.md) | エージェントの後の push、マージ、公開は、規則にしない | 承認 |
| [0030](0030-exclusive-step-conditions.md) | エージェントのステップと両立しない `if:` を持つ後のステップは、同じ実行では動かないとみなす | 承認 |
| [0031](0031-ignore-comments.md) | 指摘は、エージェントのステップの `uses:` の行の `# sekimori: ignore[規則]` のコメントで抑える | 承認 |
| [0032](0032-claude-code-base-action.md) | Claude Code の base action も、4 つ目のエージェントとして検査する | 承認 |
| [0033](0033-ignore-warnings.md) | 読めない抑える指示と知らない規則の ID は、標準エラー出力に警告する | 承認(`--format github` での出し方は 0035 で足した) |
| [0034](0034-default-branch-upstream-checkout.md) | workflow_run で、上流が同じリポジトリの既定の枝で動いたと確かめていれば、その commit の取り出しは PR の head としない | 承認 |
| [0035](0035-github-annotations.md) | `--format github` で GitHub の注釈を出し、Action は注釈とジョブの要約に指摘を出す | 承認(Action の実行の回数と要約の書き方は 0037 で置き換え) |
| [0036](0036-action-exit-code.md) | Action は sekimori の終了コードを `exit-code` の出力に出し、Action を試すジョブはそれが 1 であることを確かめる | 承認(`exit-code` は 1 回の実行の終了コードになり、要約が書けないときの扱いと `sarif-file` を出す条件は 0037 で置き換え。試すジョブの注釈の確かめは 0046 で足した) |
| [0037](0037-output-files.md) | CLI は `--sarif-file` と `--summary-file` で SARIF とジョブの要約もファイルに書き、Action は sekimori を 1 回だけ動かす | 承認(書きかけの SARIF と、書けなかったことの出し方は 0038 で置き換え) |
| [0038](0038-output-file-failures.md) | 書きかけの SARIF のファイルは消し、書けなかったことは `--format github` で注釈にする | 承認 |
| [0039](0039-codex-bedrock-credentials.md) | Codex でも `AWS_` で始まる `env` を、エージェント自身の資格情報として数えない | 承認(`AWS_` の接頭辞、後のステップの指摘での扱いと、その理由は 0040 で置き換え) |
| [0040](0040-codex-credentials-in-hand.md) | Codex の env は全てエージェントの手にあるとして後のステップの指摘から除き、資格情報の名前は Bedrock の鍵に絞る | 承認(`unprivileged-user` の扱いは 0042 で足した) |
| [0041](0041-build-tool-commands.md) | ビルドとテストのコマンドは、書き換えと組むと広い道具として数え、パッケージを取って動かすコマンドは単独で数える | 承認(シェルのコマンドでの書き換えは 0043 で足した) |
| [0042](0042-codex-separate-user.md) | `safety-strategy: unprivileged-user` の Codex には、ステップの env が届かないとみなす | 承認 |
| [0043](0043-shell-file-writers.md) | ファイルを書き換えるシェルのコマンドを許すことも、書き換えとして数える | 承認 |
| [0044](0044-auto-mode.md) | claude-code-action の auto モードは、それだけでは広い道具として数えない | 承認 |
| [0045](0045-crates-io-trusted-publishing.md) | crates.io には、リリースのワークフローから Trusted Publishing で出す | 承認 |
| [0046](0046-action-test-annotations.md) | Action を試すジョブの後に、そのジョブの注釈を API で読み、SARIF と突き合わせる | 承認 |
