# 0009. ライブラリと CLI に分け、読む、エージェントのステップを見つける、軸の事実に写す、規則と重さ、出すの段に分ける

- ステータス: 承認
- 日付: 2026-11-12

## 文脈

- ADR 0001 は CLI と GitHub Action の 2 つで配ると決め、ADR 0003 は各社の設定の違いを翻訳の層に閉じ込めると決めた。ADR 0006 と 0007 は規則と重さを決めた
- ADR 0005 のクレートの作り(2026-11-12 に 1.32.0 のソースで確認)
  - github-actions-models: ジョブは、ステップを並べる通常のジョブと、再利用できるワークフローの呼び出しの 2 種類。ステップは `uses` と `run` のほか、`parallel:` の中に `uses` と `run` を並べられ、`wait`、`wait-all`、`cancel` もある。`uses` は、リポジトリ(owner、repo、subpath、ref)、ローカル、Docker の 3 種類
  - yamlpath: キーと添字の列(ルート)で、YAML の中の位置(行と列)を引く
- GitHub のリポジトリの名前は大文字と小文字を区別しない。API で `ANTHROPICS/Claude-Code-Action` が `anthropics/claude-code-action` に解決されることを確かめた
- claude-code-action のリポジトリには、別の入力を持つ `base-action` という Action が下の階層にある

## 検討した選択肢

1. ライブラリ(`src/lib.rs`)と CLI(`src/main.rs`)に分け、処理を段に分ける。ワークフローを読む、エージェントのステップを見つける、各社の設定を軸の事実に写す、規則と重さ、出す
2. 実行ファイルだけにし、中をモジュールに分ける
3. zizmor の audit のように、規則ごとにワークフロー全体を走査する

## 決定

高橋さんが判断を任せ(2026-11-12 に「承認取らなくていい」)、Claude が 1 を選んだ。

- `src/lib.rs` に段ごとのモジュールを置き、`src/main.rs` は引数、出力、終了コードだけを扱う
- 段は次のとおり。この ADR と同じ PR では、最初の 2 つを作る
  - `workflow`: ファイルを読み、github-actions-models の型と yamlpath の文書を 1 つにまとめる
  - `agent`: エージェントのステップを見つける
  - 各社の設定を軸の事実に写す段、規則、重さ、出力は、規則を実装する PR で足す
- エージェントのステップの見つけ方
  - `uses` がリポジトリの形で、owner/repo が `anthropics/claude-code-action`、`openai/codex-action`、`google-github-actions/run-gemini-cli` のどれかと一致する。大文字と小文字は区別せず、ref は問わない
  - subpath があるもの(`anthropics/claude-code-action/base-action` など)は、入力が違う別の Action なので含めない。末尾に `/` があるだけの空の subpath は、元の Action として扱う
  - 通常のジョブのステップと、`parallel:` の中のステップを見る。再利用できるワークフローの呼び出しは追わない(ADR 0006)
  - 見つけたステップは、ジョブの ID と、yamlpath のルート(`jobs.<id>.steps[<i>]`、`parallel:` の中なら `jobs.<id>.steps[<i>].parallel[<j>]`)を持つ。指摘の位置はこのルートから引く
- エラーは段ごとに意味のある型にし(thiserror)、CLI で表示する

## 理由

- ライブラリに分けると、段ごとにテストでき、Action も同じ実行ファイルを呼ぶだけで済む
- 段に分けると、各社の入力の名前を知るのは写す段だけになり、規則は 3 社に共通の事実だけを見る。ADR 0003 の翻訳の層がそのまま段になる
- 3 では、規則ごとにエージェントのステップを探し直すことになる。重さ(ADR 0007)はステップごとに全ての規則の結果を集めて決めるので、ステップを先に見つけて規則を当てる方が合う

## 受け入れたトレードオフ

- 段の間で受け渡す型が増える
- `parallel:` の中は見るが、並行に動くステップの前後の関係は、PR の head の checkout の規則(前のステップを数える)を作るときに改めて扱う
- 名前で見分けるので、`base-action`、フォーク、自作の包みは見逃す(ADR 0006)

## 再検討する条件

- zizmor に規則を寄稿するとき(zizmor の audit の形に合わせ直す)
- `base-action` を使ったワークフローが、公開リポジトリで目立つとき
- 段の間の型が、規則を足すたびに大きく変わるとき
