<p align="center">
  <img src="docs/assets/kumi-logo.svg" alt="kumi" width="300">
</p>

<p align="center">
  <a href="https://github.com/user1303836/kumi/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/user1303836/kumi/actions/workflows/ci.yml/badge.svg?branch=main"></a>
  <a href="https://github.com/user1303836/kumi/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/user1303836/kumi?label=release"></a>
  <img alt="Ableton Live 12" src="https://img.shields.io/badge/Ableton%20Live-12-111111">
  <img alt="REAPER 7" src="https://img.shields.io/badge/REAPER-7-111111">
  <img alt="Renoise 3.5" src="https://img.shields.io/badge/Renoise-3.5-111111">
  <img alt="Native Rust runtime" src="https://img.shields.io/badge/runtime-native%20Rust-555555">
  <a href="LICENSE.md"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue"></a>
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a> · 日本語
</p>

**Kumi は、DAW の中であなたと一緒に作業する汎用の AI エージェントです。** 現在は Ableton Live、REAPER、Renoise に対応し、どの DAW でも同じことができます。プロジェクトを読み、その中のほぼすべてを変更し、結果を聴き、あなたの音、好み、癖を学びます。ターミナルで動くチャットに、普段の言葉で頼みます。Kumi は変更を計画して実行し、各ステップを取り消し付きで履歴に残します。使うほどに、Kumi はあなたの作業のしかた、好み、プロデューサーとしてのパターンをよく知るようになります。

https://github.com/user-attachments/assets/f53fe0be-c9a9-476e-a1d5-efe3dc429526

## クイックスタート

Mac ではターミナル、Windows では PowerShell で、コマンド一つで始められます。

Mac または Linux：

```sh
curl -fsSL https://github.com/user1303836/kumi/releases/latest/download/install.sh | sh
```

Windows：

```powershell
irm https://github.com/user1303836/kumi/releases/latest/download/install.ps1 | iex
```

Kumi のガイド付きセットアップに従ってください。

次回からは、ターミナルを開いて `kumi` と入力し、DAW（Live、REAPER、Renoise。Kumi の前でも後でもかまいません）を開きます。Kumi は開いている DAW につながります。

> [!NOTE]
> Kumi はターミナルのウィンドウで動きますが、コマンドを知っている必要はありません。インストールの一行のあとは、チャットです。テキストはドラッグするだけでコピーでき、ダブルクリックで単語をコピーできます。

## Kumi に頼めること

- **チュートリアルから音を作る。** YouTube のリンクを貼ると、Kumi がビデオを見て、その内容を新しいトラックに組み立てます。
- **ミックスを直す。** 「ベースはこもっている？」と聞くと、Kumi が聴いて、ぶつかっているものを見つけて直し、変更のたびに聴き直します。
- **リファレンスに合わせる。** 好きな曲を渡すと、Kumi がいくつかのバージョンを試し、それぞれをその曲と比べて採点し、いちばん良いものを磨き上げます。
- **プラグインのプリセット作成とマスタリング。** 組み込みの VST3 形式を使い、DAW 標準デバイスの前に適切なインストール済みプラグインを確認します。作業を続けながら、Kumi にバックグラウンドでプリセットを作ってもらえます。[対応形式と制限](plugin-formats/README.md)。
- **デバイスを作る。** Max for Live のエフェクトやインストゥルメントを言葉で説明すると、Kumi が作ってトラックに載せます。
- **自分の音を探す。** 「これみたいな埃っぽいスネア」「いつものボーカルチェーン」：Kumi はあなたのサンプル、プリセット、プロジェクトを知っています。
- **音を地図にする。** `/map` はあなたのサンプルと曲を、似たものどうしが近くに来るように並べます。点を指せばその音を聴けます。気に入った音をパレットに集めておくと、「パレットからもっと暗いキックを」と頼んだときに Kumi がそこから選び、パレットをまるごとキットとして読み込むこともできます。
- **雑用を片づける。** 名前の変更、色付け、ルーティング、サイドチェイン、グループ化、フリーズ、バウンス、書き出し。

3 つの DAW のどれでも同じように、それぞれの DAW の言葉で動きます：[DAW ごとにできること](docs/ja/DAWS.md)。

