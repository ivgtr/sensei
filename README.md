# Sensei

Windowsでイラスト制作に付き添う、操作中心のAIメンター。**v0.4 Rust MVP**。
Discordで先生に画面共有しながら作業する体験を目指す。通常は操作イベントを使い、必要な場合だけ画像を取得する。

## Quick start (Windows x64)

[Actions](https://github.com/ivgtr/sensei/actions/workflows/rust.yml) の成功した Windows ビルドから `sensei-windows-x64` artifact をダウンロードして展開する（GitHubログインが必要な場合があります）。まだ署名済みインストーラーではありません。

ネイティブモードには Python、Git、uv、DeskMate、Codex、外部 MCP の事前導入は不要です。PowerShellで EXE のディレクトリを開きます。

```powershell
.\sensei.exe setup --paint csp
.\sensei.exe doctor --api
.\sensei.exe run --vision
```

Kritaなら `setup --paint krita`。setupでAPIキーを非表示入力し、Windows Credential Managerに保存します。初回セットアップでは API プロバイダー → API キー → モデルの順に選択します。OpenAI、Google Gemini (公式OpenAI互換API)、Anthropic Claude (Messages API)、OpenRouter、任意のOpenAI互換HTTPS APIに対応する実装です。各プロバイダーのキーは接続先ごとにWindows Credential Managerへ保存します。ChatGPT等の月額契約とは別にAPI料金が発生する場合があります。モデル一覧・画像・JSON出力の対応は各モデルに依存します。 `setup --no-key` はプロバイダーの選択・キー入力・モデル選択を省略（明示指定がなければOpenAI）。

CSPまたはKritaを開き、描画・Undoなどを行って、Senseiが操作候補を認識するか確認します。CLIで `:status`。AIは操作がまとまり、対象アプリで手が止まったときだけ推論する設計です。必要な場合のみ画像を追加で送ります。

### API provider selection

```powershell
.\sensei.exe setup --paint csp
# または初回にプロバイダーを指定
.\sensei.exe setup --paint csp --provider gemini
# 設定済みの場合は切替（キーは接続先別に保存、登録済みなら再利用）
.\sensei.exe provider
.\sensei.exe provider anthropic
.\sensei.exe provider openrouter
.\sensei.exe provider custom --api-base https://example.com/v1 --model your-model
.\sensei.exe key
.\sensei.exe doctor --api
```

キー発行先: [OpenAI](https://platform.openai.com/api-keys) / [Google AI Studio](https://aistudio.google.com/apikey) / [Anthropic](https://console.anthropic.com/settings/keys) / [OpenRouter](https://openrouter.ai/keys)。設定画面にも発行先URLを表示します。APIキーはチャットやコマンド引数に貼らず非表示入力してください。保存したキーを優先し、未登録の場合に環境変数を参照します。環境変数は OPENAI_API_KEY / GEMINI_API_KEY / ANTHROPIC_API_KEY / OPENROUTER_API_KEY。旧 SENSEI_API_KEY はOpenAI公式接続先のみの明示的な上書きとして有効です。カスタムAPIは保存済みキーを使用します。キーを削除する場合は対象プロバイダーを選び `forget-key`（環境変数は別途解除）。他社APIはモックテスト対象で、実キー・全モデルの実機検証は未実施です。

### Model selection

```powershell
# 利用可能なモデルIDを取得（推論リクエストは行わない）
.\sensei.exe models
# 番号選択または任意のモデルIDの直接入力
.\sensei.exe model
# 使用モデルを保存（キーは再登録不要）
.\sensei.exe model gpt-4.1-mini
# このセッションだけ別モデルを使う
.\sensei.exe run --vision --model gpt-5-mini
# 初回からモデルを明示して選択画面を省略
.\sensei.exe setup --paint csp --model gpt-4.1-mini
```

一覧はキーで参照可能なモデルIDで、Chat Completions・画像入力・JSON modeへの対応を保証しません。選択中のプロバイダーの既知の候補を優先し、一覧にないIDや互換プロバイダーのモデルも手入力できます。モデル一覧取得が未対応・失敗の場合は手入力と既定候補にフォールバックします。変更後は `doctor --api` でテキスト推論の疎通、`run --vision --manual` の `:look` で画像入力を確認してください。これらは課金されるAPIリクエストです。

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

## Local observer diagnostics (v0.4)

APIキーやネット接続は不要です。未設定なら `setup --no-key --paint csp`（Kritaなら `--paint krita`）。既存設定があればそのまま使います。

```powershell
# 標準ペンHIDコレクションの検出と公称機能
.\sensei.exe devices
# CSP/Kritaに切り替えてペンをホバー、描画、離す。Ctrl+Cで停止
.\sensei.exe observe --seconds 60
# 自分の作業を明示的にローカルファイルへ記録（上書きしない）
.\sensei.exe record session.jsonl --seconds 120
# ローカル再生・統計。必要なら --timed --speed 2
.\sensei.exe replay session.jsonl
```

`devices` はHID準備済みデータから取得可能な公称フィールドを表示します。実際の配信を保証しません。 `observe` の stderr に登録状態・デコードできたレポート数・完了したネイティブストローク数を表示します。stdout はイベントのJSON要約です。 `pen_stroke` と `pressure_mean` 等が出れば取得成功です。マウス互換入力の場合は `pointer_drag_candidate` / `click_candidate` になります。

- 標準 Digitizer Pen (0x0D/0x02)・Stylus (0x0D/0x20) の Raw Input を登録し、Windows HID パーサーで Tip Switch / X / Y / Pressure / X Tilt / Y Tilt を取得。圧力と座標は論理範囲で0–1に正規化、傾きは符号付きの論理値（度ではない）。取得できない項目は null。ベンダー独自レポートの解析やWinTabの横取りはしません。
- 対象ウィンドウが前面にありカーソルが対象上にある場合のみレポートを処理。フォーカス変更・機器変更・不完全な接触で途中のストロークを破棄し、次のペンアップから再開。Raw Inputには描画先ウィンドウ情報がないため、厳密な関連付けはできません。
- ネイティブペンとマウス互換入力の二重記録を時間窓で抑制（ヒューリスティック、近接した本物のマウス操作を落とす場合あり）。マウスは引き続きサンプリング、キーボードは固定のUndo/Redo候補のみ。
- 各ストロークのサンプル数・正規化軌跡長・範囲・筆圧平均/最小/最大・傾き範囲を集計。最大128点の間引き軌跡を保持。タイムスタンプは受信処理時刻であり、センサー固有時刻ではありません。
- JSONL記録は明示指定時のみ作成され、画像・ウィンドウタイトル・文字入力・APIキーは含みません。ただし描画軌跡は作品の一部を表す可能性があります。必要な間だけ記録し、自分で管理・削除してください。リング溢れやウィンドウ切替に伴う欠落は gap として記録。中断されたファイルの末尾に破損行がある場合は修復後に再生してください。
- LLMとMCPへは従来どおり直近操作の要約・追加の集計を渡し、フル軌跡は送信しません。ネイティブのタブレット座標はキャンバス座標ではありません。実機でのペン入力互換は要検証（特にWacom/Windows Ink/WinTab）。

## Architecture

```text
Windows APIs (foreground executable / Raw Input pen / pointer / fixed shortcuts)
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

- Windows操作: windows-sys / Win32 API (8ms polling + Raw Input HID messages)
- 画像: xcap + image、対象ウィンドウをPIDとHWNDで確認、最大1280px
- MCP: 公式Rust SDK rmcp (stdio client/server)
- API: reqwest + rustls、OpenAI互換Chat Completions / Anthropic Messages
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

- 完全一致する実行ファイル名のフォアグラウンドウィンドウだけを対象とします。文書タイトル、入力文字列、クリップボード、音声は取得・保存しません。操作履歴は最大256件のメモリ上のリングに保存し、終了時に破棄します。明示的な record 時のみローカルJSONLに書き込みます。
- 標準HIDペンの取得を試み、利用できない場合は左ボタンとカーソルのサンプリングにフォールバックします。Ctrl+Z / Ctrl+Yは押下候補のみ。ペン互換性は機器依存です。レイヤー編集履歴・キャンバス座標への変換は未実装です。短いクリックはサンプリングで見逃す場合があります。
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
