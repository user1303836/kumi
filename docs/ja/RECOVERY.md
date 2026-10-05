# 回復手順

[English](../en/RECOVERY.md) · [简体中文](../zh-CN/RECOVERY.md) · 日本語

呼び出しが失敗したときや、変更が起きたかどうかわからなくなったときの対処法です。

原則：**不確定な変更に、新しい変更で応じないでください。** 適用がタイムアウトしたり、応答が失われたりした場合は、サーバーと Live が動いている間に、同じ `transactionId` と `idempotencyKey` でその適用だけを再試行します。それ以外の場合は、Live を読み直して手で直してください。

## エラーの読み方

不正なリクエストには JSON-RPC エラーが返ります。

| コード | 意味 |
| --- | --- |
| `-32700` | 行が JSON ではない |
| `-32600` | 不正なリクエスト、500 MiB を超えるメッセージ、すでに使われている `id`、またはサーバーが終了処理中 |
| `-32601` | そのようなツールはない |
| `-32602` | 不正な引数、またはプロトコルバージョンの誤り |
| `-32002` | まだ初期化されていない、またはそのようなリソースやプロンプトはない（`2025-11-25`） |
| `-32022` | 対応していないプロトコルバージョン（エラーに対応バージョンが示されます） |
| `-32000` | ビジー：待機中のリクエストが多すぎます。いくつか終わってから再試行してください |
| `-32603` | 内部エラー |

拒否したツールは、`isError: true` と `{"reason": "...", "remediation": "..."}` を返します。理由はブリッジまたは Live 自身のもので、1 行です。次のように読みます。

