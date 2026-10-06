<p align="center">
  <img src="docs/assets/kumi-logo.svg" alt="kumi" width="300">
</p>

<p align="center">
  <a href="https://github.com/user1303836/kumi/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/user1303836/kumi/actions/workflows/ci.yml/badge.svg?branch=main"></a>
  <a href="https://github.com/user1303836/kumi/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/user1303836/kumi?label=release"></a>
  <img alt="Ableton Live 12" src="https://img.shields.io/badge/Ableton%20Live-12-111111">
  <img alt="Native Rust runtime" src="https://img.shields.io/badge/runtime-native%20Rust-555555">
  <a href="LICENSE.md"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue"></a>
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · 日本語
</p>

**あなたの作業のしかたを覚える、Ableton Live のためのスタジオプロデューサーエージェント。** やりたいことを普段の言葉で伝えれば、Kumi があなたの Set の中で直接作業します。面倒な作業から、自分ではなかなか時間を取れない作業まで。YouTube のチュートリアルから音を作り直したり、ミックスをリファレンスに合わせたり、言葉で説明した Max for Live デバイスを書いたり、指し示したラックを作り直したりします。変更はすべて HISTORY に表示され、ほとんどの変更には個別の取り消しが付きます。プラン全体は Live の取り消し一回で戻せます（Cmd-Z、Windows では Ctrl-Z）。Kumi は残したテクニックを覚えておくので、セッションを重ねるごとにあなたに合っていきます。ほかの DAW への対応も予定しています。

<p align="center">
  <img src="docs/assets/kumi-screenshot.png" alt="ビデオチュートリアルから Drift のベースを作り直す Kumi：手順ごとの会話、新しいトラックのデバイスチェーンを表示する FOCUS、変更ごとに取り消しの付いた HISTORY" width="760">
</p>

## クイックスタート

macOS 13 以降、または Windows 10・11 上の Ableton Live 12 が必要です。

**macOS**（ターミナル）：

```sh
curl -fsSL https://raw.githubusercontent.com/user1303836/kumi/main/install.sh | sh
```

**Windows**（PowerShell）：

```powershell
irm https://raw.githubusercontent.com/user1303836/kumi/main/install.ps1 | iex
```

インストールが終わると Kumi が起動し、サインインと Live への接続を案内します。次回からは `kumi` を実行するだけです。うまくいかないときは `kumi doctor` が直し方を教えます。

[ガイド](docs/ja/KUMI_GUIDE.md) · [コマンド、キー、画面](docs/ja/KUMI_TUI.md) · [Kumi が Set を変更するしくみ](docs/ja/KUMI_CHANGES.md) · [変更履歴（英語）](CHANGELOG.md)

## できること

- **Set のほぼすべてを変更：** テンポ、スケール、グルーヴ。ミキサー、ルーティング、サイドチェイン。トラック、シーン、クリップ。ノートと MIDI 変換。デバイス、ラックとそのパラメータ。
- **聴く：** ミックス、サンプル、自分でバウンスした音のラウドネス、トーンバランス、ステレオ幅、テンポとキー。あなたのミックスとリファレンスの違いも聴き分けます。
- **リファレンスに合わせる：** 一つの音について何通りかのバージョンを作り、それぞれをリファレンスと比べて採点し、いちばん良いものを磨き上げます。`/goal` を使えば、目標に届くまで続けます。
- **チュートリアルを見る：** YouTube やファイルのビデオを見て、その内容を新しいトラックに組み立てます。
- **再生・録音・リサンプリング**を行い、作ったものを聴かせたり、自分の作業を確かめたりします。
- **Live を隅々まで操作：** 頼まれたものを削除し、MIDI をアレンジメントに直接書き込み、オフラインでレンダリングし、ほかのツールでは届かないところは Live の中で Python を実行し、Live で右クリックしたもの（「Ask Kumi about this」）について答えます。数百トラックの大きな Set でも速いままです。
- **Max for Live デバイスを作る：** 言葉で説明したデバイス（MIDI エフェクト、オーディオエフェクト、インストゥルメント）を作り、トラックに載せます。
- **調べる：** ウェブを検索し、ページ、PDF、マニュアル、GitHub のコードを読むので、読んだものに似たエフェクトも作れます。
- **いまいる場所を表示：** FOCUS は Live で触れたものを追い、デバイスツリー、ピアノロール、セッションやアレンジメントの帯として表示します。デバイスをクリックして指し示せます。「この Saturator、きつすぎる」のように。
- **覚える：** あなたと各 Set についてのメモ、残した音から学んだテクニック、再実行できるレシピ。保存したものはすべて表示され、クリック一つで忘れさせられます。
- **会話を保存：** Set ごとに会話を保存し、閉じていた間に変わったことも伝えます。
- **好きなモデルで：** ChatGPT でサインインするか、OpenAI・Anthropic・OpenCode の API キーを使います。

## 現状

Kumi 1.8.11 は macOS の Ableton Live 12.4（ベータ）で確認しています。
Windows ではインストールと更新を確認済みですが、Live と一緒に使うのはまだ新しいです。
問題が起きたら `kumi report` を送ってください。次は Renoise と Reaper への対応を予定しています。

## 開発

このチェックアウトは Rust と Cargo でビルドします。

```sh
cargo build --release --locked --workspace --bins
cargo run --release -p kumi --                     # -- の後に bridge、doctor などを追加
npm ci --prefix crates/kumi-runtime/tests/support  # 初回のみ：一部のテストが使う公式 SDK
sh scripts/test-isolated.sh                        # Live もサインインも不要
```

従来の `npm run setup` と `npm run kumi -- ...` も使えます。Cargo がある場合はこのチェックアウトを
ビルドして実行します。Cargo がない場合は、同じバージョンの公開済みネイティブ版をインストールして
実行します。`~/.kumi` の設定、サインイン、会話、ライブラリーはそのままです。
以後は `kumi` でネイティブ版を直接起動できます。

| フォルダー | 内容 |
| --- | --- |
| `crates/kumi` | ターミナルアプリと `kumi` コマンド |
| `crates/kumi-runtime` | Kumi のエージェントコア：モデルのプロバイダー、メモリー、音声解析、ビデオ、ウェブ、Live との連携 |
| `crates/ableton-mcp-server` | ブリッジ：Kumi が起動するローカルの MCP サーバー。ほかの MCP クライアントからも単独で使えます（[ブリッジのガイド（英語）](crates/ableton-mcp-server/README.md)） |
| `remote-script` | Live の中で動く、ブリッジの Remote Script |
| `apps/live-extension` | Kumi の Live 拡張機能（Live 12.4 以降） |
| `protocol` | ブリッジと Remote Script が共有する操作の一覧 |

ビルド、テスト、リリースについては[開発者ガイド](docs/ja/DEVELOPER_GUIDE.md)で説明しています。

## ライセンス

[MIT](LICENSE.md)。Ableton Live は Ableton AG の商標です。Kumi は Ableton と提携しておらず、Ableton の承認を受けたものでもありません。
