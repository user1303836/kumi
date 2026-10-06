# 実装状況

[English](../en/IMPLEMENTATION_STATUS.md) · [简体中文](../zh-CN/IMPLEMENTATION_STATUS.md) · 日本語

現在の状況をまとめます：現在のバージョン、本物の Live で何をテストしたかとその記録の場所、既知の制限です。Live の各領域で何に対応しているかは[機能マトリクス](CAPABILITY_MATRIX.md)に、システムと Live のバージョンは[対応プラットフォーム](SUPPORT_MATRIX.md)にあります。

## バージョン

[変更履歴](../../CHANGELOG.md)の各リリースには、同梱されるブリッジのバージョンが記載されています。すべてのブリッジのバージョンは[ブリッジのバージョン](KUMI_CHANGES.md#ブリッジのバージョン)に一覧があります。バージョン番号そのものは、クレートの `Cargo.toml` ファイル（Kumi、ブリッジ）、ルートの `package.json`（Kumi）、`apps/live-extension/manifest.json`（拡張機能）にあります。

| 部分 | バージョン |
| --- | --- |
| Live プロトコル | `ableton-live/v1`。レジストリのハッシュは各リリースの `release-manifest.json` に記録されます |
| MCP プロトコルの世代 | `2025-11-25` と `2026-07-28` |
| ランタイム | アプリと単体のブリッジはネイティブ Rust。新規インストールに Node は不要 |
| 以前のインストール（Kumi 1.7.5 以前） | Node 22/24。残された Node はロールバックと任意の YouTube チャレンジ処理にも使用（[詳細](SUPPORT_MATRIX.md)） |

## どこで何をテストしたか

以下の本物の Live の記録は、過去の TypeScript リリースのものです。ネイティブ版の CI と移行テストは、新たな実機での受け入れテストの代わりにはなりません。

- **すべてのプルリクエスト：** ネイティブ Rust のビルドとテスト、Remote Script と Live 拡張機能のテスト、6 ターゲットのバンドル、インストールと移行のテスト。[CI](TESTING.md#ci) を参照してください。
- **macOS 上の本物の Live**（Apple silicon、Live 12.4.15 beta）：Kumi が行うあらゆる種類の変更と、そのそれぞれを Kumi で取り消すこと（ブリッジ 1.0.62 と 1.0.63 で 65 件中 65 件、19 トラックと 200 トラックの Set で）、再生、バウンス、聴き取り、監視、Kumi の拡張機能によるオフラインレンダリングと右クリックメニュー、そして Willington による編集。
- **Windows 上の本物の Live**（Windows 10、Live 12.4.15 beta、Kumi 1.6.0 とブリッジ 1.0.71）：移動した User Library へのブリッジのインストール、Remote Script の読み込み、Kumi の接続。これについての記録ファイルはまだありません。

## エビデンス

記録は [`docs/evidence`](../evidence/) にあります。記録にブリッジのバージョンが書かれていない場合は、その記録を追加したコミットの時点のバージョンを記載しています。

| 記録 | 日付 | Live | ブリッジ | 内容 |
| --- | --- | --- | --- | --- |
| [kumi-poc.md](../evidence/kumi-poc.md) | 2026-09-28〜09-30 | 12.4.15b4、b5 | 1.0.0〜1.0.63 | 本物の Live 上の Kumi：受け入れテスト、速度と大きな Set、`kumi bridge`、試聴とゴール、Max for Live デバイス、再接続 |
| [live-extension.md](../evidence/live-extension.md) | 2026-09-30 | 12.4.15b5 | 1.0.57〜1.0.65 | Live が Kumi の拡張機能をどう実行するか。オフラインレンダリングとそのコスト、両チャンネルの結果の一致、取り消し、右クリック |
| [lom-audit.md](../evidence/lom-audit.md)、[JSON](../evidence/lom-audit-12.4.15b5.json) | 2026-09-30 | 12.4.15b5 | 1.0.55 | Live の Python API の全数調査と、Remote Script が使っているものとの照合 |
| [kumi-benchmark.md](../evidence/kumi-benchmark.md) | 2026-09-30 | 12.4 beta | 1.0.52 | 曲の一部分を耳で再現する：実行とスコア |
| [kumi-clip-follow-actions-b5.json](../evidence/kumi-clip-follow-actions-b5.json) | 2026-09-30 | 12.4.15b5 | 1.0.53 | Willington を通じた Follow Actions と Legato。読み戻しと取り消し |
| [willington-kumi-chat.json](../evidence/willington-kumi-chat.json) | 2026-09-30 | 12.4.15b4 | 1.0.52 | Willington による編集を使った Kumi との会話。それぞれ取り消し済み |
| [rack-zones-b5.json](../evidence/rack-zones-b5.json) | 2026-10-01、2026-10-02 | 12.4.15b5 | 1.0.66 | Willington を通じたラックのチェーンゾーン：読み取り、書き込み、取り消し、保存して開き直し。その後、信号のゲート、フェード、Max の `live.object` での書き込み・読み戻し・復元 |
| [capability-manifest.json](../evidence/capability-manifest.json) | Kumi 1.7.6 時点（2026-10-04） | — | 1.0.74 | レジストリのすべての操作（実行可能なものと予約済みのもの）と、レジストリのハッシュ。今後は再生成されません |
| `phase-3`〜`phase-9` のファイル | 2026-07-26〜07-28 | 12.4.5b8 | 0.1.0 | Kumi 以前の、ブリッジの最初の本物の Live での実行：ディスカバリー、試聴、トランスポート、クリップ、アレンジメント、ミキサー、オートメーション、デバイス、Browser、ルーティング、録音、プロジェクトファイル、イベント、リアルタイムとキャプチャ。加えて [FFmpeg のラウドネスオラクル](../evidence/phase-8-audio-oracle.json)と、偽の Live に対するパッケージ版でのジャーニー。過去の記録です：ブリッジはその後大きく変わっています。 |

## 既知の制限

- Live の API では、Set の保存や書き出し、フリーズ、グループトラックの作成、アレンジメントのオートメーションの編集はできません。[Live の API が提供しないもの](CAPABILITY_MATRIX.md#live-の-api-が提供しないもの)を参照してください。
- 一部の変更は Kumi では取り消せず、Live 自身の取り消しでしか戻せません。[Live の安全性](LIVE_SAFETY.md)を参照してください。
- Windows では、Kumi の拡張機能は未テストで、1.6.0 以前からの `kumi update` は tar のエラーで失敗することがあります。[Windows](SUPPORT_MATRIX.md#windows) を参照してください。
- これまでの本物の Live でのテストは Live 12.4.15 beta で、ほとんどが Apple silicon の Mac です。Live 12.0〜12.3、Intel Mac、スクリーンリーダーは未テストです。
- Willington による編集は `/willington` でオンにするまではオフで、Willington のバインディングがある Live ビルドが必要です：macOS ARM64 の Live 12.4.15b4 と b5（ラックのチェーンのゾーンは b5 のみ）と、Windows x64 の Live 12.4.15b5。Windows では、バインディングは Live の中の Python を通じてテストされていますが、Kumi 自身の編集を通じたテストはまだです。[Willington](WILLINGTON_INTEGRATION.md)を参照してください。
- Kumi とブリッジには署名がありません。[リリースと配布](DISTRIBUTION_POLICY.md)を参照してください。

## 残っている作業

- Renoise と Reaper への対応。
- Windows 版の Live が Extensions フォルダーを置く場所の確認。
- 生成される機能マニフェストは、Live の Browser がプレビューできるときにはブリッジが提供しているにもかかわらず、Browser のプレビューをまだ予約済みとしています。
- [未決のオーナー判断](DISTRIBUTION_POLICY.md#未決のオーナー判断)に挙げた判断。