- **"Nothing changed in Live"**、または "; nothing changed" で終わる理由：呼び出しは何もしていません。書かれていることを直してから、プレビューし直してください。
- **"Live state changed since the preview"**：プレビューと適用の間に何かが変わりました。Live での自分の編集か、別のクライアントによるものかもしれません。読み直してから、プレビューし直してください。
- **`tool-unavailable-in-current-live-shape`**：この Live は、いまはそのツールを提供していません。理由は `ableton://capabilities` で確認してください。
- **`tool-denied-by-deployment-policy`**：[デプロイメントポリシー](USER_GUIDE.md#デプロイメントポリシー)がそのツールを隠しています。
- **remediation に変更が不確定だと書かれている**：次の節を参照してください。

## 変更が不確定なとき

適用や取り消しの途中でタイムアウト、切断、応答の消失が起きると、変更は不確定になります。起きたかもしれないし、起きていないかもしれません。

1. プレビューし直さないでください。新しいキーも送らないでください。
2. 同じサーバーが動いていて、Live が再起動していない間は、同じ `transactionId` と `idempotencyKey` で、同じ適用（または取り消し）をもう一度送ります。Remote Script は実行したことを覚えています。最初の結果を返すか、変更を最後まで行い、ブリッジがそれを読み戻します。
3. その間に Live が再起動していた場合、再試行は拒否されます。Set を読み（`live_discover`、`live_snapshot`）、変更が反映されているかを確かめて、手で正しい状態にしてください。
4. そのあと、`live_recovery_finalize` で記録を閉じます。

   ```json
   {"transactionId": "<id>", "resolution": "manually-restored", "confirmation": "finalize-recovery-record",
    "evidence": {"provenance": "checked the mixer in Live", "scope": "track 3 volume"}}
   ```

   Live を今のまま残す場合は、`"accepted-current-state"` を使います。finalize しても、Live は何も変わりません。何かが再生中、録音中、またはリアルタイムチャネルを保持している間は拒否されます。

不確定な記録は、サーバーの取り消しの容量を消費します。容量がいっぱいになると、finalize するまで新しい変更は拒否されます（"capacity is exhausted by recovery-protected work"）。

## よくある問題

| 問題 | 対処 |
| --- | --- |
| Kumi 1.7.5 以前がインストールした JavaScript 版のブリッジが "Unsupported Node.js" で終了する | Node 22 または 24 で実行してください。ネイティブ版のブリッジは Node を使いません。 |
| "version-1 configuration does not enable a Live adapter" | `ableton-mcp-setup` とブリッジのオプションで、バージョン 2 のファイルを書き出してください。[設定ファイル](USER_GUIDE.md#設定ファイル)を参照。 |
| "secret file is invalid"、またはアクセス権が "must be conclusively owner-only" | シークレットは 32 文字以上の 1 行で、自分だけが読めるようにする必要があります。ライフサイクルでインストールした場合は、`ableton-mcp-lifecycle repair` で元に戻せます。 |
| `live_status` が `"connected": false` を返す | Live が動いていて **AbletonMcpBridge** が Control Surface として選ばれていること、そして設定のポートとシークレットが、Remote Script の使っているものと同じであることを確認してください。そのあと `ableton-mcp-diagnostics --config <path>` を実行します。 |
| Live で AbletonMcpBridge が読み込まれず、Live のログに "bridge configuration reference is missing or unsafe" と出る | Remote Script が `--config` なしでインストールされています。`--config` を付けてインストールし直すか（[Live に接続する](USER_GUIDE.md#live-に接続する)を参照）、ライフサイクルを使ってください。 |
| "Unknown or expired … transaction" | プレビューの期限が切れたか、サーバーが再起動しました。プレビューし直してください。 |
| `live_undo` が拒否する：ref が "isn't the one this change was made on any more" | オブジェクトが置き換えられています。手で直すか、Live の最後の取り消しステップがその変更であれば `live_song_undo` を使ってください。 |
| 誤って何かを削除した | すぐに `live_song_undo`（`confirmation: "undo-in-live"`）を使ってください。`live_undo` では削除を戻せません。 |
| Live 拡張機能のツールが見当たらない | 拡張機能が動いていません。Live 12.4 以降が必要で、さらに Live の Extensions フォルダへのインストールか、Live の Developer Mode のどちらかが必要です。[Kumi の Live 拡張機能](USER_GUIDE.md#kumi-の-live-拡張機能)を参照。 |

インストールの問題（ライフサイクルのレシート、隔離、修復、ロールバック）は、[ブリッジのインストール](DELIVERY.md)で扱っています。

## 再生や録音をすべて止める

`live_session_emergency_stop` は、セッションクリップ、トランスポート、両方の録音モードを止めます。トランザクションは不要なので、再起動後にも使えます。

1. 再生中のものを読みます：`{"kind": "session-playback"}` を指定した `live_discover`。
2. 読んだとおりの内容を送ります。

   ```json
   {"confirmation": "emergency-stop", "expectedTargets": ["<trackRef>|<clipSlotRef>|<sceneRef>"],
    "expectedRecording": "session"}
   ```

   `expectedRecording` は `stopped`、`session`、`arrangement`、`both` のいずれかです。

3. その間に再生状態が変わったために拒否された場合は、読み直して繰り返します。成功すると `"stopped": true` と `recordingStopped` が返ります。

自分で起動したクリップを一つ止めるには `live_clip_launch_stop` を、オーディションには `live_session_audition_stop` を使います。キャプチャには専用の緊急停止があります。[オーディオインテリジェンス](AUDIO_INTELLIGENCE.md)を参照してください。リアルタイムコントロールでは `live_realtime_disarm` を呼び、[リアルタイムコントロール](REALTIME_CONTROL.md)を参照してください。

## 再起動のあと

サーバーは取り消しの記録とプレビューをメモリにだけ保持するので、再起動すると失われます。Live の再起動や再接続で Live のエポックが新しくなり、それ以前の ref はすべて使えなくなります。

1. サーバーを起動し、もう一度初期化します。
2. `live_status` を呼び、Set を読み直します。
3. 再起動前の変更は、`live_undo` では取り消せません。Live 自身の取り消し（`live_song_undo`）には、まだ残っているかもしれません。
4. サーバーが止まったときに不確定だった変更があれば、手で確認してください。再試行や finalize に使える記録は残っていません。
