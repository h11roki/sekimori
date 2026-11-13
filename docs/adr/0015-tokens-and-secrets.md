# 0015. 期限の無いトークンと他の秘密情報は、エージェントの手に届く値だけを見る

- ステータス: 承認
- 日付: 2026-11-13

## 文脈

- ADR 0006 は、権限と秘密情報の軸を満たす規則として、期限の無いトークン(`long-lived-token`)と他の秘密情報(`other-secrets`)を挙げた。前者は、エージェントに渡す GitHub のトークンが `secrets.GITHUB_TOKEN` でも GitHub App で作ったトークンでもないとき。後者は、エージェントのステップかジョブの `env`、またはステップの `with` に、エージェント自身の API キーと GitHub のトークン以外の `secrets.*` があるとき
- ADR 0013 と 0014 と同じく、各社の振る舞いはソースで確かめた(2026-11-13)
  - claude-code-action(4d75e5f): GitHub のトークンは `github_token` の入力で受け取る。無ければ OIDC を Claude GitHub App のインストールトークンに換え、ジョブの終わりに失効させる。得たトークンを `GITHUB_TOKEN` と `GH_TOKEN` に入れてから、`process.env` をまるごと Claude Code に渡す(`src/entrypoints/run.ts`、`base-action/src/parse-sdk-options.ts`)。呼び出し側の `env` の `GITHUB_TOKEN` と `GH_TOKEN` はこのとき上書きされ、他の `env` は Bash から読める。資格情報として読む `env` は `ANTHROPIC_API_KEY`、`CLAUDE_CODE_OAUTH_TOKEN`、`AWS_*`、`GOOGLE_APPLICATION_CREDENTIALS`、`ANTHROPIC_FOUNDRY_*`、`ANTHROPIC_CUSTOM_HEADERS`
  - codex-action(13e9c17)と Codex(16280e1): 資格情報の入力は `openai-api-key` だけで、キーは中継のプロキシにだけ渡り、Codex からは見えない。GitHub のトークンの入力は無い。Codex は `process.env` をまるごと受け継ぎ、`shell_environment_policy` の既定も名前に `KEY` や `TOKEN` を含む変数を消さないので、`env` の値はシェルから読める
  - run-gemini-cli(387c8dd)と Gemini CLI(b977656): 資格情報の入力は `gemini_api_key` と `google_api_key`。GitHub のトークンの入力は無く、公式の例は `env` の `GITHUB_TOKEN` に置き、`settings` の `mcpServers.github.env` で `${GITHUB_TOKEN}` と参照して GitHub の MCP サーバーに渡す。Gemini CLI のシェルは、GitHub Actions の上では許した名前以外の環境変数を消す(`environmentSanitization.ts`)
  - 公式の例で GitHub App のトークンは、`actions/create-github-app-token` の出力 `${{ steps.<id>.outputs.token }}` で渡す。`steps.mint_identity_token.outputs.token || secrets.GITHUB_TOKEN || github.token` のように短命なものを `||` でつなぐこともある

## 検討した選択肢

1. エージェントの手に届く値だけを見る。各社が上書きや消去で届かなくする値は見ない
2. ADR 0006 の書き方どおり、ステップかジョブの `env` とステップの `with` の `secrets.*` を全て見る
3. 2 に加え、同じジョブの他のステップの `secrets.*` も見る

## 決定

高橋さんが判断を任せ(2026-11-12 に「承認取らなくていい」)、Claude が 1 を選んだ。

