# リアルタイム制御

[English](../en/REALTIME_CONTROL.md) · [简体中文](../zh-CN/REALTIME_CONTROL.md) · 日本語

コントローラースクリプト、OSC アプリ、Max パッチから、一度に最大 30 秒間、パラメータを高速に制御するための、Remote Script 内の UDP エンドポイントです。MCP クライアント向けのもので、Kumi は使いません。

ポートが開いているだけでは何も許可されません。クライアントがブリッジを通じてプレーンをアームするまでパケットは捨てられ、アームでは、パケットが動かしてよいパラメータを正確に指定します。

## 設定

Remote Script は、バージョン 2 の設定の `bridge` セクションに、`port` とは異なる `realtimePort` があるときに UDP で待ち受けます。

```json
"bridge": {"host": "127.0.0.1", "port": 9765, "realtimePort": 9766, "secretFile": "/absolute/path/to/bridge.secret", "timeoutMs": 5000}
```

ファイル全体については、Remote Script の README（[daws/ableton/remote-script/README.md](../../daws/ableton/remote-script/README.md)）で説明しています。

- `ableton-mcp-lifecycle install`、つまり `kumi bridge` は、`--realtime-port` で別のポートを指定しない限り、`realtimePort` を 9766 に設定します。インストールは、まず両方のポートが空いていることを確認します。
- `ableton-mcp-setup` がこれを書くのは、ほかのブリッジのオプションと一緒に `--realtime-port` を渡したときだけです（[ユーザーガイド](USER_GUIDE.md)）。
- `realtimePort` がなければ UDP ソケットはなく、`realtime.*` の操作も提供されません。

どちらのソケットも、設定されたループバックアドレスにだけバインドします。Live の起動時に UDP ポートが使われていると、Remote Script は TCP も含めてまったく読み込まれません。ポートを空けるか、別のポートで再インストールしてください。

## アーム

1. `live_realtime_arm_preview` を次の引数で呼び出します。
   - `channels`：`udp-json`、`osc`、`xy`、`max` から 1〜4 個
   - `parameterRefs`：最新のディスカバリーで得たパラメータ参照を 0〜32 個（空のリストでは緊急停止パケットだけが許可されます）
   - `ttlMs`（省略可）：1,000〜30,000、デフォルト 10,000
   - `sourcePorts`（省略可）：パケットの送信元として許可する UDP ポートを最大 16 個
   - `outputSafety`（省略可）

   `realtime.arm`、`realtime.disarm`、`realtime.stats` を提供する、接続済みの `real-live` の Remote Script が必要です。プレビューは各パラメータの同一性を、そのデバイス、トラック、デバイスのほかのパラメータとあわせて記録します。
2. `live_realtime_arm_apply` を、トランザクション ID、`confirmation: "apply"`、冪等キーで呼び出します。Remote Script は Live のスレッドで、記録したすべての同一性を今の Live と比べ、パラメータが置き換えられたり移動したりしていれば拒否します。
3. 結果には、エンドポイント（`host`、`port`）、ベアラートークン `token`、`expiresAt`、チャンネルとパラメータ参照、制限が含まれます。トークンはログやファイルに残さないでください。
4. パケットを送り（下記）、`live_realtime_stats` でそれらがどうなったかを確認します。
5. 終わったら、`live_realtime_disarm` を `confirmation: "disarm"` で呼び出します。

もう一度アームすると、トークンが置き換わり、シーケンスが最初からになり、前のアームからキューに残っている書き込みは捨てられます。ディスアーム、期限切れ、Remote Script の終了でも捨てられます。

## パケット

### UDP JSON

データグラム 1 つにつき JSON オブジェクト 1 つで、最大 512 バイトです。未知のフィールドは拒否されます。

パラメータを 1 つ設定する：

```json
{"token":"<arm token>","seq":1,"channel":"udp-json","op":"parameter.set","ref":"<parameter ref>","value":0.5,"sentAtMs":1700000000000}
```

2 つのパラメータを同時に設定する（どちらかの書き込みが失敗すると、両方とも元に戻ります）：

```json
{"token":"<arm token>","seq":2,"channel":"xy","op":"xy.set","xRef":"<parameter ref>","x":0.4,"yRef":"<parameter ref>","y":0.6,"sentAtMs":1700000000000}
```

緊急停止：

```json
{"token":"<arm token>","seq":3,"channel":"udp-json","op":"emergency-stop","sentAtMs":1700000000000}
```

- `seq` は 2^53 − 1 までの正の整数で、1 つのアームの中でパケットごとに大きくなります。小さい `seq` や繰り返した `seq` はリプレイとみなされ、捨てられます。
- `sentAtMs`（省略可、1970 年からのミリ秒）があると、統計は転送のジッターを測ります。ないときは到着間隔のジッターを測ります。
- `channel` はアームしたチャンネルのどれかで、しかも操作に合ったものでなければなりません。

| 操作 | チャンネル |
| --- | --- |
| `parameter.set` | `udp-json`、`osc`、`max` |
| `xy.set` | `xy`、`osc`、`max` |
| `emergency-stop` | アームしたどのチャンネルでも |

