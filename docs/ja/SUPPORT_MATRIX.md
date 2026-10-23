# 対応プラットフォーム

[English](../en/SUPPORT_MATRIX.md) · [简体中文](../zh-CN/SUPPORT_MATRIX.md) · 日本語

Kumi と各ブリッジが動作する環境、対応する Live、REAPER、Renoise、Logic Pro のバージョン、どこで何をテストしたかをまとめます。それぞれの「テスト済み」の根拠は[実装状況](IMPLEMENTATION_STATUS.md)に記載しています。

## Kumi

| システム | バージョン | プロセッサ | 状況 |
| --- | --- | --- | --- |
| macOS | 13（Ventura）以降 | Apple silicon、Intel。Logic Pro：Apple silicon のみ | ネイティブのビルドとインストールは CI 対象。過去の TypeScript 版を Apple silicon 上の Live でテスト済み。Logic Pro は Apple silicon でテスト済み |
| Windows | 10 または 11 | x64、ARM64 | ネイティブのビルドとインストールは CI 対象。過去の TypeScript 版を Windows 10 上の Live でテスト済み。[Windows](#windows) を参照 |
| Linux | glibc 2.35 以降（Ubuntu 22.04、Debian 12 以降。Alpine などの musl 系は不可） | x64、ARM64 | ネイティブのビルドとインストールは CI 対象。Linux 版の Live は存在しませんが、REAPER と Renoise は動きます |

- Kumi と単体のブリッジはネイティブ Rust プログラムです。新規インストールは Node をダウンロードせず、必要としません。
- Node 24 を使う既存の Kumi 1.7.4 と 1.7.5 は、`kumi update` で設定、サインイン、データを保ったまま移行します。残された Node はロールバックと任意の YouTube チャレンジ処理に使えます。Windows では、ネイティブ版の初回起動で古いランチャーが置き換わり、以降の起動は Node を経由しません。古い Node メジャーでは同じ `KUMI_HOME` でインストーラーの再実行が必要な場合があります。[更新方法](KUMI_GUIDE.md)を参照してください。
- YouTube チャレンジ処理には残された Node または PATH 上の Node を使い、Kumi はそのための Node を取得しません。Live 拡張機能は Live 自身の JavaScript ホストで動きます。

以下の実機 Live の結果は旧 TypeScript 版の記録です。ネイティブ版の CI と移行テストは、実機 Live での受け入れ確認とは別です。

## Ableton Live

| Live | 状況 |
| --- | --- |
| 12.4 以降 | すべての機能。Kumi の Live 拡張機能による、オフラインレンダリング、アレンジメントへの MIDI クリップの書き込み、範囲のクリア、**Ask Kumi about this** も含みます。macOS 上の Live 12.4.15 beta でテスト済み。 |
| 12.0〜12.3 | ブリッジはその Live の API にあるものを提供します。拡張機能は使えないので、上記の機能はありません。`kumi doctor` がそう伝えます。未テスト。 |
| 11 以前 | 非対応。 |

エディション：ブリッジは接続先の Live が提供するものを検出するので、エディションにないデバイスやコンテンツ（Standard と Intro は少なめです）は推測で扱われず、使えないままになります。Max for Live デバイスを作るには Max for Live（Suite、またはアドオンを追加した Standard）が必要です。Willington（そのファイルを収めたリリースからは Kumi のブリッジに入っていて、それまでは自分でインストールします。`/willington` でオンにするまではオフ）には、macOS ARM64 の Live 12.4.15b4 と b5 用（ラックのチェーンのゾーンは b5 のみ）と、Windows x64 の Live 12.4.15b5 用のバインディングがあります。[Willington](WILLINGTON_INTEGRATION.md)を参照してください。

ネイティブ編集（グループ、Arrangement オートメーション、シーン/全体の Follow Actions、ノートごとの MPE）には、インストール済みの検証済み `WillingtonEditing`、実際のライブラリに一致するセルフテストの合格、明示的な有効化が必要です。Live 12.4.15b5 macOS ARM64 のみで、b4、Intel macOS、Windows は対象外です。[セルフテスト手順](WILLINGTON_INTEGRATION.md#ネイティブ編集とセルフテスト)を参照してください。

## REAPER と Renoise

| DAW | バージョン | システム | 状況 |
| --- | --- | --- | --- |
| REAPER | 7 以降 | macOS（Apple silicon と Intel に一つの拡張機能）、Windows x64（x64 を読み込む Windows on Arm の REAPER にも同じ拡張機能）、Linux x64 と ARM64（glibc 2.35 以降） | macOS の REAPER 7.82（評価版、ポータブル）でテスト済み。CI は各システムで拡張機能をビルドし、確認し、システム自身のローダーで読み込みます。Windows と Linux の REAPER ではまだ試していません。Windows on Arm は未テスト |
| Renoise | 3.5 以降 | macOS、Windows、Linux | macOS の Renoise 3.5.4 でテスト済み。CI は Renoise の API のモックに対してツールのテストを実行し、Renoise 自身の API 定義と照らし合わせます。Windows と Linux の Renoise ではまだ試していません |

- Linux はどちらにとっても DAW のプラットフォームです。セットアップは macOS や Windows と同じく、そこでも REAPER と Renoise につなぎます。
- CI はどのシステムでも、Kumi 自身のコマンドで、両方のブリッジをスクラッチの DAW フォルダーに入れ、更新し、2 回ロールバックし、アンインストールします。
- Renoise のデモ版はレンダリングできないので、Kumi はそこでソングを聴けません。

## Logic Pro

| Logic Pro | 状況 |
| --- | --- |
| 12（Apple シリコン搭載 Mac） | Kumi の Logic Pro ブリッジを通じて対応（[Logic Pro につなぐ](KUMI_GUIDE.md#logic-pro-につなぐ)）：App Store の Logic Pro（`com.apple.logic10`）と Creator Studio のもの（`com.apple.mobilelogic`）。macOS 26.6.1 上の、App Store の英語版 Logic Pro 12.3.1 でテスト済み。 |
| 11 以前 | 非対応。`kumi --daw logic doctor` がそう伝えます。 |
| Intel Mac、Linux、Windows | Logic Pro ブリッジはありません。Logic Pro 12 は Apple シリコン搭載 Mac でしか動かないので、ブリッジを収めているのは Apple シリコン搭載 Mac の Kumi だけです。ほかの Kumi は「Logic Pro is supported on Apple Silicon Macs only」と伝え、何も用意しません。 |

ブリッジは Logic Pro の横で動く Kumi のプログラムで、Kumi 自身のフォルダーにあります。macOS のアクセシビリティ（Kumi を動かしているアプリに一度だけ許可します）、Logic Pro のキーコマンドとメニュー、仮想 MIDI ポート上の Mackie Control サーフェスを通じて Logic Pro を読み、操作し、Core Audio のプロセスタップを通じて Logic Pro を聴きます。プロセスタップには macOS 14.2 以降が必要です（macOS が求める場合は System Audio Recording も）。ほかの言語の Logic Pro はテストしていません：ブリッジは Logic Pro のラベルを Logic Pro 自身の翻訳で読みますが、その翻訳を手で確かめたのはドイツ語だけです。そこで Kumi がカバーする範囲は[機能マトリクス](CAPABILITY_MATRIX.md#logic-pro)に、本物の Logic Pro での実行は[実装状況](IMPLEMENTATION_STATUS.md#どこで何をテストしたか)にあります。

## Windows

テスト済みの項目：CI 上の Windows PowerShell 5.1 と PowerShell 7 での、インストーラー、`kumi update`、`kumi bridge`、`kumi uninstall`。非 ASCII 文字の名前のフォルダーにある `KUMI_HOME` での、インストーラー、`kumi bridge`、ロールバック、`kumi uninstall`。そして Live 12.4.15 beta を入れた Windows 10 マシンでの、ユーザーフォルダーの外に移動した User Library への `kumi bridge`、Live での Remote Script の読み込み、Kumi の接続。CI の Windows ランナーは標準のフォルダー構成の管理者アカウントなので、一般のアカウントや移動したライブラリでしか起きない問題は確認できません。

Windows でまだ確認できていないこと：

- **Live が Extensions フォルダーを置く場所。** Kumi は `%LOCALAPPDATA%\Ableton\Extensions` を使い、`KUMI_LIVE_EXTENSIONS_DIR` で上書きできます。これが確認できるまで、拡張機能の機能は Windows では未テストです。
- **Windows のターミナルでのフルスクリーンアプリ。** Windows Terminal を推奨します。[ターミナル](KUMI_TUI.md#ターミナル)を参照してください。
- **Kumi 1.6.0 以前からの `kumi update`** は、PATH 上で Git の `tar` が Windows 自身の `tar` より前にあると（Git Bash から起動した PowerShell など）、tar のエラーで失敗します。インストールのコマンドをもう一度実行するか、`kumi update` の前に `$env:Path = "$env:SystemRoot\System32;$env:Path"` を実行してください。

## 以前のインストール向けの Node.js

| Node.js | 状況 |
| --- | --- |
| 22.x、24.x | 対応。Node 24 LTS を推奨 |
| 25.x | 非対応：2026年6月1日にサポートが終了しました |
| 26.x 以降、21.x 以前、プレリリース | テストされるまで非対応 |

この表は、Node で動く Kumi 1.7.5 以前のインストールに適用されます。その npm のエンジン範囲は `>=22 <23 || >=24 <25` です。その `kumi` はそれ以外のメジャーバージョンでは動作を拒否します（例外は `kumi doctor` で、何が問題かを伝えます）。そのブリッジのサーバーと `ableton-mcp-setup` も拒否し、`ableton-mcp-diagnostics` はそれを報告します。`ableton-mcp-lifecycle` と `ableton-mcp-migrate` は引き続き動作するので、古いインストールを調べたり削除したりできます。

## MCP プロトコル

ブリッジは stdio 上で、二つのプロトコル世代の MCP を話します。initialize ハンドシェイクを使う `2025-11-25` と、リクエストごとのメタデータと `server/discover` を使う `2026-07-28` です。新しい世代では、すべての結果は完全なもので、キャッシュヒントは TTL ゼロの private で、頼まれていないものをプッシュすることはありません。MRTR、Tasks、HTTP は提供しません。テストは両方の世代を対象にしていますが、特定の MCP クライアントやモデルを認定しているわけではありません。クライアントの接続方法は[ユーザーガイド](USER_GUIDE.md)で説明しています。

## アクセシビリティ

`KUMI_UI=plain`（または出力をパイプする）で、Kumi はスクリーンリーダーに向いた、一行ずつ表示するプレーンなインターフェースになります。[プレーンモード](KUMI_TUI.md#プレーンモード)を参照してください。ブリッジ自身の出力は決まった順序のプレーンテキストで、色だけで示す状態はなく、ポインターが必要な操作もありません。どちらも VoiceOver や Narrator ではテストしていません。Live、プラグインのウィンドウ、MCP クライアントの挙動は、それぞれの開発元が決めるものです。

## CI がカバーする範囲

CI は GitHub がホストする macOS、Ubuntu、Windows のランナー上で、Rust のビルドとテスト、Remote Script と Live 拡張機能のテスト、6 ターゲットのバンドル、インストールと移行のテスト、各システムでの REAPER の拡張機能の読み込み、Renoise のツールのテスト、macOS での Logic Pro ブリッジのテスト（記録したシーンと、シミュレートされた Logic Pro に対して）を実行します。どのランナーにも Live、REAPER、Renoise、Logic Pro はありません。すべてのジョブは[テスト](TESTING.md#ci)に記載しています。
