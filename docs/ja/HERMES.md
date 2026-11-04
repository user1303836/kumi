# Hermes Agent で使う Kumi

[English](../en/HERMES.md) · [简体中文](../zh-CN/HERMES.md) · 日本語

Kumi は [Hermes Agent](https://hermes-agent.nousresearch.com) のプラグインとしても使えます。エージェントは Hermes のまま（ターミナル、Hermes Desktop、Web ダッシュボード、チャットアプリ）で、Kumi はそこに DAW を加えます。変更の前には毎回 Hermes が確認し、Kumi はそれぞれの変更を取り消し付きで HISTORY に記録します。どちらもあなたのコンピューターで動きます。

単体の Kumi（ターミナルで使う `kumi` アプリ）と Hermes の中の Kumi は別々のインストールです。どちらか一方だけでも使えます。

## インストール

```sh
hermes plugins install kumi
hermes plugins enable kumi
```

Hermes Desktop では、**Capabilities → Plugins** で Kumi もオンにしてください。プラグインの Desktop 側は最初はオフになっています。

## セットアップ

Hermes に「Live 用に Kumi をセットアップして」（REAPER や Renoise でも）と頼むか、ターミナルで `hermes kumi setup` を実行します。セットアップは 2 段階で、それぞれの前に Hermes が確認します。

1. **Kumi のランタイム:** お使いのコンピューター向けの Kumi 公式リリース（25〜32 MB）を Kumi の GitHub リリースからダウンロードします。プラグインが固定している SHA-256 と照合し、このプロファイル用の Hermes のデータ内に展開します（77〜113 MB）。
2. **DAW 側の Kumi:** Live の Remote Script（トラックを聴くための Kumi Ears デバイスを含む）、REAPER の拡張機能、または Renoise のツールを、セットアップが示す正確な場所にインストールします。

そのあと DAW で:
- **Live:** 設定 → Link, Tempo & MIDI で **AbletonMcpBridge** をコントロールサーフェスに選びます。
- **REAPER:** REAPER を再起動します。
- **Renoise:** Kumi のツールを有効にします。

単体の Kumi がすでに DAW 側をインストールしている場合、セットアップはそれを伝え、置き換えずにそのインストールを使うよう提案します。その場合、Hermes の中の Kumi はそのインストールの接続用ファイルを使いますが、サインイン情報は使いません。

セットアップが DAW を起動・終了したり、コントロールサーフェスを選んだり、プロジェクトを保存したりすることはありません。

## 頼めること

- 「いま何を見てる？」選択中のトラック、デバイス、クリップを Kumi が読みます。
- 「もっと暗いスネアを探して」指定したサンプルフォルダーから探します。Hermes Desktop ではサウンドマップが開きます。
- 「それをパッド 2 に入れて」何が変わるかを Hermes が正確に示して確認し、Kumi が変更して HISTORY に加えます。
- 「本当に暗くなった？」Kumi が両方のファイルを測ります（ラウドネス、トゥルーピーク、スペクトラム）。
- 「スネアの変更を取り消して」取り消せるあいだは、その項目を取り消します。

## どこに何があるか

- **すべての画面:** Kumi のツール、スキル（`kumi:daw`、`kumi:sounds`、`kumi:listening`）、状態を見る `/kumi`。
- **Hermes Desktop:** ステータスチップ、FOCUS と HISTORY のサイドペイン、専用タブのサウンドマップ、測定した音声のカード、パレットのコマンド。ボタンはチャットに依頼を入れるだけで、ボタンだけでプロジェクトが変わることはありません。
- **Web ダッシュボード:** HISTORY、状態、マップのある Kumi タブ。

## 設定

| 設定 | 既定 | 内容 |
|---|---|---|
| DAW | Live | Kumi がつなぐ DAW |
| サンプルフォルダー | なし | サウンドマップのために Kumi が読んでよいフォルダー。それ以外は読みません |
| 見ているものをエージェントに伝える | オン | Kumi の接続中、各ターンに短い FOCUS とその間の変更のメモを加えます |
| 測定結果を解釈する | オフ | 測定値と小さな帯域エネルギーの画像を Hermes のモデルにも送り、言葉で説明させます |

## コンピューターの外に出るもの

- **セットアップ:** Kumi のバージョンごとに一度、Kumi の GitHub リリースからランタイムをダウンロードします。
- **会話:** Kumi が書き留めたプロジェクトの情報（FOCUS、変わったこと）と測定結果は、会話の一部として Hermes で使っているモデルに送られます。
- **測定結果を解釈する**（オンにしたときだけ）: 測定値と、測った帯域エネルギーの小さな画像を、Kumi の聴取タスク用に選んだモデルに送ります。音声そのものやファイルのパスは送りません。
- **音声は手元に:** Kumi は音声をこのコンピューターで測り、アップロードしません。
- Hermes の中の Kumi は、Kumi 自身のサインイン、モデルプロバイダー、アップデーターを使わず、テレメトリーも送りません。

## 制限

- DAW は Hermes を動かしているコンピューターで動いている必要があります。リモートの Hermes につないだ Hermes Desktop が操作するのは、そのコンピューターの DAW です。
- Live は macOS と Windows、REAPER と Renoise は macOS、Windows、Linux で使えます。
- Hermes の確認なしに動いてしまうものは Hermes の中の Kumi に含めていません（DAW 内でのコード実行、Live のメニュー操作、録音、プロジェクトの保存や読み込み）。

## うまくいかないとき

- `/kumi`（ターミナルでは `hermes kumi status`）で、セットアップに足りないものがわかります。
- Hermes Desktop に Kumi が出てこない: Capabilities → Plugins で Kumi がオンか、Hermes で有効か（`hermes plugins enable kumi`）を確かめてください。
- Live につながらない: AbletonMcpBridge がコントロールサーフェスに選ばれているか確かめてください。
- Live のトラックを聴くときに Kumi Ears がないと言われる: セットアップをもう一度実行してください。Hermes の中の Kumi が別の Kumi のインストールを使っている場合は、その Kumi で `kumi bridge` を実行してください。Kumi Ears には Max for Live が必要です。