### OSC

OSC バンドルと、対応していない引数の型は拒否されます。

| アドレス | 引数 |
| --- | --- |
| `/ableton-mcp/parameter` | token（文字列）、seq（int32 または int64）、ref（文字列）、value（数値）、省略可能な sentAtMs（数値） |
| `/ableton-mcp/xy` | token、seq、xRef、x、yRef、y、省略可能な sentAtMs |
| `/ableton-mcp/emergency-stop` | token、seq、省略可能な sentAtMs |

数値は int32、int64、float32、float64 のどれでもかまいません。

### Max

Max パッチは、`"channel": "max"` を付けた UDP JSON オブジェクトを、`udpsend` で返されたエンドポイントに送れます。ブリッジに Max デバイスは付属せず、Max 用のハンドシェイクもありません。パケットの取り決めは、ブリッジの `ableton://max-extension` リソースに書かれています。

### 緊急停止

`emergency-stop` パケットは、その時点で鳴っているものを Live のスレッドで止めます。Session のクリップ、トランスポート、Session Record、Arrangement Record です。`live_session_emergency_stop` は、認証された TCP チャンネルで、トークンなしに同じことをします。

## 制限

| 項目 | 制限 |
| --- | --- |
| パケットサイズ | 512 バイト |
| レート | 持続して毎秒 64 パケット、バースト 16（トークンバケット） |
| アームあたりのパラメータ数 | 32 |
| アームあたりの送信元ポート数 | 16 |
| アームの有効期間 | 1〜30 秒 |
| Live のスレッドを待つ時間 | 1 秒。それまでに始まらなかった書き込みは捨てられます |
| キュー | Remote Script のメインスレッドのキュー（65,536 エントリ）と共有 |

## パケットがたどる流れ

Live にまったく触れない UDP スレッドで、Remote Script はパケットをデコードし、トークン、送信元、チャンネル、対象のパラメータ、シーケンス、レートを確認してから、書き込みを Live のメインスレッド用のキューに入れます。レートのために捨てられたパケットも、その `seq` は使用済みです。同じ番号をもう一度送るのではなく、次の番号を送ってください。

Live のメインスレッドでは、次の表示のティックで、アームがまだ有効であること、各パラメータがアーム時に記録した同一性のままであることを確認します。何かが変わっていれば、アームは取り消されます。次に値を確認します。パラメータの範囲内にあり、段のどれかに乗っていて、パラメータが有効であることです。通らない値は拒否され、合うように動かされることはありません（型付きのパラメータツールとは違います）。そのあと値を書き込み、読み戻します。

`accepted` は Live のスレッド用のキューに入ったという意味で、書き込まれたという意味ではありません。UDP 自体は配送を確認しません。書き込まれて読み戻されたことを意味するのは `applied` だけです。

`live_realtime_stats` が報告するもの：

| フィールド | 意味 |
| --- | --- |
| `armed` | アームが有効かどうか |
| `accepted`、`applied`、`pending` | キューに入ったもの、書き込まれて確認されたもの、まだ待っているもの |
| `applyFailures`、`revokedBeforeApply` | Live のスレッドで失敗した書き込みと、アームが終わったか対象が変わったために捨てられた書き込み |
| `droppedBeforeDispatch`、`droppedQueueFull` | Live のスレッドが 1 秒以内に始めなかった書き込みと、キューに入れられなかった書き込み |
| `droppedUnarmed`、`droppedEndpoint`、`droppedTarget`、`droppedInvalid`、`droppedReplay`、`droppedRateLimited` | 捨てられたパケット：アームがないかトークンやチャンネルが違う、送信元が違う、アームしていないパラメータ宛て、形式が不正、リプレイ、レート超過 |
| `sequenceGaps`、`lastSequence` | これまでに抜けたシーケンス番号と、最後に受け付けた番号 |
| `jitterMs`、`maxJitterMs` | 平滑化したジッターと、最大のジッター |

## 復旧

- `applyFailures`、`revokedBeforeApply`、`droppedBeforeDispatch` のどれかが出たとき、または `pending` がゼロまで下がらないときは、ディスアームし、ディスカバリーをやり直してから新たにアームしてください。
- 捨てられたパケットは数えられるだけで、代わりに再送されることはありません。
- Live が動いたまま MCP ホストが再起動した場合、トークンは期限が切れるまで使えます。再接続しても期限は延びません。
- Live または Remote Script が再起動すると、ソケットが閉じ、すべてのトークンが無効になります。
- `live_recovery_finalize` は、アームが有効なあいだや書き込みが保留中のあいだは拒否します。
- セッションのあとは、動かしたパラメータを元に戻し、Live が止まっていて録音していないことを確認してください。

## 証拠

[phase-7c-realtime-live.json](../evidence/phase-7c-realtime-live.json) では、macOS 上の Live 12.4.5b8（2026-07-27、ブリッジ 0.1.0）で、4 つのチャンネルすべて、リプレイ、送信元と対象による破棄、パラメータの復元を試しました。これは現在のブリッジより前のもので、Windows での実行は記録されていません。Max デバイスはテストしていません。
