<p align="center">
  <img src="docs/assets/kumi-logo.svg" alt="kumi" width="300">
</p>

<p align="center">
  <a href="https://github.com/user1303836/kumi/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/user1303836/kumi/actions/workflows/ci.yml/badge.svg?branch=main"></a>
  <a href="https://github.com/user1303836/kumi/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/user1303836/kumi?label=release"></a>
  <img alt="Ableton Live 12" src="https://img.shields.io/badge/Ableton%20Live-12-111111">
  <img alt="REAPER 7" src="https://img.shields.io/badge/REAPER-7-111111">
  <img alt="Renoise 3.5" src="https://img.shields.io/badge/Renoise-3.5-111111">
  <img alt="Logic Pro 12" src="https://img.shields.io/badge/Logic%20Pro-12-111111">
  <img alt="Native Rust runtime" src="https://img.shields.io/badge/runtime-native%20Rust-555555">
  <a href="LICENSE.md"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue"></a>
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · 日本語
</p>

**あなたの作業のしかたを覚える、Ableton Live、REAPER、Renoise、Logic Pro のためのスタジオプロデューサーエージェント。** やりたいことを普段の言葉で伝えれば、Kumi があなたの Set の中で直接作業します。面倒な作業から、自分ではなかなか時間を取れない作業まで。YouTube のチュートリアルから音を作り直したり、ミックスをリファレンスに合わせたり、言葉で説明した Max for Live デバイスを書いたり、指し示したラックを作り直したりします。変更はすべて HISTORY に表示され、ほとんどの変更には個別の取り消しが付きます。プラン全体は Live の取り消し一回で戻せます（Cmd-Z、Windows では Ctrl-Z）。Kumi は残したテクニックを覚えておくので、セッションを重ねるごとにあなたに合っていきます。REAPER と Renoise では、Kumi 専用の拡張機能とツールを通じて、それぞれの DAW の言葉で動きます。Apple シリコン搭載 Mac の Logic Pro 12 では、Logic Pro の横で動く Kumi 自身のブリッジを通じて使えます。

https://github.com/user-attachments/assets/f53fe0be-c9a9-476e-a1d5-efe3dc429526

## クイックスタート

macOS 13 以降、または Windows 10・11 上の Ableton Live 12、macOS・Windows・Linux 上の REAPER 7 以降か Renoise 3.5 以降、あるいは Apple シリコン搭載 Mac 上の Logic Pro 12 が必要です。

**macOS または Linux**（ターミナル）：

```sh
curl -fsSL https://github.com/user1303836/kumi/releases/latest/download/install.sh | sh
```

**Windows**（PowerShell）：

```powershell
irm https://github.com/user1303836/kumi/releases/latest/download/install.ps1 | iex
```