## よくある質問

<details>
<summary><b>コードを書ける必要はありますか？</b></summary>

いいえ。Kumi とは普段の言葉で、文字でも声でもやり取りできます。コードが必要な作業（Live では Python、REAPER と Renoise では Lua）は、Kumi が自分で書いて実行します。

</details>

<details>
<summary><b>プロジェクトを壊されませんか？</b></summary>

変更はすべて Kumi の履歴に並び（「Tempo 120 → 124 BPM」）、ほとんどには個別の取り消しが付きます。Live では、一つの依頼が Live 自身の取り消しの 1 ステップにもなり、大きな変更や削除を伴う変更の前には、最後に保存した Set を Kumi がコピーしておきます。

</details>

<details>
<summary><b>対応する DAW とコンピューターは？</b></summary>

- macOS 13 以降、または Windows 10・11 上の Ableton Live 12
- macOS・Windows・Linux 上の REAPER 7 以降
- macOS・Windows・Linux 上の Renoise 3.5 以降

複数入っているときは、どれで作業するかをセットアップが尋ねます。あとから `/daw` で切り替えられます。

</details>

<details>
<summary><b>どの AI を使いますか？費用は？</b></summary>

Kumi は無料のオープンソース（MIT）です。モデルはあなたが選びます：

- **ChatGPT のプラン：** アカウントでサインインします。Kumi はプランの利用上限の範囲で動きます。
- **API キー：** Anthropic、OpenAI、Google Gemini、OpenRouter と[ほか 20 のプロバイダー](docs/ja/KUMI_GUIDE.md#サインインとモデルの選択)。料金はプロバイダーに支払います。
- **ローカル：** Ollama、LM Studio、または llama.cpp や vLLM のような OpenAI 互換のサーバー。ツールを呼び出せるモデルを長いコンテキスト（Kumi は約 57k トークンを求めます）で動かせるハードウェアが必要です。

</details>

<details>
<summary><b>オーディオはコンピューターの外に出ますか？</b></summary>

オーディオはローカルで解析され、モデルに届くのは測定値です。ライブラリーと音楽のコレクションもローカルで学習されます。Gemini か OpenAI の API キーがあると、リスニングモデルも、判定する変更のモノラルの抜粋（最長 10 秒）を聴きます。`/slots listening off` で止められます。ほかは[プライバシー](docs/ja/KUMI_GUIDE.md#プライバシーコンピューターの外に出るもの)にあります。

</details>

<details>
<summary><b>うまく動かないときは</b></summary>

`kumi doctor` がインストール、サインイン、ブリッジ、DAW への接続を調べ、直し方を伝えます。`kumi report` は [issue](https://github.com/user1303836/kumi/issues) に添付するファイルを作ります。

</details>

[ガイド](docs/ja/KUMI_GUIDE.md) · [コマンド、キー、画面](docs/ja/KUMI_TUI.md) · [Kumi がプロジェクトを変更するしくみ](docs/ja/KUMI_CHANGES.md) · [変更履歴（英語）](CHANGELOG.md)

## 現状

Kumi 1.24.14 は macOS の Ableton Live 12.4（ベータ）で確認しています。
Windows ではインストールと更新を確認済みですが、Live と一緒に使うのはまだ新しいです。
問題が起きたら `kumi report` を送ってください。
REAPER 7.82 と Renoise 3.5.4 は macOS で確認しています。Windows と Linux では、Kumi の拡張機能とツールの配置と拡張機能の読み込みを CI で確認していますが、そこで REAPER や Renoise と一緒に使うのはまだ新しいです。

## 開発

Kumi をソースからビルドするには、[DEVELOPMENT.md](DEVELOPMENT.md)（英語）と[開発者ガイド](docs/ja/DEVELOPER_GUIDE.md)を参照してください。

## ライセンス

[MIT](LICENSE.md)。他の人の成果から来た部分と、Kumi が取得するモデルについては [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) にあります。Ableton Live は Ableton AG の商標です。Kumi は Ableton、Cockos（REAPER）、Renoise、および対応するプラグインのメーカーと提携・関係・スポンサー契約・推奨関係を持ちません。製品名は互換性のあるソフトウェアを示すために使用しています。
