# 0016. SARIF は `--format sarif` で 1 つの run として出し、重さは結果の level で表す

- ステータス: 承認(重複の見込みは 0018 で置き換え)
- 日付: 2026-11-13

## 文脈

- ADR 0001 は、出力を SARIF にして GitHub のコードスキャンに載せるとした。ADR 0011 は平文の形を決め、規則の ID を SARIF の `ruleId` に使えるようにした
- GitHub のコードスキャンが受け付ける SARIF(2026-11-13 に GitHub の文書で確かめた)
  - 版は 2.1.0 だけ。`runs[].tool.driver` と `results[]` が要り、1 つの run には 1 つの道具の結果だけを入れる
  - 規則(`reportingDescriptor`)には `id`、`shortDescription.text`、`fullDescription.text`、`help.text` が要り、空の文字列は受け付けない
  - `artifactLocation.uri` の相対パスは、リポジトリの根からの相対とみなす。絶対パスは、取り出した場所が分かるときだけ相対に直す
  - 重さは、規則の `defaultConfiguration.level`(既定は `warning`)を、結果の `level` が上書きする。`security` の札を付けた規則は、`properties.security-severity`(0.1 から 10.0)で critical から low に分ける
  - `partialFingerprints` が無ければ、`upload-sarif` の action がソースから作る。API で直接送るときは、重複した警告になりうる
  - 1 つの run の結果は 25,000 件まで
- 指摘の重さは、規則ではなくエージェントのステップの軸の合計で決まる(ADR 0007)。同じ規則が、あるステップでは `error`、別のステップでは `warning` になる

## 検討した選択肢

1. `--format plain|sarif` を足し、SARIF のときは全てのファイルの結果を 1 つの run にまとめて標準出力に出す。重さは結果の `level` で表し、規則に `security-severity` を付けない
2. 1 と同じで、規則に `security-severity` を固定の値で付ける
3. SARIF を常にファイルに書き、平文と同時に出す

## 決定

高橋さんが判断を任せ(2026-11-12 に「承認取らなくていい」)、Claude が 1 を選んだ。

- 引数 `--format` は `plain`(既定)と `sarif` を受け付ける。`sarif` のときは、全てのファイルを検査してから 1 つの SARIF の文書を標準出力に書く
- 文書は `version` が `2.1.0`、`$schema` が SARIF 2.1.0 の JSON Schema。run は 1 つ
- `tool.driver` は、`name` が `sekimori`、`version` と `semanticVersion` が Cargo の版、`informationUri` がリポジトリの URL。`rules` には 5 つの規則を全て、ADR 0011 の ID の順に並べる。各規則は `shortDescription` に平文の 1 行目の文、`fullDescription` と `help` に規則が見る設定と軸を書き、`properties.tags` を `security` にする
- 結果は、指摘ごとに 1 つ。`ruleId`、`level`(`warning` か `error`)、`message.text`、`locations` に 1 つの位置を書く。`ruleIndex` は書かない(任意で、規則の一覧とずれたときに壊れた SARIF になるため)
- `message.text` は、平文の 1 行目の文と、軸ごとに 1 行の `who can trigger: 1 (理由)` の形。GitHub の画面は等幅ではないので、平文の桁揃えは使わない
- 位置の `artifactLocation.uri` は、ファイルの置き場所(ディレクトリのシンボリックリンクは解き、ファイル自身のリンクは解かない)から、リポジトリの根からの相対パスを `/` で区切って書く。根は `.git` を持つ最も近い祖先のディレクトリとする。ただし GitHub Actions の上で、その祖先が `GITHUB_WORKSPACE` の外にある(または無い)ときは `GITHUB_WORKSPACE` とする。`actions/checkout` の `path:` でサブディレクトリに取り出したリポジトリでも、根がずれないようにするため。根が見つからなければ、渡された相対パスを `.` と `..` を字面で畳んで書く。畳んでも `..` で始まるときと絶対パスは、`file:///` の URI にする(Windows のドライブの `:` は残し、UNC のパスは `file://server/share/...`)。空白や `#` などはパーセントエンコードする
- 列は SARIF の既定に合わせて UTF-16 の単位で数え、run の `columnKind` に `utf16CodeUnits` と書く。平文の列も同じ数え方にする(ASCII だけの行では今までと変わらない)
- 読めないファイルは、平文と同じく標準エラーに出し、加えて `invocations[0].toolExecutionNotifications` に `error` で書き、`executionSuccessful` を偽にする。終了コードは ADR 0010 のとおりで、指摘があれば 1。GitHub Actions で `upload-sarif` に渡すときは、送るステップに `if: always()` を付ける
- `partialFingerprints` と `runAutomationDetails` は書かない。前者は `upload-sarif` が作り、後者は `upload-sarif` の `category` で利用者が決める

## 理由

- 同じ規則でもステップごとに重さが変わるので、規則に固定の `security-severity` を付けると、`warning` と `error` の差が GitHub の画面で消える(選択肢 2)。結果の `level` なら差が残る
- 標準出力に 1 つの文書を書けば、`sekimori --format sarif > results.sarif` で `upload-sarif` に渡せる。ファイルの置き場を決める引数を増やさずに済む(選択肢 3)
- 平文の内訳を `message.text` に入れると、GitHub の警告の画面で 3 つの軸の理由を読める

## 受け入れたトレードオフ

- `security-severity` が無いので、GitHub の画面の重さは critical から low ではなく、`error` と `warning` で出る
- `partialFingerprints` が無いので、API で直接送ると、行が動いたときに同じ指摘が別の警告になりうる
- 指摘があると SARIF を書いたステップが失敗するので、送るステップに `if: always()` を付ける必要がある。終了コードを形式で変えると、CI で指摘を見落とすため
- リポジトリの根は `GITHUB_WORKSPACE` か `.git` で探すので、どちらも無い取り出し方(手元でアーカイブを展開したものなど)では、渡されたパスがそのまま書かれる。ホームディレクトリ全体が git のリポジトリのように、外側の `.git` を拾うと、根がずれる
- SARIF のときは全ての検査が終わるまで何も出さない。ワークフローのファイルの数は多くないので、待つ時間は問題にならない
- 同じ checkout や秘密情報を複数のエージェントのステップが指すと、同じ位置と規則の結果が複数できる。GitHub は指紋が同じなら 1 つの警告にまとめる
- `security` の札だけを付けて `security-severity` を付けないときの画面の出方は、文書に書かれていない。実際に送って確かめるまで、この形とする

## 再検討する条件

- GitHub に送った結果で、重さや重複の出方が期待と違うとき
- 利用者から、GitHub 以外の SARIF の読み手(エディタなど)向けの指紋や分類を求められたとき
