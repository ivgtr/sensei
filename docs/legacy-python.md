# Sensei

デジタル作業に付き添う AI のための、操作中心（action-first）のローカル MCP。
初期版 v0.2 は **Paint MCP Facade + Windows 操作履歴の読み取り** に絞っています。
画面を常時 LLM に送らず、必要な時だけ既存ペイント MCP に画像を要求します。
モデル、常駐エージェント、音声、推論スケジューラーはまだ含みません。

## Architecture

```text
Windows / DeskMate (optional) --> desktop_recent_actions
CSP Companion MCP or Krita MCP --> paint_state / on-demand preview
                                    |
                              Sensei (stdio MCP)
                                    |
                              Local agent host
```

既存プロジェクトのプロトコルや Windows 入力取得を再実装しません。
MCP クライアントと upstream MCP は公式 Python SDK 1.x を使用。
CSP/Krita のうち一つをローカル設定で選びます。

## Quick setup (推奨)

Windows・Python 3.12・Git を前提に、Sensei が外部依存の導入と設定ファイルの生成をまとめて行います。各 MCP をホストに登録する必要はありません。

```powershell
git clone https://github.com/ivgtr/sensei.git
cd sensei
py -3.12 scripts/bootstrap.py install --paint csp --desktop
```

Krita なら `--paint krita`。DeskMate が不要なら `--desktop` を省略します。DeskMate 単独なら `--paint none --desktop --app Krita`。
既定の管理ディレクトリは `%LOCALAPPDATA%\Sensei`。CSP と DeskMate は Git コミット固定、Krita は PyPI バージョン固定です。各ツールを別の仮想環境に導入するため、CSP の MCP SDK 2.x と Sensei/Krita の 1.x が依存関係で衝突しません（通信互換性は doctor で確認）。依存パッケージのすべてをハッシュ固定したロックではありません。

```powershell
# CSP の場合：一度だけ、CSP の QR URL をローカル端末へ入力
.\.venv\Scripts\sensei-setup.exe pair
# MCP の起動・ツール検出・CSP 接続状態を確認（作品は編集しない）
.\.venv\Scripts\sensei-setup.exe doctor
# DeskMate を使う場合：別端末で明示的に開始。Ctrl+C で停止
.\.venv\Scripts\sensei-setup.exe desktop-start
```

Krita の場合は、Krita を閉じて `sensei-setup.exe krita-bridge --allowed-root "C:\Krita Work"` を実行（実在する専用フォルダを指定）。Krita 内で Python Plugin Manager の Krita Codex Bridge を有効にし再起動してください。`sensei-setup.exe krita-check` は上流の実機チェック（使い捨てドキュメントを作成して描画などをテスト）です。

ホストには `%LOCALAPPDATA%\Sensei\codex.toml` または `mcp.json` に生成された **Sensei 1件だけ**を追加してください。既存ホスト設定は勝手に上書きしません。`sensei-setup.exe host-config` で Codex 用ブロックを再表示できます。追加の管理環境を作る場合は `--root` をサブコマンドの前に指定してください。既存の管理設定を上書きする再インストールは未対応です。

DeskMate はオプション。インストール中や Sensei MCP 起動時に記録を開始しません。専用のデータディレクトリとローカルポート 3031 を使い、スクリーンショット・OCR・キー入力・クリップボード・音声を無効化する設定を生成します。ただし上流の操作イベントには他アプリのウィンドウ名等が保存される可能性があります。対象アプリの完全な収集制限ではありません。MCP 出力ではアプリ名完全一致でフィルタします（`--app` で指定、既定 CSP=CLIPStudioPaint / Krita=krita。実際の記録名に合わせてください）。最初は機密情報を含まない実験環境で確認し、不要な時は記録を停止してください。

## Manual install (既存環境を持っている場合)

Windows PowerShell、Python 3.11+（Python 3.12 推奨）。

```powershell
git clone https://github.com/ivgtr/sensei.git
cd sensei
py -3.12 -m venv .venv
.\.venv\Scripts\python.exe -m pip install -e ".[test]"
.\.venv\Scripts\python.exe -m pytest -q
```

設定なしで MCP の起動・ツール検出を試せます（sensei_status のみ）。
設定例は examples/sensei.example.json。コピーして sensei.local.json として編集してください。

## CLIP STUDIO PAINT