- 値は、`${{ }}` の式を ADR 0012 と同じ式のパーサーで読み、式の値に流れ込むものだけを見る(`a && b` は `b`、`a || b` は両方、`toJSON`、`fromJSON`、`format`、`join` の引数)。`secrets.APP_ID != ''` や `contains(secrets.LIST, 'x')` のように比べるだけのものは、値が出ないので数えない。その上で `secrets.NAME`、`secrets['NAME']`、または `secrets` そのもの(`toJSON(secrets)`、`secrets.*`)を、大文字と小文字を区別せずに探す。文字列の中の `secrets` や、`steps.import-secrets` のような名前の一部は数えない。`secrets[matrix.name]` のように名前が実行時に決まるものは、`GITHUB_TOKEN` かもしれないので数えない。`secrets.GITHUB_TOKEN` は GitHub が作る短命のトークンなので、どちらの規則でも数えない
- `env` は、ステップ、ジョブ、ワークフローの順で最初に見つかる値で決める。途中の `env` がまるごと式(`env: ${{ fromJSON(...) }}`)なら、それより近い `env` の値は残し、それより外側は見ない。どの名前を上書きするか分からないため。ADR 0014 の `run` の変数と `GEMINI_CLI_TRUST_WORKSPACE` も同じ決め方で読む。ステップの `env` が `${{ env.NAME }}` で外側の `env` の秘密情報を参照するものも、その秘密情報とみなす
- 期限の無いトークン: GitHub のトークンとしてエージェントに渡る値に、`secrets.GITHUB_TOKEN` 以外の `secrets.*` がある
  - claude-code-action: `with` の `github_token`
  - codex-action と run-gemini-cli: `env` の `GITHUB_TOKEN` と `GH_TOKEN`
  - `github.token`、`steps.*.outputs.*`(App のトークン)、それらを `||` でつないだものは数えない
- 他の秘密情報: 次の値に、`secrets.GITHUB_TOKEN` 以外の `secrets.*` がある
  - `with` のうち、値がエージェントに届く入力(claude は `prompt`、`settings`、`claude_args`、codex は `prompt`、`codex-args`、gemini は `prompt`、`settings`)。`gcp_workload_identity_provider` のように action が自分で使う入力は、エージェントに届かないので見ない。`${{ env.NAME }}` で秘密情報を持つ `env` の変数を参照するもの、`${{ toJSON(env) }}` のように `env` をまるごと渡すものも数える(GitHub のプロパティの参照と同じく、名前の大文字と小文字は区別しない)。GitHub が action に渡す前に値へ置き換えるため
  - まるごと式の `env` が秘密情報を参照するもの(claude-code-action と codex-action)
  - claude-code-action と codex-action: `env` のうち、エージェント自身の資格情報として読む名前と `GITHUB_TOKEN`、`GH_TOKEN` を除いたもの。claude は上の資格情報の名前、codex は `OPENAI_API_KEY`
  - run-gemini-cli: `env` のうち、`settings` の文字列が `$NAME` か `${NAME}` で参照する名前のもの。それ以外はシェルから消える。`GEMINI_API_KEY`、`GOOGLE_API_KEY`、`GITHUB_TOKEN`、`GH_TOKEN` は除く
- どちらも当たれば権限と秘密情報の軸を 1 とし、書き込みの権限と合わせて、理由を全て書く。指摘はその値の `with` か `env` の鍵を指す

## 理由

- ADR 0006 がこの 2 つの規則で見たいのは、エージェントが盗み出せる、または誤って使える資格情報。各社が上書きや消去で届かなくした値を指摘すると、利用者が確かめても届かないので、誤検知になる
- 2 は、claude-code-action の `env` の `GH_TOKEN` と run-gemini-cli のシェルに渡らない `env` を誤検知する
- 3 は、エージェントが他のステップの値を読めるかが、ランナーのファイルや出力の扱いしだいで静的に決めにくい

## 受け入れたトレードオフ

- `secrets.*` の名前からは、中身が GitHub のトークンかどうか分からない。`secrets.BOT_TOKEN` が実は App の秘密鍵から作った短命のトークンを毎回入れ替えていても、期限の無いトークンとみなす
- エージェント自身の資格情報は、ADR 0006 のとおり数えない。codex-action で `env` に `OPENAI_API_KEY` を置くとシェルから読めるが、数えない
- `$GITHUB_ENV` に書いて後のステップに渡す秘密情報や、ファイルに書いた秘密情報は見ない
- run-gemini-cli の `security.environmentVariableRedaction.allowed` で消さない名前を足していても、`settings` から参照していなければ数えない
- run-gemini-cli の GitHub のトークンは、`settings` から参照しているかに関わらず、`env` にあれば数える。公式の例の書き方で MCP サーバーに渡るため
- 秘密情報を参照しない値(`vars.*`、直接書いた文字列)は数えない
- `with` の入力のうち、エージェントに届くとした一覧の外のもの(`plugin_marketplaces` の URL など)に書いた秘密情報は見ない。各社が入力を足したときは一覧を見直す

## 再検討する条件

- 各社が、環境変数の受け渡しや消去の振る舞いを変えたとき(ADR 0003)
- 利用者から、エージェント自身の API キーがシェルから読めることも指摘してほしいと求められたとき
