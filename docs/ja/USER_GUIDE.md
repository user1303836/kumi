# ユーザーガイド

[English](../en/USER_GUIDE.md) · [简体中文](../zh-CN/USER_GUIDE.md) · 日本語

ブリッジは Kumi と Ableton Live をつなぐもので、どの MCP クライアントからも単独で使えます。ブリッジは二つの部分からなります。

- ローカルの MCP サーバー `@ableton-mcp/mcp-server`
- Live 12 の中で動く Remote Script `AbletonMcpBridge`

サーバーは認証付きのループバック接続で Remote Script と通信します。これによりクライアントは開いている Set を読み、変更できます。変更はそれぞれプレビューしてから適用し、必要なときには取り消せます。クライアントはオーディオの再生、録音、解析もできます。

このガイドでは、セットアップ、設定、デプロイメントポリシー、変更のしくみ、そしてすべてのツールを説明します。Kumi を使っている場合は、`kumi bridge` がセットアップをすべて行います。[Kumi ガイド](KUMI_GUIDE.md)を参照してください。

## インストール

ネイティブのサーバーは、macOS、Windows、Linux 上で実行ファイルとして動きます。Live に接続するには macOS か Windows が必要です。自分の OS とアーキテクチャに合ったアーカイブを使ってください。解析ワーカーはサーバーの隣に置いたままにする必要があります。別途 Node のランタイムを用意する必要はありません。

- **Kumi で使う：** Live を閉じて `kumi bridge` を実行します。
- **単体で使う：** [ブリッジのインストール](DELIVERY.md)の説明に従い、`ableton-mcp-server lifecycle` でネイティブの tarball をインストールします。
- **ソースから：** リポジトリのルートで `cargo build --release --locked -p ableton-mcp-server --bins` を実行します。`target/release/ableton-mcp-server` は、設定を与えられるまではオフラインのツールだけで起動します。

## Live に接続する

ライフサイクルのインストーラーは、所有者専用のシークレットと設定を作成し、Remote Script をインストールし、アップグレードとロールバックのためのレシートを残します。Windows でもライフサイクルを使ってください。必要なファイルのアクセス権を付けてくれます。[ブリッジのインストール](DELIVERY.md)の手順に従ってください。

次に Live を開き、**Settings → Link, Tempo & MIDI** で **AbletonMcpBridge** を選びます。接続は次のように確認します。

```sh
/absolute/path/ableton-mcp-server diagnostics --config /absolute/path/bridge-config.json
```

`"provenance": "real-live"` と `"readiness": { … "realLiveOperational": true }` を探してください。診断は接続されていなくても正常に終了することがあるので、レポートの readiness のフィールドを読んでください。

### 設定ファイル

`ableton-mcp-server setup` はバージョン 2 のファイルを書き出します。サーバー、Remote Script、ライフサイクルのすべてがこのファイルを読みます。

```json
{
  "version": 2,
  "server": {
    "command": "/absolute/path/ableton-mcp-server",
    "args": ["--config", "/absolute/path/bridge-config.json"]
  },
  "bridge": {
    "host": "127.0.0.1",
    "port": 9765,
    "secretFile": "/absolute/path/bridge.secret",
    "timeoutMs": 5000,
    "realtimePort": 9766
  }
}
```

| フィールド | ルール |
| --- | --- |
| `server.command` | ネイティブのサーバーの実行ファイルの絶対パス |
| `server.args` | `--config` と、このファイル自身の絶対パス |
| `bridge.host` | `127.0.0.1` または `::1` |
| `bridge.port` | 1–65535。Remote Script がここで待ち受けます |
| `bridge.secretFile` | 絶対パス。所有者専用で、32 文字以上 |
| `bridge.timeoutMs` | Live への 1 リクエストあたり 100–60,000 ms（デフォルト 5,000） |
| `bridge.realtimePort` | 省略可。`port` とは異なる値にします。[リアルタイムコントロール](REALTIME_CONTROL.md)を参照 |
| `bridge.diagnostics` | 省略可。`ableton-mcp-server lifecycle install --enable-bridge-diagnostics` だけが書き込みます（[運用ガイド](OPERATIONS.md)を参照） |

Node と `cli.js` を指定した旧形式のバージョン 2 の設定も、移行のあいだは読み込めます。未知のフィールドは拒否されます。ファイルは自分だけが読めるようにしておく必要があります。ブリッジのオプションなしで `ableton-mcp-server setup` を実行すると、バージョン 1 のファイルが書き出されます。このファイルはサーバーの起動方法しか記述しておらず、`--config` に渡すと拒否されます。古いファイルは `ableton-mcp-server migrate` で変換できます（[ブリッジのインストール](DELIVERY.md)を参照）。

## MCP クライアントにブリッジを追加する

設定の `server.command` と `server.args` を使います。よく使われる `mcpServers` 形式では次のようになります。

```json
{
  "mcpServers": {
    "ableton": {
      "command": "/absolute/path/ableton-mcp-server",
      "args": ["--config", "/absolute/path/bridge-config.json"],
      "env": { "ABLETON_MCP_TOOL_POLICY": "edit-no-audio" }
    }
  }
}
```

サーバーは stdin と stdout で、JSON lines として MCP をやり取りします。自分のログ行は、先頭に `mcp-host:` を付けて stderr に書き出します。