1. [CSP Companion MCP](https://github.com/TMAK1710/csp-companion-mcp) を別途インストールし、その README の手順で**自分でローカルの Companion ペアリング**を実施。
2. 専用のプレビュー出力ディレクトリを作成し、設定例の command/args/preview_root を実際の絶対パスに変更。
3. MCP ホストに Sensei を登録。最初に sensei_status、paint_connection_status、paint_connect、paint_state。
4. 画像が必要な場合にだけ paint_preview_gallery → paint_preview_block。

Companion は非公式で、上流の実機確認は CSP 5.1.4 の接続等に限られています。
プレビュー・操作の実機互換は別途検証が必要です。CSP のレイヤー、ストローク、正確なキャンバス座標、直接描画は未対応です。
プレビューは合成プレビューであり、必ずしも原寸ではありません。
ペアリング URL / session.json をチャットや Git に入れないでください。

## Krita

[krita-codex-mcp](https://github.com/cyyprezz/krita-codex-mcp) の公式 README と Windows 導入ガイドに従い、Krita ブリッジと MCP サーバーをインストールし、動作確認してください。
設定の backend を krita に、command をその MCP の実行ファイル（絶対パス）、args を既存 MCP 設定の引数にします。
preview_root は不要です。paint_state, paint_history, paint_preview が利用できます。

直接描画は初期状態で無効。実験用の新規ドキュメントで、手動作成した添削専用レイヤーの ID とドキュメント ID を draw_layer_id / draw_document_id としてローカル設定に指定した場合だけ paint_draw_path が公開されます。
ホスト側で毎回ユーザーに確認してください。設定の有効化自体は各回の承認を保証しません。人間の描画中には使わないでください。既存レイヤーの自動作成・削除・保存は未実装です。

## Windows actions (optional)

[DeskMate](https://github.com/zhaohb/deskmate) を別途ローカルで起動し、記録設定を確認してください。
初期状態で Sensei からは接続しません。
DeskMate が不要なアプリの画像、キー入力、音声、クリップボードを収集しないよう設定し、実験用の環境で確認してから deskmate_port (通常 3030) と allowed_apps (DeskMate が実際に記録する app_name の完全一致) を設定します。
desktop_recent_actions は /events/recent の限定サンプルを取得し、アプリとイベント種類を絞り、座標と件数のみ返します。文字入力、キーコード、タイトル、URL、クリップボードは返しません。
この出力フィルターは DeskMate 自身の収集を制限するものではありません。ペン筆圧・傾き・ストロークはまだ取得しません。

## MCP client (Codex example)

ローカルの config.toml に追記（パスは例です）。

```toml
[mcp_servers.sensei]
command = 'C:\Projects\sensei\.venv\Scripts\python.exe'
args = ['-m', 'sensei_mcp.server']
tool_timeout_sec = 60

[mcp_servers.sensei.env]
SENSEI_CONFIG = 'C:\Projects\sensei\sensei.local.json'
```

ホストとペイントソフトは同じ PC で実行してください。MCP の画像・操作履歴をモデルに渡す場合は、ホストのデータ処理ポリシーが適用されます。

## Tools

| Tool | Scope |
| --- | --- |
| sensei_status | 設定・利用可能な機能 |
| desktop_recent_actions | DeskMate の直近操作（明示的に有効化した場合） |
| paint_state | CSP ブラシ状態 / Krita ドキュメント状態 |
| paint_connection_status / paint_connect / paint_disconnect | CSP 接続 |
| paint_preview_gallery / paint_preview_block | CSP オンデマンド画像 |
| paint_history / paint_preview | Krita 履歴状態・画像 |
| paint_navigate | CSP ビュー変更（デフォルト無効） |
| paint_draw_path | Krita の設定済みレイヤーへの実演（デフォルト無効） |

実際に上流が公開するツールのみ有効。任意のコマンド、任意ファイル読み取り、任意の upstream ツール実行は公開しません。
CSP プレビューは専用ディレクトリ内の PNG のみ、最大 1600px/辺。Krita 画像は上流の PNG 結果をそのまま返します。
タイムアウト後の操作は自動再試行しません。

## Next steps

- Windows Raw Input / HID によるペン入力を、対象アプリ限定・明示的開始/停止で検証する（[Microsoft Raw Input](https://learn.microsoft.com/en-us/windows/win32/inputdev/about-raw-input)）。
- OS イベントと Paint MCP の viewport/state を時刻で結合し、ストローク集計・Undo 候補・描画領域変化を抽出する。
- ホスト側でイベント駆動の wake / watch / inspect / advise を実装する。MCP は自発的な推論のスケジューラーではない。
- 画像なし、画像のみ、操作＋画像を同じ制作セッションで比較し、費用・有益な指摘・不要な介入を評価する。
- 非破壊の画面注釈と、ローカルの介入許可 / 人間の入力優先制御を追加する。

## Reused assets and validation

- [MCP Python SDK](https://github.com/modelcontextprotocol/python-sdk) — transport/client/server
- [CSP Companion MCP](https://github.com/TMAK1710/csp-companion-mcp) — CSP protocol, MIT
- [Krita Codex MCP](https://github.com/cyyprezz/krita-codex-mcp) — Krita bridge, MIT
- [DeskMate](https://github.com/zhaohb/deskmate) — Windows activity recording, MIT

既存プロジェクトは独立した依存サービスです。管理インストーラーは必要なものだけを導入します。ソースコードは Sensei に取り込んでいません。
実機の CSP/Krita/DeskMate 連携は未検証です。ユニットテストと stdio スモークテストを同梱しています。