インストールが終わると Kumi が起動し、サインインと Live への接続を案内します。次回からは `kumi` を実行するだけです。REAPER や Renoise では `kumi --daw reaper` か `kumi --daw renoise` を実行します。Kumi が拡張機能かツールを入れ、DAW を開きます（[詳しく](docs/ja/KUMI_GUIDE.md#reaper-や-renoise-につなぐ)）。うまくいかないときは `kumi doctor` が直し方を教えます。

Logic Pro では `kumi --daw logic` を実行します。Kumi は Logic Pro を開いたまま、自身のフォルダーに Logic Pro ブリッジを用意し、macOS はアクセシビリティでの許可を一度だけ求めます（[Logic Pro につなぐ](docs/ja/KUMI_GUIDE.md#logic-pro-につなぐ)）。

[ガイド](docs/ja/KUMI_GUIDE.md) · [コマンド、キー、画面](docs/ja/KUMI_TUI.md) · [Kumi が Set を変更するしくみ](docs/ja/KUMI_CHANGES.md) · [変更履歴（英語）](CHANGELOG.md)

## できること

- **Set のほぼすべてを変更：** テンポ、スケール、グルーヴ。ミキサー、ルーティング、サイドチェイン。トラック、シーン、クリップ。ノートと MIDI 変換。デバイス、ラックとそのパラメータ。
- **聴く：** ミックス、サンプル、自分でバウンスした音のラウドネス、トーンバランス、ステレオ幅、テンポとキー。あなたのミックスとリファレンスの違いも聴き分けます。
- **自分の変更を判定する：** 変更のたびにレベルをあるべき所へ戻し（最後の Live の Limiter、なければ Kumi が加える Utility で）、前後を聴いて、狙った項目が良くなり、ほかに悪くなったものがない場合だけ変更を残します。`/loop` はゴールを達成するまでラウンドで作業し、`/goal` はターンをまたいで一つのゴールを続けます。
- **スタイルを測る：** ファイル、フォルダー、YouTube や Spotify のリンク、あるいは「Burial」「dub techno」のような言葉から例のトラックを集め、そのスタイルの範囲を持つプロファイルとして測り、次回のために保存します。
- **リファレンスに合わせる：** 一つの音について何通りかのバージョンを作り、それぞれをリファレンスと比べて採点し、いちばん良いものを磨き上げます。`/loop` を使えば、目標に届くまで続けます。
- **チュートリアルを見る：** YouTube やファイルのビデオを見て、その内容を新しいトラックに組み立てます。
- **再生・録音・リサンプリング**を行い、作ったものを聴かせたり、自分の作業を確かめたりします。
- **Live を隅々まで操作：** 頼まれたものを削除し、MIDI をアレンジメントに直接書き込み、オフラインでレンダリングし、ほかのツールでは届かないところは Live の中で Python を実行し、Live で右クリックしたもの（「Ask Kumi about this」）について答えます。数百トラックの大きな Set でも速いままです。
- **Max for Live デバイスを作る：** 言葉で説明したデバイス（MIDI エフェクト、オーディオエフェクト、インストゥルメント）を作り、トラックに載せます。
- **調べる：** ウェブを検索し、ページ、PDF、マニュアル、GitHub のコードを読むので、読んだものに似たエフェクトも作れます。
- **いまいる場所を表示：** FOCUS は Live で触れたものを追い、デバイスツリー、ピアノロール、セッションやアレンジメントの帯として表示します。デバイスをクリックして指し示せます。「この Saturator、きつすぎる」のように。
- **覚える：** あなたと各 Set についてのメモ、残した音から学んだテクニック、再実行できるレシピ。保存したものはすべて表示され、クリック一つで忘れさせられます。
- **会話を保存：** Set ごとに会話を保存し、どこからでも ID で再開でき（`kumi -r`）、閉じていた間に変わったことも伝えます。`/share` で会話を送れる Web ページにします（キーやパスは取り除かれます）。
- **REAPER と Renoise でも：** REAPER ではトラック、アイテムとテイク、FX とそのパラメーター、エンベロープ、ルーティング。Renoise ではパターンとその行、シーケンス、インストゥルメント、サンプル、フレーズ、DSP チェーン。どの変更にも取り消しがあり、そこでも聴いて判定し、FOCUS は DAW 自身のビューを追います（[DAW ごとにできること](docs/ja/DAWS.md)、[変更できるもの](docs/ja/KUMI_CHANGES.md#reaper-と-renoise)）。
- **Logic Pro 12 でも動く：** Apple シリコン搭載 Mac で、プロジェクトを読み、トラック、リージョン、ノート、プラグイン、センド、オートメーション、マーカー、テンポを Kumi の取り消し付きで変更し、聴き、Kumi を閉じていた間に変わったことを伝え、Apple の Logic Pro ユーザガイドから答えます。[そこでできること、できないこと](docs/ja/KUMI_GUIDE.md#logic-pro-での作業)。
- **好きなモデルで：** ChatGPT でサインインするか、OpenAI・Anthropic・OpenCode の API キーを使います。

## 現状

Kumi 1.21.1 は macOS の Ableton Live 12.4（ベータ）で確認しています。
Windows ではインストールと更新を確認済みですが、Live と一緒に使うのはまだ新しいです。
問題が起きたら `kumi report` を送ってください。
REAPER 7.82 と Renoise 3.5.4 は macOS で確認しています。Windows と Linux では、Kumi の拡張機能とツールの配置と拡張機能の読み込みを CI で確認していますが、そこで REAPER や Renoise と一緒に使うのはまだ新しいです。
Logic Pro 12 には、Apple シリコン搭載 Mac で Kumi 自身の Logic Pro ブリッジを通じて対応しています（Logic Pro 12.3.1 と macOS 26.6.1 で確認）。Intel Mac、Linux、Windows には Logic Pro ブリッジはありません。
Logic Pro で問題が起きたら `kumi --daw logic report` を送ってください。

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
| `crates/kumi-runtime` | Kumi のエージェントコア：モデルのプロバイダー、メモリー、音声解析、ビデオ、ウェブ、Live・REAPER・Renoise・Logic Pro との連携 |
| `crates/kumi-common`、`crates/kumi-store` | Kumi のクレートが共有するものと、セッションをまたいで Kumi が保存するもの（SQLite） |
| `crates/kumi-genexpr` | Kumi が作る Max for Live デバイスのテストのための GenExpr インタープリター |
| `crates/ableton-mcp-server` | Live のブリッジ：Kumi が起動するローカルの MCP サーバー。ほかの MCP クライアントからも単独で使えます（[ブリッジのガイド（英語）](crates/ableton-mcp-server/README.md)） |
| `remote-script` | Live の中で動く、ブリッジの Remote Script |
| `apps/live-extension` | Kumi の Live 拡張機能（Live 12.4 以降） |
| `protocol` | ブリッジと Remote Script が共有する操作の一覧 |
| `crates/daw-mcp-server` | Kumi が起動する、REAPER、Renoise、Logic Pro のための Kumi のホスト。ほかの MCP クライアントからも使えます |
| `crates/daw-bridge` | Kumi の REAPER、Renoise、Logic Pro のブリッジが共有するもの |
| `crates/reaper-kumi` | Kumi の REAPER 拡張機能 |
| `daws/renoise` | Kumi の Renoise ツール |
| `crates/logic-kumi` | Kumi の Logic Pro ブリッジ（Logic Pro の横で動くプログラム）と、そのアクセシビリティ用ヘルパー |

ビルド、テスト、リリースについては[開発者ガイド](docs/ja/DEVELOPER_GUIDE.md)で説明しています。

## ライセンス

[MIT](LICENSE.md)。他の人の成果から来た部分と、Kumi が取得するモデルについては [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) にあります。Ableton Live は Ableton AG の商標です。Kumi は Ableton、Cockos（REAPER）、Renoise と提携しておらず、その承認を受けたものでもありません。Logic Pro は Apple Inc. の商標です。Kumi は Apple と提携しておらず、Apple の承認を受けたものでもありません。