## Kumi の Live 拡張機能

Live 12.4 以降では、Kumi の Live 拡張機能が動いていれば、ブリッジはそれにも接続します。これで次のことができるようになります。

- オフラインレンダリング
- アレンジメントへ直接書き込む MIDI クリップ
- アレンジメントの一区間の消去
- デバイスの複製
- プロジェクトへのファイルの取り込み
- 「Ask Kumi about this」の右クリックイベント

拡張機能は、次のどちらかの方法で動きます。

- **Live の Extensions フォルダにインストールする。** `kumi bridge` がそこに置き、Live が起動するときに拡張機能を開始します。
- **ブリッジが起動する。** Live の Developer Mode がオンのとき（Settings → Extensions）、Live 自身の Extension Host を通じて起動します。

Live が接続している間、ブリッジは 10 秒ごとに拡張機能を探します。拡張機能が応答すると、そのツールが `tools/list` に現れます。[ツールリファレンス](#live-拡張機能のツール)を参照してください。それ以外の機能は、拡張機能がなくてもすべて動きます。

| 変数 | 効果 |
| --- | --- |
| `ABLETON_MCP_EXTENSION=off` | 拡張機能に接続しない |
| `ABLETON_MCP_EXTENSION=external` | 動いている拡張機能に接続するが、自分では起動しない |
| `ABLETON_MCP_EXTENSION_DIR` | ブリッジが起動した拡張機能が、エンドポイント、シークレット、レンダリングを置く場所（デフォルト：設定の隣の `live-extension`） |
| `ABLETON_MCP_LIVE_EXTENSIONS_DIR` | Live の Extensions フォルダ。デフォルト（`~/Library/Application Support/Ableton/Extensions`、`%LOCALAPPDATA%\Ableton\Extensions`）以外の場合に指定します |

## コマンド

| コマンド | オプション |
| --- | --- |
| `ableton-mcp-server` | なし、または `--config PATH` のみ |
| `ableton-mcp-server setup` | `--output PATH`。バージョン 2 ではさらに `--bridge-port N`、`--secret-file PATH`、必要に応じて `--bridge-host`、`--bridge-timeout MS`、`--realtime-port N`。`--force` で上書きします。 |
| `ableton-mcp-server install-remote-script` | `--destination DIR`、`--config PATH`、`--dry-run`、`--force` |
| `ableton-mcp-server diagnostics` | なし、または `--config PATH` のみ。JSON のレポートを出力します |
| `ableton-mcp-server lifecycle`、`ableton-mcp-server migrate` | [ブリッジのインストール](DELIVERY.md)を参照 |

最初の四つのコマンドは、不正なオプションでは 2、失敗したときは 1 で終了します。

## プロトコル

サーバーは MCP プロトコルの二つのバージョンに対応しています。サーバープロセスごとに、どちらか一方を使ってください（`2025-11-25` の `initialize` の前に `server/discover` を送ることはできます）。

- **`2025-11-25`：** `initialize` を送り、続けて `notifications/initialized` を送ります。サーバーは `notifications/tools/list_changed` と Live のイベントを送ります（[イベント](#イベント)を参照）。
- **`2026-07-28`：** ハンドシェイクはありません。すべてのリクエストが、`params._meta` に `io.modelcontextprotocol/protocolVersion` と `io.modelcontextprotocol/clientCapabilities` を含めます。`server/discover` は省略できます。

  ```json
  {"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{}}}}
  ```

  結果には `resultType: "complete"` が付き、ツールの結果は JSON を `structuredContent` にも入れて返します。一覧とリソースには `ttlMs: 0` が付きます。キャッシュせずに読み直してください。このバージョンにはプッシュ通知がありません。`live_subscribe` の代わりに `live_observe_poll` を使ってください。

`tools/list` は、接続している Live が対応していて、デプロイメントポリシーが許可しているツールだけを、その時点で表示します。一覧は、Live の接続、切断、再接続や、Live 拡張機能が現れたり消えたりするのに応じて変わります。`2025-11-25` では、サーバーは変化のたびに `notifications/tools/list_changed` で知らせます。`capabilities` と `ableton://capabilities` リソースは、隠れているツールとその理由も示します。

## デプロイメントポリシー

デプロイメントポリシーは、クライアントがどのツールを見て、呼び出せるかを決めます。サーバーの環境で設定します。

| 変数 | 値 |
| --- | --- |
| `ABLETON_MCP_TOOL_POLICY` | プロファイル：`read-only`、`edit-no-audio`、`performance`、`full`（デフォルト） |
| `ABLETON_MCP_TOOL_ALLOW` | カンマ区切りのツール名または `prefix*` パターン。プロファイルの範囲内で、これらだけを許可します |
| `ABLETON_MCP_TOOL_DENY` | カンマ区切りの名前またはパターンで、決して許可しないもの。拒否が常に優先します |

| プロファイル | 許可するもの |
| --- | --- |
| `read-only` | ローカルのツールと読み取り。変更はできません |
| `edit-no-audio` | 読み取りと編集：構造、MIDI、デバイス、ミキサー、オートメーション、ルーティング。再生、録音、キャプチャ、ファイルの書き込み、Python の実行は含みません。 |
| `performance` | 読み取りに加えて、再生、ビューと選択、ミキサー、テンポ、`live_undo` と `live_recovery_finalize` |
| `full` | `python` を含むすべてのクラス |

各ツールにはクラスが一つあり、[ツールリファレンス](#ツールリファレンス)に記載しています：`local`、`read`、`edit`、`performance`、`audio`、`filesystem`、`recording`、`realtime`、`capture`、`python`。クラスとツールの実際の動作が一致しないものは次のとおりです。

- `live_render_offline` はレンダリングファイルを書き出しますが、`read` です。
- `.als` のツールは読み取りしかしませんが、`filesystem` です。
- `live_change` は `edit` なので、`performance` には含まれません。

ポリシーは呼び出しのたびに確認し直されます。ポリシーがもう許可していないツールで行った変更に対しては、`live_undo` が拒否されます。値が不正だと、サーバーは起動時に停止します。`ableton-mcp-diagnostics` は、有効になっているポリシーを報告します。

完全には信頼できないクライアントには、まず `read-only` か `edit-no-audio` を使ってください。`full` では、Live の中で任意の Python を実行する `live_run_python` を拒否してください。

## 変更のしくみ

### プレビュー、適用、取り消し

変更は三つのステップで行います。

1. 変更するものを**読み取り**（`live_discover`、`live_snapshot`）、その ref を取得します。
2. `*_preview` ツールで**プレビュー**します。プレビューは何も変更しません。何が変わるか、`transactionId`、`confirmation`、`expiresAt` を返します。
3. 対応する `*_apply` ツールで**適用**します。`transactionId`、`confirmation`、そして自分で決めた `idempotencyKey`（8–128 文字）を渡します。ブリッジは Live のスレッド上で、プレビュー以降に何も変わっていないことを確かめてから変更を適用し、結果を読み戻します。

同じキーで同じ適用をもう一度送ると、もう一度同じ答えが返り（`"idempotent": true`）、二重に適用されることはありません。`transactionId` を保管しておけば、`live_undo`（`confirmation: "undo"`）で変更を取り消せます。

`live_change` は、プレビューと適用を一度の呼び出しで行います：`{"tool": "live_mixer_preview", "args": {…}}`。答えは適用の結果で（プレビューの結果は `preview` の下にあります）、`live_undo` もいつもどおり使えます。人が事前に確認すべき変更は拒否します：オーディション、クリップの起動、起動ボタン、キャプチャ、録音、リアルタイムのアーム、Live のダイアログです。

### 確認トークンと有効期限

- ほとんどのプレビューは、確認トークンとして `"apply"` を返します。
- シーンのオーディションとクリップの起動は、予測できないトークンと、停止用の別のトークンを返します。キャプチャは予測できないトークンを返します。
- いくつかのツールは、独自の語を受け取ります：`"undo"`、`"backup"`、`"disarm"`、`"undo-in-live"`、`"redo-in-live"`、`"emergency-stop"`、`"emergency-stop-and-clean"`、`"finalize-recovery-record"`。

プレビューは 10 分で期限切れになります。バッチ、MIDI クリップ、デバイス状態のプレビューは 30 秒、キャプチャのプレビューは 60 秒で期限切れになります。期限が切れたら、プレビューし直してください。

再生や録音を行うツールのスキーマには、`outputSafety` オブジェクト（`{"safe": true, "provenance": "…"}`）があります。これが必須なのはシーンのオーディションだけです。ほかのツールでは、クライアントが渡さなければブリッジが独自のものを使います。ただし、スキーマを厳密に守るクライアントは、スキーマが必須としている箇所では送る必要があります。

### ブリッジが調整すること

- 範囲外のパラメータ値は、近いほうの端に合わせます。段階的なパラメータで段と段の間にある値は、最も近い段に合わせます。拒否されるのは、Live がグレーアウトしているパラメータだけです。
- トラックやシーンの名前は重複してもかまいません。ただし、バッチが作成するトラックは別です。
- `seed` のないランダムな MIDI 変換は、リクエストから seed を導き出すので、プレビューと適用の結果が一致します。

### 変更が拒否されたとき

拒否された呼び出しは、`isError: true` と `{"reason": "...", "remediation": "..."}` を返します。理由はブリッジまたは Live 自身のものです。

- "Nothing changed in Live"：理由に書かれていることを直してから、プレビューし直してください。
- "Live state changed since the preview"：読み直してから、プレビューし直してください。
- タイムアウト、応答の消失、読み戻しの失敗があると、変更は不確定のままになります。同じキーで同じ適用だけを再試行してください。[回復手順](RECOVERY.md)を参照してください。

### 取り消し

`live_undo` は、その後オブジェクトがどう変わっていても変更を元に戻します。名前を変えたトラック、もう一度動かしたフェーダー、クリップが加わったデバイスなどです。ref が今は別のオブジェクトを指している場合は拒否します。

`live_undo` で取り消せない変更もあります。

- 削除、アレンジメントの一区間の消去、`live_run_python`、そしてプレビューで kept（残る）とされるその他の変更（クロップ、保存したラックのバリエーション、クリアしたパッド）。これらは Live 自身の取り消し（`live_song_undo`）で戻せます。
- 再生：クリップの起動、シーンの起動、トランスポートの操作、起動ボタン。戻すには停止してください。

`live_undo_step_begin` と `live_undo_step_end` は、変更をまとめて Live での Cmd-Z 一回分にします。このステップは、`timeoutMs`（デフォルトは 2 分）が過ぎたとき、接続が切れたとき、または別のステップが開かれたときに自動で閉じます。Live 拡張機能を通じた変更はまとめられません。

サーバーは、適用した変更の取り消しを保持します。記録は全体で最大 1 GiB、バッチ、MIDI クリップ、デバイス状態の変更はそれぞれ 512 件までです。それを超えると、最も古い適用済みの変更から取り消しができなくなります。使わない取り消しは `live_transaction_release` で手放せます。取り消しの記録はサーバーのメモリ上にあり、再起動すると失われます。

[Live の安全性](LIVE_SAFETY.md)では、ブリッジが何を保証するのか、そしてプレビューと適用の外で動くツールについて説明しています。

## ツールリファレンス

サーバーが提供しうるすべてのツールです。`tools/list` には、接続している Live が対応していて、ポリシーが許可しているものだけが表示されます。`name_preview/apply` は `name_preview` と `name_apply` の組を表し、`/stop` が付くと `name_stop` が加わります。クラスは[デプロイメントポリシー](#デプロイメントポリシー)のクラスです。

### ステータスとオフラインのツール

これらは Live なしで動きます。例外は `live_status` で、Live がいるかどうかを報告します。

| ツール | クラス | 内容 |
| --- | --- | --- |
| `server_status` | local | サーバーのバージョンと、Live アダプターが接続されているかどうか。 |
| `capabilities` | local | ネゴシエートされた機能と、どのツールが実行可能か、表示されているか、ポリシーで拒否されているか。 |
| `live_status` | read | Live との接続：プロトコル、アダプター、来歴（`real-live`）、エポック、レジストリハッシュ、機能と操作。ブリッジが切れていれば、先に再接続します。常に一覧に表示されます。 |
| `plan_user_journey` | local | 五つのガイド付きジャーニーのいずれかのプラン。何も変更しません。[作業例](USER_JOURNEYS.md)を参照。 |
| `audio_analyze` | local | 送った float32 PCM のラウドネス（BS.1770-5 / EBU R128）、トゥルーピーク、スペクトル、ダイナミクス、クリッピング。 |
| `audio_compare_reference` | local | 自分の PCM をリファレンスと比べます：アライメント、レベル合わせ、違い。 |
| `als_read/lint/diff` | filesystem | 指定した `allowedRoot` の中にある保存済み `.als` ファイルを、Live なしで読み取り、lint、diff します。 |
| `live_project_snapshot_diff` | read | エクスポートした二つの Set スナップショットを、Live なしで比べます。 |
| `live_library_search` | read | 許可したフォルダの中で、Live 自身のライブラリデータベース（ファイル、タグ、プラグイン）を検索します。読み取り専用。 |

### Set を読む

これらの読み取りで得られる ref（`<epoch>:track:4` など）を、変更用のツールに渡します。ref は Live のエポックが変わるまで有効です。

| ツール | クラス | 内容 |
| --- | --- | --- |
| `live_snapshot` | read | Set 全体のスナップショットを、上限付きで一つ。 |
| `live_discover` | read | 一種類のオブジェクトをページ単位で返します：`set`、`track`、`return-track`、`main-track`、`scene`、`clip-slot`、`session-clip`、`arrangement-clip`、`note`、`locator`、`device`、`parameter`、`selection`、`routing-choice`、`session-playback`。クリップスロット、クリップ、ノート、パラメータ、ルーティングの選択肢には `parent` の ref が必要です。フィルターは最大 8 個、`fields` の指定、`limit`、`cursor`。 |
| `live_song_state` | read | ソング全体の状態：拍子、スウィング、録音とオーバーダブのモード、アームとソロのモード、Link。 |
| `live_performance_read` | read | CPU 負荷、トラックのメーター、デバイスのレイテンシーを一度だけ取得します。 |
| `live_note_read` | read | MIDI クリップのノートを id で、または選択中のノートを読みます。 |
| `live_key_estimate` | read | MIDI クリップまたはノートの一覧から、キーの候補を順位付けして返します。 |
| `live_project_info` | read | 保存した Set のファイル、参照しているメディア、見つからないもの。 |
| `live_project_snapshot_export` | read | プライバシーフィルターをかけた Set スナップショット（`strict`、`collaboration`、`local`）の 1 ページ。保存して、あとで diff できます。 |
| `live_automation_read` | read | セッションクリップの一つのパラメータのエンベロープと、ある拍での値。 |
| `live_arrangement_automation_read` | read | アレンジメントクリップの一つのパラメータのエンベロープのポイント。 |
| `live_take_lane_read` | read | トラックのテイクレーンとそのクリップ。 |
| `live_comp_read` | read | コンプしたクリップを構成するテイクレーンのセグメント。 |
| `live_warp_marker_read` | read | オーディオクリップのワープマーカー。 |
| `live_device_read` | read | プラグインのすべてのパラメータ名、または Max for Live デバイスのバンク。 |
| `live_clip_time_convert` | read | オーディオクリップ内で、拍、サンプルフレーム、秒を相互に変換します。 |
| `live_data_read` | read | Set やトラックにキーで保存したテキスト。 |
| `live_browser_roots` | read | Live のブラウザのルート。 |
| `live_browser_search` | read | カテゴリと語句による、Live のブラウザの順位付き検索。 |
| `live_browser_inspect` | read | id で指定したブラウザ項目一つ：それが何か、読み込めるかどうか。 |

### 変化を追う

| ツール | クラス | 内容 |
| --- | --- | --- |
| `live_subscribe` | read | Live が変わるたびに `notifications/live_event` を送ります（[イベント](#イベント)を参照）。旧プロトコルのみ。 |
| `live_unsubscribe` | read | その通知を止めます。 |
| `live_observe_subscribe/poll/unsubscribe` | read | 変化したトピック（トランスポート、選択、トラック、クリップ、デバイス、パラメータ、グルーヴ、チューニング、シーン、メーター、ラック）をポーリングで取得するオブザーバー。どちらのプロトコル世代でも動きます。 |

### トラック、シーン、構造

| ツール | クラス | 内容 |
| --- | --- | --- |
| `live_session_structure_preview/apply` | edit | 指定した位置に MIDI トラック、オーディオトラック、シーンを作成します。名前は重複してもかまいません。 |
| `live_track_structure_preview/apply` | edit | リターントラックの作成や削除。トラックやシーンの複製。 |
| `live_scene_capture_preview/apply` | edit | 再生中のものを新しいシーンにキャプチャします。 |
| `live_object_rename_preview/apply` | edit | トラック、シーン、クリップ、デバイス、ロケーター、テイクレーンの名前を変えます。 |
| `live_track_properties_preview/apply` | edit | トラックの色（パレットのインデックス 0–69）。 |
| `live_scene_preview/apply` | edit | シーンの色、テンポ、拍子。 |

### 削除

削除は kept です：`live_undo` では戻せませんが、Live 自身の取り消し（`live_song_undo`）なら戻せます。

| ツール | クラス | 内容 |
| --- | --- | --- |
| `live_track_delete_preview/apply` | edit | オーディオ、MIDI、グループトラックを、クリップとデバイスごと削除します。グループは中のトラックも一緒に削除します。 |
| `live_scene_delete_preview/apply` | edit | シーンとそのクリップを削除します。Set には少なくとも一つのシーンが残ります。 |
| `live_clip_delete_preview/apply` | edit | セッションまたはアレンジメントのクリップを削除します。 |
| `live_locator_delete_preview/apply` | edit | ロケーターを削除します。 |
| `live_device_delete_preview/apply` | edit | デバイスを削除します。 |

### セッションクリップ、ノート、クリップのオートメーション

| ツール | クラス | 内容 |
| --- | --- | --- |
| `live_midi_clip_preview/apply` | edit | 空のセッションスロットに、ノート付きの MIDI クリップを作成します。 |
| `live_note_update_preview/apply` | edit | id で指定したノートを変更します：ピッチ、開始位置、長さ、ベロシティ、ミュート、確率、ベロシティのばらつき、リリースベロシティ。 |
| `live_note_delete_preview/apply` | edit | id で指定したノートを削除します。 |
| `live_note_edit_preview/apply` | edit | ノートのクオンタイズや複製、ノートの選択、ピッチと時間の範囲内にあるノートの削除。 |
| `live_midi_transform_preview/apply` | edit | 変換とジェネレーター：トランスポーズ、スケール、クオンタイズ、スウィング、ヒューマナイズ、アルペジエート、ユークリッドリズム、コード進行、ドラムパターン、ベースライン、モチーフの反転など。ランダムなものは `seed` を受け取るか、リクエストから導き出します。ジェネレーターはデフォルトで、空のスロットにコピーとして書き込みます。 |
| `live_capture_midi_preview/apply` | edit | Live の Capture MIDI。 |
| `live_clip_properties_preview/apply` | edit | クリップのミュート、色、MIDI ループ、起動モードとクオンタイズ、レガート、RAM モード、ベロシティ量、グルーヴ。 |
| `live_clip_action_preview/apply` | edit | クロップ、ループや範囲の複製、スクラブ、再生位置の移動。 |
| `live_clip_duplicate_preview/apply` | edit | セッションクリップを、別のスロットやアレンジメントにコピーします。 |
| `live_clip_move_preview/apply` | edit | アレンジメントクリップを移動します。またはセッションクリップを別のスロットへ移動します。 |
| `live_automation_preview/apply` | edit | セッションクリップのエンベロープ：作成や削除、ポイントの挿入や削除、ステップの描画、すべてのエンベロープの消去。 |

### アレンジメント

| ツール | クラス | 内容 |
| --- | --- | --- |
| `live_arrangement_section_preview/apply` | edit | セクションの前後に、名前付きのロケーターを二つ追加します。 |
| `live_arrangement_clip_preview/apply` | edit | アレンジメントに、空の MIDI クリップ、または `filePath`（そのまま Live に渡します）からオーディオクリップを作成します。 |
| `live_locator_jump_preview/apply` | performance | 再生ヘッドを、次の、前の、または指定したロケーターに移動します。 |

### オーディオクリップとファイル

オーディオの取り込み、Simpler やドラムパッドへの読み込みには、ファイルパスと、そのファイルを含む `allowedRoot` フォルダを渡します。ブリッジはファイルを確認し、管理フォルダに置いたコピーを Live に渡します（[Live の安全性](LIVE_SAFETY.md)を参照）。

| ツール | クラス | 内容 |
| --- | --- | --- |
| `live_audio_clip_preview/apply` | audio | オーディオクリップのゲイン、ピッチ、ループ、ワープ、フェード（クリップが対応している範囲で）。 |
| `live_warp_marker_preview/apply` | audio | ワープマーカーを拍の位置で追加、移動、削除します。 |
| `live_audio_import_preview/apply` | filesystem | オーディオファイルを、空のセッションスロットまたはテイクレーンに置きます。MIDI ファイルは拒否されます。 |
| `live_simpler_preview/apply` | filesystem | Simpler のサンプルを差し替えます。 |
| `live_project_backup_preview/apply` | filesystem | 保存した Set の検証済みコピーを、その隣に作ります。プレビューには `confirmation: "backup"` と、Set を含む `allowedRoot` を渡します。 |

### デバイス、ラック、ブラウザ

| ツール | クラス | 内容 |
| --- | --- | --- |
| `live_browser_load_preview/apply` | edit | ブラウザの項目を、トラックのデバイスの後ろ、またはラックのチェーン（`chainRef`）に読み込みます。トラックへの二つ目のインストゥルメントは拒否されます。 |
| `live_device_preview/apply` | edit | ネイティブデバイスを名前で挿入する（Simpler はサンプルを同時に読み込めます）、デバイスをオン・オフする、デバイスを移動する。 |
| `live_device_parameter_preview/apply` | edit | パラメータを一つ、または `values` で一つのデバイスのパラメータを最大 10,000 個まとめて設定します。範囲外の値は端に留め、段と段の間の値は最も近い段に合わせます。 |
| `live_device_state_save` | filesystem | デバイスやラックのパラメータ値を、指定したフォルダの JSON ファイルに保存します。 |
| `live_device_state_recall_preview/apply` | read, edit | 保存した状態をデバイスに呼び戻します。または二つの状態の間をモーフィングします。 |
| `live_device_advanced_preview/apply` | edit | パラメータバンク、オートメーションの再有効化、A/B の保存、チェーンへの挿入、別のトラックやチェーンへの移動。 |
| `live_device_specialized_preview/apply` | edit | Drift（モジュレーションマトリクスを含む）、Drum Cell、EQ Eight、Hybrid Reverb、Meld、プラグインのプリセット、Simpler のサンプル設定、Wavetable。 |
| `live_device_edit_preview/apply` | edit | パラメータではない設定（Roar、Shifter、Spectral Resonator、Hybrid Reverb、CC Control、Simpler）、Simpler のスライスとワープ、Wavetable のモジュレーション量。 |
| `live_device_io_preview/apply` | edit | デバイス自身の入力・出力ルーティング、またはコンプレッサーのサイドチェインのソース。 |
| `live_chain_preview/apply` | edit | ラックのチェーンの色、ミュート、ソロ。 |
| `live_chain_mixer_preview/apply` | edit | ラックのチェーンのボリューム、パン、センド、アクティベーター。 |
| `live_rack_preview/apply` | edit | マクロの数とバリエーション。マクロの追加、削除、ランダム化。チェーンの挿入。パッドのコピー。 |
| `live_rack_view_preview/apply` | edit | ラックが表示するチェーンやパッド。 |
| `live_drum_pad_preview/apply` | edit | パッドのノートとソロ、パッドのクリア、パッドへのサンプルの読み込み（一つ、またはラック全体）を Simpler または Drum Sampler として。 |
| `live_looper_preview/apply` | edit | Looper の操作と設定。 |

### ミキシング、ルーティング、バッチ

| ツール | クラス | 内容 |
| --- | --- | --- |
| `live_mixer_preview/apply` | edit | ボリューム、パン、ミュート、ソロ、キュー、センド。 |
| `live_mixer_extended_preview/apply` | edit | トラックのアクティベーター、クロスフェーダーとその割り当て、パンのモード、スプリットステレオ。 |
| `live_routing_preview/apply` | edit | 入力と出力のルーティング、アーム、モニタリング。フィードバックを起こすルーティングは拒否されます。 |
| `live_batch_preview/apply` | edit | ミキサー、パラメータ、クリップ、名前の変更、新規トラック、アームの操作を最大 32 個、一つの変更・一つの取り消しとしてまとめます。新規トラックの名前は、既存の名前と重複できません。 |

### テンポ、ソング設定、チューニング、グルーヴ

| ツール | クラス | 内容 |
| --- | --- | --- |
| `live_tempo_preview/apply` | edit | テンポ、20–999 BPM。 |
| `live_song_settings_preview/apply` | edit | 拍子、スウィング、起動と録音のクオンタイズ、起動時の選択（select on launch）。 |
| `live_tuning_preview/apply` | edit | チューニングシステムとスケール。 |
| `live_groove_preview/apply` | edit | グローバルのグルーヴ量と、プールにあるグルーヴ。 |

### 再生

これらは音が出ます。ほとんどは取り消せず、戻すには停止します。

| ツール | クラス | 内容 |
| --- | --- | --- |
| `live_transport_preview/apply` | performance | ソングの位置、ループ、パンチ、メトロノーム（取り消し可能）。 |
| `live_transport_action_preview/apply` | performance | 開始、続きから再生、停止、選択範囲の再生、すべてのクリップの停止、アレンジメントに戻る、スクラブ、タップテンポ、ナッジ、ジャンプ、セッション録音のトリガー。 |
| `live_clip_launch_preview/apply/stop` | performance | Set が再生中でもそうでなくても、クリップを一つ起動し、そのクリップをまた止めます。 |
| `live_scene_fire_preview/apply` | performance | シーンを起動します。 |
| `live_fire_button_preview/apply` | performance | クリップ、スロット、シーンの起動ボタンを、コントローラーのように押したり離したりします。 |
| `live_session_audition_preview/apply/stop` | performance | 保護付きのシーンのオーディション：Set の名前、出力の安全性の根拠、そして停止中で、アームもインプットのモニタリングもしていない Set が必要です。 |
| `live_session_emergency_stop` | performance | 直前に確認したセッションクリップ、トランスポート、録音を止めます。トランザクションは不要で、再起動後にも使えます。 |
| `live_browser_preview` | performance | Live でクリックしたときと同じように、ブラウザ項目のプレビューを再生します。 |
| `live_browser_preview_stop` | performance | そのプレビューを止めます。 |

### 録音とキャプチャ

| ツール | クラス | 内容 |
| --- | --- | --- |
| `live_recording_preview/apply` | recording | セッションまたはアレンジメントの録音を開始・停止します。録音先はアームされている必要があります。Live はほかのアームされたトラックにも録音します。 |
| `live_audio_capture_preview/apply` | capture | クリップの 1–9 秒を Resampling で録音し、解析してから、録音を削除します。実際の Live でのみ動きます。[オーディオインテリジェンス](AUDIO_INTELLIGENCE.md)を参照。 |
| `live_audio_capture_status` | read | キャプチャがライフサイクルのどの段階にあるか。 |
| `live_audio_capture_emergency_stop` | capture | 失敗や再起動のあとで、キャプチャを止めて後片付けします。 |
| `audio_diagnose_live_context` | read | 送った PCM の測定結果を、一つのトラックの現在のデバイスとミキサーに結び付けます。 |

### ビューと Live のインターフェース

| ツール | クラス | 内容 |
| --- | --- | --- |
| `live_view_preview/apply` | performance | セッションまたはアレンジメントの表示。アレンジメントのズーム、スクロール、フォロー。 |
| `live_track_view_preview/apply` | performance | トラックの折りたたみ、デバイスの挿入モード、表示するラックのチェーン、インストゥルメントの選択。 |
| `live_selection_preview/apply` | performance | トラック、シーン、スロット、クリップ、デバイス、パラメータ、チェーンの選択。ドローモード。 |
| `live_clip_view_preview/apply` | performance | クリップのグリッド、エンベロープ、ループの表示。 |
| `live_device_view_preview/apply` | performance | デバイスを折りたたむ、または展開します。 |
| `live_application_dialog_preview/apply` | edit | Live で開いているダイアログを読み、そのボタンの一つを押します。 |
| `live_message` | performance | Live のステータスバーに、または `modal: true` でダイアログに、メッセージを表示します。 |

### 取り消しと記録の管理

| ツール | クラス | 内容 |
| --- | --- | --- |
| `live_undo` | edit | 適用した変更を一つ取り消します（`confirmation: "undo"`）。 |
| `live_change` | edit | プレビューとその適用を一度の呼び出しで行います：`tool`（`*_preview`）、`args`、省略可能な `idempotencyKey`。オーディション、クリップの起動、起動ボタン、キャプチャ、録音、リアルタイムのアーム、ダイアログは拒否します。 |
| `live_undo_step_begin/end` | edit | その間の変更を、Live 自身の取り消しの一ステップにまとめます。 |
| `live_song_undo/redo` | edit | Live 自身の取り消しとやり直しを一回（`undo-in-live`、`redo-in-live`）。削除や、Live で行った編集に使います。 |
| `live_transaction_release` | edit | 取り消すつもりのない適用済みの変更について、最大 64 件の取り消しを手放します。 |
| `live_recovery_finalize` | edit | Live を手で確認したあとで、不確定な変更の記録を閉じます。[回復手順](RECOVERY.md)を参照。 |

### リアルタイムコントロール

パラメータをすばやく動かすための、短時間だけ使う UDP チャネルです。[リアルタイムコントロール](REALTIME_CONTROL.md)を参照してください。

| ツール | クラス | 内容 |
| --- | --- | --- |
| `live_realtime_arm_preview/apply` | realtime | 指定したパラメータ用にチャネルを開き、そのトークンを返します。 |
| `live_realtime_disarm` | realtime | チャネルを閉じます。 |
| `live_realtime_stats` | realtime | 受け付けた、適用した、破棄したパケットの数。 |

### Python と保存テキスト

| ツール | クラス | 内容 |
| --- | --- | --- |
| `live_run_python` | python | Live のメインスレッドで Python を実行します（`code`、`mode` は `exec` または `eval`、省略可能な `ref`、`timeoutMs` は最大 30,000）。プレビューも `live_undo` もありません。Live の取り消しでは一ステップになります。[Live の安全性](LIVE_SAFETY.md)を参照。 |
| `live_data_preview/apply` | edit | Set またはトラックに、`kumi.` で始まるキーでテキストを保存します。 |

### Live 拡張機能のツール

ブリッジが Kumi の Live 拡張機能に接続している間、一覧に表示されます（[Kumi の Live 拡張機能](#kumi-の-live-拡張機能)を参照）。

| ツール | クラス | 内容 |
| --- | --- | --- |
| `live_render_offline` | read | オーディオトラックのクリップを、二つの拍の間でファイルにレンダリングします。デバイスを通る前の音で、再生はしません。 |
| `live_project_import` | filesystem | オーディオファイルを Set のプロジェクトフォルダにコピーします。 |
| `live_arrangement_midi_clip_preview/apply` | edit | ノート付きの MIDI クリップを一つ以上、アレンジメントに書き込みます。 |
| `live_clip_clear_range_preview/apply` | edit | トラックのアレンジメントの一区間を消去し、その両端でクリップを切ります。削除と同じく kept です。 |
| `live_device_duplicate_preview/apply` | edit | デバイスを設定ごと、そのすぐ後ろにコピーします。 |

### Willington のツール

Willington のバインディングがオン（`/willington`）で、それが対応する Live ビルドのときだけ表示されます。[Willington](WILLINGTON_INTEGRATION.md)を参照してください。

| ツール | クラス | 内容 |
| --- | --- | --- |
| `live_willington_device_preview/apply` | edit | ラックのマクロ名とバリエーション名、マクロのマッピング、チェーンのゾーン。 |
| `live_follow_actions_preview/apply` | edit | 再生を止めた状態での、セッションクリップの Follow Actions。 |

## イベント

`2025-11-25` では、`live_subscribe`（必要なら `types` の一覧を付けて）を呼ぶと、Live が変わるたびにサーバーが `notifications/live_event` を送ります。

| 種類 | タイミング |
| --- | --- |
| `transport` | 再生または録音が開始・停止したとき |
| `object` | トラックやシーンの一覧が変わったとき |
| `selection` | 選択が変わったとき |
| `name` | トラック、シーン、クリップの名前や色が変わったとき |
| `mixer` | トラックのミュート、ソロ、アーム、ボリューム、パン、センドが変わったとき |
| `parameter` | 選択中のデバイスのパラメータが変わったとき |
| `structure` | トラック、シーン、ロケーター、またはトラックのデバイスやクリップが変わったとき |
| `reset` | Live を読み直してください。手元の情報が古くなっているかもしれません |

各イベントには `epoch`、`sequence`、`type`、`channel`（`remote-script` または `extension`。それぞれ独立して番号が振られます）、`payload` があります。`pointed` イベントは、拡張機能の「Ask Kumi about this」から、購読なしで届きます。たまったイベントが 65,536 を超えると、サーバーは残りを捨て、`resnapshot: true` 付きの `notifications/live_event_overflow` を送ります。そのあとや、`reset` を受け取ったとき、`sequence` に抜けがあったときは、Set を読み直してください。

`2026-07-28` では、`live_observe_subscribe` と `live_observe_poll` を使ってください。

## リソースとプロンプト

| リソース | 内容 |
| --- | --- |
| `ableton://capabilities` | ネゴシエートされた機能と、どのツールが利用可能か、表示されているか、ポリシーで拒否されているか、そのクラス（JSON） |
| `ableton://safety` | 安全性についての短い要約（Markdown） |
| `ableton://journeys` | 五つのガイド付きジャーニーと、この Live がそれぞれのどこまでに対応しているか（JSON） |
| `ableton://live-workflow` | 安全なテンポ変更の手順（Markdown） |
| `ableton://max-extension` | オペレーターが作る Max パッチがリアルタイムコントロールに使えるパケット形式。Max デバイスは同梱していません（JSON） |

| プロンプト | 引数 |
| --- | --- |
| `analyze_audio` | `sampleRate`、省略可能な `channels` |
| `change_tempo_safely` | なし |
| `create_beat_or_song`、`sequence_advanced_drums`、`design_owned_sound`、`compare_reference_mix`、`diagnose_performance_setup` | `traits`、省略可能な `experienceLevel`（`beginner` または `advanced`）と `bars`（`"1"` から `"16"` までの文字列） |

プロンプトとリソースは説明するだけで、何も許可しません。ジャーニーのプロンプトは[作業例](USER_JOURNEYS.md)で説明しています。

## 環境変数

| 変数 | 効果 |
| --- | --- |
| `ABLETON_MCP_TOOL_POLICY`、`ABLETON_MCP_TOOL_ALLOW`、`ABLETON_MCP_TOOL_DENY` | [デプロイメントポリシー](#デプロイメントポリシー) |
| `ABLETON_MCP_EXTENSION`、`ABLETON_MCP_EXTENSION_DIR`、`ABLETON_MCP_LIVE_EXTENSIONS_DIR` | [Kumi の Live 拡張機能](#kumi-の-live-拡張機能) |
| `ABLETON_MCP_IMPORT_STAGING_DIR` | 取り込むファイルを Live 用にコピーする場所（絶対パス。デフォルトは `~/.config/ableton-mcp/import-staging`、Windows では `%APPDATA%\ableton-mcp\import-staging`） |
| `ABLETON_MCP_USER_LIBRARY` | Drum Sampler に読み込むサンプルに使う、Live の User Library（ブリッジはその `Kumi` フォルダにキャリアプリセットを書き込みます） |
| `ABLETON_MCP_LIVE_RESOURCES` | デフォルトの Drum Sampler プリセットがある、Live の Resources フォルダ |
