# Sensei

Windowsでイラスト制作に付き添う、操作中心のAIメンター。**v0.3 Rust MVP**。
Discordで先生に画面共有しながら作業する体験を目指す。通常は操作イベントを使い、必要な場合だけ画像を取得する。

## Quick start (Windows x64)

[Actions](https://github.com/ivgtr/sensei/actions/workflows/rust.yml) の成功した Windows ビルドから `sensei-windows-x64` artifact をダウンロードして展開する（GitHubログインが必要な場合があります）。まだ署名済みインストーラーではありません。

ネイティブモードには Python、Git、uv、DeskMate、Codex、外部 MCP の事前導入は不要です。PowerShellで EXE のディレクトリを開きます。

```powershell
.\sensei.exe setup --paint csp
.\sensei.exe doctor --api
.\sensei.exe run --vision
```

Kritaなら `setup --paint krita`。setupでAPIキーを非表示入力し、Windows Credential Managerに保存します。OpenAI Chat Completions API、既定モデル gpt-4.1-mini。利用できるモデルを `--model`、OpenAI互換HTTPSエンドポイントを `--api-base` で明示できます（JSON mode・画像・max_completion_tokensの互換性は各サービス依存）。APIは別料金です。環境変数 SENSEI_API_KEY も使えます。setup --no-key なら入力を省略できます。

CSPまたはKritaを開き、描画・Undoなどを行って、Senseiが操作候補を認識するか確認します。CLIで `:status`。AIは操作がまとまり、対象アプリで手が止まったときだけ推論する設計です。必要な場合のみ画像を追加で送ります。

### CLI interaction

| Command in run | Meaning |
| --- | --- |
| 普通の文章 | 質問。操作概要から判断し、必要なら画像を要求 |
| :look 肩の位置を確認して | 画像を一度取得して質問 |
| :status | 監視対象の検出・直近イベント数 |
| :pause / :resume | 自動推論の停止・再開（ローカル観測は続く） |
| :accept / :reject | AIのビュー変更提案を明示承認・破棄（60秒以内） |
| :zoom in / out / fit / reset | Companion接続時のみ、明示的にCSPのビュー変更を要求（resetは回転リセット） |
| :quit / Ctrl+C | セッション終了・観測停止 |

画像をモデルに渡さない場合は `run`（--visionなし）。自動助言を無効にするには `run --manual`。通常は `run --vision --max-calls-per-hour 20 --cooldown-seconds 60`。呼び出し上限はセッション内の移動1時間（手動・画像再推論も含む）。API料金そのものの上限ではありません。無操作時・非対象アプリ使用中は自動呼び出しを行いません。

`:pause` は監視自体の停止ではない点に注意してください。観測を停止するにはセッションを終了してください。

### Optional: application-native MCP (CSP)

通常のネイティブモードは外部MCPなしで動きます。CSPのブラシ状態とビュー操作が必要なら、Senseiが既存の [CSP Companion MCP](https://github.com/TMAK1710/csp-companion-mcp) を固定コミットで管理します。PythonソースをSensei本体に取り込まず、上流だけを隔離環境で実行します。

Senseiが [uv](https://docs.astral.sh/uv/) の固定バージョンのWindows x64 ZIPを取得してSHA-256を検証し、Companionの固定コミットの必要ファイルもGit blobハッシュで検証します。uvがPython 3.12と仮想環境・依存パッケージを管理します。Git/uv/Pythonの手動インストールは不要です（インターネット接続は必要）。

```powershell
# 既にsetup済みの場合
.\sensei.exe install-companion
# または新規setup時に --companion
.\sensei.exe pair
.\sensei.exe doctor
.\sensei.exe run --vision
```

pairのみCSPの「スマートフォンと接続」QRのURLを手動で取得し、ローカル端末に入力します。絶対にチャットに貼らないでください。既存のペアリングデータは上書きしません。Companion起動・接続に失敗してもネイティブ観測で続行します。依存の取得に失敗した場合は install-companion で再試行できます（未完成の companion-download が残っている場合は内容を確認してから取り除いてください）。ブラシ状態は上流が提供する数値・真偽値フィールドだけを使います。

Companionは非公式プロトコルです。CSPのプレビュー・変更コマンドの実機互換は未確認。ビュー操作はユーザーの明示的なCLI入力またはAIの提案を :accept で承認した場合のみ。モデルからの自動実行は行いません。既存レイヤーの編集・保存・直接描画はMVP外です。Krita固有MCPのRust側統合は今後の拡張です（旧Python版には実装があります）。

## External MCP client (optional)

Sensei本体に直接LLM接続があるため、通常は別のMCPホスト不要です。Codex等で使う場合は次の1サーバーのみを登録できます。

```toml
[mcp_servers.sensei]
command = 'C:\Tools\sensei.exe'
args = ['mcp', '--vision']
```

`--vision` を省略すれば画像をMCPから取得できません。ネイティブMCPは `sensei_status` / `desktop_recent_actions` / `paint_preview` を公開します。読み取りのみで、MCPホストからAPIキーや操作コマンドを扱いません。起動中は対象アプリの観測が動き、ホスト終了で停止します。Companion連携は現在CLIのmentor loopのみです。

## Architecture

```text
Windows APIs (foreground executable / pointer / fixed shortcuts)
        |     (no frames, no typed text)
        v
Bounded in-memory action ring ----> Gate / rolling budget
        |                                  |
        |                           OpenAI-compatible LLM
        |                                  |
        +---- xcap target-window PNG <--- inspect only
                                           |
                                     observe / advise
                                           |
                         Optional CSP Companion MCP (rmcp client)
                           read brush / explicit CLI navigation

Native rmcp server <-- same Observer (separate session)
```

- Windows操作: windows-sys / Win32 API (25ms sampling)
- 画像: xcap + image、対象ウィンドウをPIDとHWNDで確認、最大1280px
- MCP: 公式Rust SDK rmcp (stdio client/server)
- API: reqwest + rustls、OpenAI互換Chat Completions
- APIキー: keyring / Windows Credential Manager、rpasswordで非表示入力
- 外部依存: uv、必要時のみCSP Companionを隔離環境に導入

## MVP acceptance checklist (manual)

- EXEを起動しsetup・APIキー登録・doctor --apiが成功する。
- CSP/Kritaで描くと :status のイベントが増え、別アプリでは新規記録しない。
- ペンを置いたときに、操作のまとまりに対して自発的な助言または沈黙を返す。
- :lookで対象ウィンドウの画像付きの講評を受ける（--visionを指定）。
- :pauseで自動API呼び出しが停止し、:quitで観測が終了する。
- 任意: CSP Companionをinstall/pairし、ブラシの状態取得と:zoom fit、AIからのビュー変更提案と:acceptを確認する。

上記は実機受け入れ条件であって、実行済みという意味ではありません。

## Privacy & limitations

- 完全一致する実行ファイル名のフォアグラウンドウィンドウだけを対象とします。文書タイトル、入力文字列、クリップボード、音声は取得・保存しません。操作履歴は最大256件のメモリ上のリングに保存し、終了時に破棄します。
- 左ボタンとカーソルをサンプリングしてドラッグ・クリック候補を集約し、Ctrl+Z / Ctrl+Yの押下候補を記録します。ペンタブレットがマウス互換入力を出さない場合はストロークを拾えません。Raw HID筆圧・傾き・ネイティブストローク・レイヤー編集履歴は未実装。座標はウィンドウ相対でキャンバス座標ではありません。短いクリックもサンプリングで見逃す場合があります。
- 画像は明示的に --vision を指定し、質問またはモデルのinspect判断が発生したときのみ取得。対象アプリのウィンドウ全体で、キャンバス以外のUIや作品名が写る場合があります。画像・操作要約・質問は指定したAPIへ送信されます。APIを利用する前に対象データとプロバイダーを確認してください。
- APIキーはローカル設定JSONに保存せず、HTTPS以外のAPIを拒否し、リダイレクトを無効化します。カスタムAPIの信頼性はユーザーが確認してください。 `sensei forget-key` で登録キーを削除できます。
- 自動助言はAPI呼び出し中に描画を再開した場合、表示を抑制します。描画の精度・ペンタブ互換・CSP/Kritaとの実機検証は別途必要。MVPはテキスト助言と限定ビュー操作であり、音声、赤ペン、描画実演、長期記憶は未実装。

## Development

```powershell
cargo test
cargo build --release
.\target\release\sensei.exe --help
```

WindowsおよびLinuxのCIでRustビルドと単体テスト。Windows x64 release EXEをActions artifactとして生成。Windows CIでMCP stdio smoke testも実行します。実際のイラストソフト、タブレット、課金APIはCIで操作しません。

旧Python版v0.2は互換用に `src/sensei_mcp` と `pyproject.toml` を残しています。新規開発・EXE配布はRust版が対象。旧版の設定とは分離して `%LOCALAPPDATA%\Sensei\rust` を使用します。[旧版の手順](docs/legacy-python.md)。
