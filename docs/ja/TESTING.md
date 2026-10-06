# テスト

[English](../en/TESTING.md) · [简体中文](../zh-CN/TESTING.md) · 日本語

リポジトリの各部分のテストの実行方法、それに必要なもの、CI が何を実行するかをまとめます。通常のテストには、Live もサインインも必要ありません。

## クイックスタート

リポジトリのルートから実行します：

```sh
npm ci --prefix crates/kumi-runtime/tests/support   # 初回のみ：一部の Rust テストが使う公式 SDK
cargo build --workspace --all-targets --locked
sh scripts/test-isolated.sh                        # Windows: ./scripts/test-isolated.ps1
python3 -m unittest discover -s remote-script -p 'test_*.py'
npm ci --prefix apps/live-extension
npm test --prefix apps/live-extension
python3 -m unittest discover -s scripts/tests -p 'test_*release.py'
```

必要なもの：

| 必要なもの | 用途 |
| --- | --- |
| Rust と Cargo | クレートと移行テスト |
| PATH 上の Node.js（CI は 24 を使用） | 一部の Rust テスト、Live 拡張機能のテスト、移行テスト |
| PATH 上の Python 3.11 以降（`python3`、Windows では `python.exe`） | Remote Script のテスト、リリーススクリプトのテスト、Python を実行する Rust テスト |
| `vendor/` にローカルで用意した Extensions SDK | Live 拡張機能のビルドや型チェック（そのテストには不要） |

Windows では、いくつかのテストがシンボリックリンクを作成するため、開発者モードか管理者アカウントが必要です。それがないと、一部のテストはスキップされ、いくつかは `EPERM` で失敗します。CI の Windows ランナーにはその権限があります。

## Kumi とブリッジ

リポジトリのルートから実行します。

| コマンド | 内容 |
| --- | --- |
| `sh scripts/test-isolated.sh`（Windows では `./scripts/test-isolated.ps1`） | テスト専用のホームで `cargo test --workspace --locked` を実行します。引数は `cargo test` に渡します |
| `sh scripts/test-isolated.sh -p kumi-runtime --test hands_transport` | 一つのクレートの、一つのテストファイルだけを実行します |
| `cargo fmt --all --check` | CI と同じようにフォーマットを確認します |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Lint。CI では今のところ参考扱いです |
| `cargo clippy --workspace --lib --bins --examples --locked -- -A clippy::all -D clippy::await_holding_refcell_ref` | await をまたいで保持される `RefCell` の借用。Linux の CI ジョブを失敗させます |
| `cargo run --locked --release -p ableton-mcp-server --bin ableton-mcp-benchmark` | ブリッジの性能の基準。[開発者ガイド](DEVELOPER_GUIDE.md#ビルドテスト計測)を参照してください |

分離ランナーはテスト専用のホームを用意します。`HOME`、`USERPROFILE`、`APPDATA`、`LOCALAPPDATA`、`XDG_CONFIG_HOME`、`KUMI_HOME` は新しい一時フォルダーの中を指し、`KUMI_REMOTE_SCRIPTS_DIR` と `KUMI_LIVE_EXTENSIONS_DIR` は取り除かれるので、どのテストもあなたの Live のフォルダーや `~/.kumi` には届きません。

公式の MCP とモデルの SDK を Kumi 自身のクライアントやプロバイダーと並べて実行するテストは、`npm ci --prefix crates/kumi-runtime/tests/support` を実行するまで失敗します。ラウドネスとトゥルーピークのテストは、生成した音声に対する FFmpeg の `ebur128` の結果と解析を照らし合わせます。

クレートのテストにある JSON のオラクルファイルは、TypeScript 実装から記録したゴールデンファイルです。その実装と、ファイルを生成したスクリプトは git タグ `v1.7.6` に残っていますが、このツリーにはファイルを再生成するものがありません。意図して動作を変えたときは、影響を受けるオラクルの項目をネイティブの出力から書き直します。失敗したアサーションが実際の出力を表示します。SHA-256 の項目には、その出力のハッシュを書きます。差分を確認し、ゴールデンファイルを変えたことと、その理由をコミットメッセージに書いてください。ホストのテストは、ゴールデンファイルを記録したときのブリッジのバージョン（`ORACLE_VERSION`）に固定して動くので、ブリッジのバージョンを上げてもゴールデンファイルは変わりません。

## Remote Script

リポジトリのルートから：

```sh
python3 -m unittest discover -s remote-script -p 'test_*.py'
python3 -m compileall -q remote-script/AbletonMcpBridge
```

テストは Remote Script を偽の Live オブジェクトに対して実行します：認証、シーケンス、メインスレッドのキュー、レジストリとそのハッシュ、ディスカバリー、トランザクション、キャプチャとリアルタイムの安全性、そしてオプションの Willington プロバイダーです。

## Kumi の Live 拡張機能

`apps/live-extension` には独自の `package.json` があります。`npm ci --prefix apps/live-extension` を実行した後、`npm test --prefix apps/live-extension` は、コミットされた `dist/extension.js` を偽の Live に対して読み込み、記録された sha256 と照合します。そこでのビルド（`npm run build`）と型チェック（`npm run typecheck`）には `vendor/` 内の Extensions SDK が必要です。それがなければ、コミットされたビルドがそのまま残ります。再ビルドした後は、`dist/extension.js` をその `.sha256` と一緒にコミットしてください。

## リリーススクリプト

`python3 -m unittest discover -s scripts/tests -p 'test_*release.py'` は、パッケージングと移行のテストを実行します。パッケージングのテストは、バンドルの内容とバージョン、そしてパッケージされたガイドのすべてのリンクが解決できることを確認します。移行テストは、最後の JavaScript リリースの更新処理をネイティブのバンドルに対して実行します。次の変数がない場合、それを必要とするテストはスキップされます：

| 変数 | 指すもの |
| --- | --- |
| `KUMI_LEGACY_APP` | 展開済みの Kumi 1.7.5。`python3 scripts/fetch-legacy-release.py [folder]` がダウンロードし、SHA-256 を確認して展開し、そのフォルダーを出力します |
| `KUMI_NATIVE_RELEASES` | ビルド済みのリリースアーティファクトのフォルダー（[リリース](DEVELOPER_GUIDE.md#リリース)を参照） |

`KUMI_LEGACY_APP` は、ブリッジ自身の移行テスト `crates/ableton-mcp-server/tests/lifecycle_migration.rs` にも使われます。このテストは変数がなくてもスキップされず、公開済みの Kumi 1.7.5 のバンドルをダウンロードして SHA-256 を確認します。変数を設定しておけば、バンドルを一度取得したあとはオフラインで実行できます。

## Live やモデルを使うチェック

これらはオプトインです。実際に何かを変更したり実際にトークンを消費したりするので、CI では実行しません。

| コマンド（ルートから） | 必要なもの | 内容 |
| --- | --- | --- |
| `cargo run --release -p kumi --example accept_live -- --set "<Set>"` | Set の使い捨てコピーを開いた Live と、先にビルドしておいたブリッジ（`cargo build --release -p ableton-mcp-server --bins`。デバッグ実行では `--release` を付けずに同じコマンド） | Kumi ができるあらゆる種類の変更を行い、それぞれを Kumi の取り消しで元に戻し、再生、バウンス、聴き取り、監視を行い、大きな Set の読み取りにかかる時間を計測します。モデルは使いません。 |
| `cargo run --release -p kumi --example time_rebuild` | Set を開いた Live と、`accept_live` と同じく先にビルドしたブリッジ | チュートリアルの再現が Live でどれだけかかるかを、モデルなしで計ります。Drift と四つのエフェクトを載せた MIDI トラックを一つのプランで作り、各デバイスのパラメータを読み、30 個を名前で設定し、Kumi が Set を見る時間も含めて、それぞれにかかったブリッジへのリクエスト数と一緒に表示します。トラックを一つ追加し、最後に削除します。 |
| `cargo run --release -p kumi --example eval_changes [-- <part of a case name>]` | サインインとモデル | モデルが Kumi のツールをどう使うかを、本物のブリッジのツールスキーマ（ネイティブのカタログから読み込みます）を持つ合成ブリッジに対して評価します。Live には一切触れません。各ケースは、かかった時間、そのうちツールの時間、モデルの呼び出し回数を示します。`EVAL_EFFORT` でモデルの推論の度合いを設定し、`EVAL_TRACE=1` で呼び出しを一つずつ表示します。`EVAL_TUTORIAL=1` は YouTube の実際の 16 分のチュートリアルを加えます（ネットワークが必要です）。`EVAL_MEASURE=1` は、サインインもモデルも使わずに、すべてのリクエストが運ぶもの（指示と各ツールの定義）をバイト数で表示します（`EVAL_MEASURE=tools` では定義そのものも表示します）。 |
| `cargo run --release -p kumi --example probe_inference` | サインイン | 無害なツールを使った認証済みのリクエストを一つ送ります。Live には一切触れません。 |
| `cargo run --release -p kumi --example probe_cache` | サインインとモデル | 再開のときのようにカーネルを作り直しても、会話がプロバイダーのプロンプトキャッシュを保てるかを確かめます。短いターンごとに入力トークンとキャッシュされたトークンを表示し、同じ会話、別の会話、会話なしで作り直します。Live には一切触れません。 |

合成ブリッジの Operator、Saturator、EQ Eight には、Live から `crates/kumi/examples/fixtures/eval_changes/live-devices.json` に読み込んだ、Live 12.4 のすべてのパラメータがあり、パラメータを設定する Kumi 自身のスクリプトを Live と同じように実行します。

## ドキュメント

ドキュメントを編集した後、パッケージングのテストを実行します：

```sh
python3 -m unittest discover -s scripts/tests -p test_native_release.py
```

このテストは、ブリッジと一緒に配布されるガイドのすべてのリンクを確認します。対象は、ブリッジの README と、`scripts/build-native-release.py` の `DOCUMENTS` に並んでいる `docs/en` の 14 ページです。3 言語の内容が一致しているかを確認するものはないので、手作業でそろえてください。

## CI

プルリクエストのたびに次のワークフローが実行され、最初の二つは `main` へのプッシュのたびにも実行されます：

| ワークフロー | ジョブ | 実行内容 |
| --- | --- | --- |
| **CI** | `Rust / Linux`、`Rust / macOS`、`Rust / Windows` | `cargo fmt --check`、すべてのターゲットのビルド、公式 SDK をインストールした状態での分離ランナーによる全テスト（Windows では先にコンソール入力のテスト）、Clippy（参考扱い。ただし await をまたいで保持される `RefCell` の借用は Linux ジョブを失敗させます）、`git diff --check` |
| | `Python Remote Script / ubuntu-24.04`、`macos-15`、`windows-2025`（Python 3.11） | Remote Script のテストを実行し、パッケージをコンパイルします |
| | `Live extension`（Ubuntu、Node 24） | コミットされたビルドに対する、拡張機能のテスト |
| | `Release scripts`（Ubuntu） | この変更の空白のチェック、続いてパッケージングのテスト |
| | `Required CI` | 上記がすべてパスした場合にだけパスします |
| **Willington files** | `Willington files`（Ubuntu） | `vendor/willington/` を変えられるのは、このリポジトリの `willington/` ブランチから出たリポジトリ所有者のプルリクエストだけで、そのプルリクエストはほかに何も変えません。`main` にあるこのチェックが、プルリクエストのコードではなくファイルの一覧を読んで実行されます |
| **Installer** | `Native bundle / <target>`（6 つ）、`Aggregate native and existing-installer releases`、続いて `Install / <system>`（6 つ）と `Existing installer transition / <system>`（3 つ） | macOS、Linux、Windows の Intel と ARM 向けのネイティブバンドル（Mac のバンドルには、Kumi が Live のメニューを使うためのヘルパー（ユニバーサル、アドホック署名）が入り、Intel 向けは Apple Silicon でクロスビルドします）、続いて既存のインストールが更新に使う互換リリースをビルドし、ローカルで配信します。各システムで：プロデューサーと同じ方法でインストールし（Windows では Windows PowerShell 5.1）、バージョン、`doctor`、ブリッジとその解析ワーカーを確認し、修復として再インストールし、使い捨ての Remote Scripts フォルダーに `kumi bridge --yes` を実行し、`kumi update`、`kumi update --rollback`、`kumi uninstall` を実行します。移行のジョブは、Kumi 1.7.5 と新しいバンドルで移行テストを実行します。`v*` タグでは、続いて `publish` がバンドルをリリースに添付します。 |

プルリクエストでは、実行される範囲が狭くなります：

- `Rust / macOS` と `Rust / Windows` はプラットフォームによって異なるテストを実行し、`Python Remote Script` は macOS を省きます。
- Installer が 6 つすべてをビルドしてインストールするのは、インストール、更新、リリース、Willington のファイル、または依存関係を変える変更の場合だけです。ブリッジやバージョン番号を変える変更では Linux のものをビルド、インストール、確認し、それ以外の変更では Linux のバンドルをビルドするだけです。
- Kumi が保持するものの保存方法（`crates/kumi-store`、設定とサインイン、メモリー、テクニック、プレイブック、ギャップ、以前の Kumi のファイル）を変える変更にも、既存のデータを残したまま更新とロールバックを行う、Linux のインストールと更新の確認が付きます。
- プルリクエストと `main` へのプッシュでは、Installer のバンドルを `ci-release` プロファイルでビルドします。これはリリースプロファイルのプログラム全体の最適化を省きます。タグは `release` でビルドします。

`main` へのプッシュとタグではすべてが実行されます。CI は毎晩すべてを実行し、Installer も `release` プロファイルで毎晩すべてを実行して、次のタグのためにそのプロファイルのビルドキャッシュを温めておきます。

`main` にマージするには、`Required CI` と `Willington files` がパスする必要があります。Installer は必須ではありません。残りのルールは[リリースと配布](DISTRIBUTION_POLICY.md#マージゲート)にあります。

## パスが意味すること

パスは、コードがテストに書かれたとおりに動作すること、パッケージが macOS、Linux、Windows でインストールでき動作すること、インストーラーが GitHub のランナー上で動作することを示します。Remote Script があなたの Live で読み込まれること、Live の API が偽物と同じ形をしていること、何かがどう聞こえるか、ターミナルやスクリーンリーダーが Kumi と一緒に動作することは示しません。本物の Live については、上記のオプトインのチェックと、[実装状況](IMPLEMENTATION_STATUS.md#エビデンス)にある記録がカバーしています。

## テストを書く

新しいプロトコルメソッドや Live への変更を追加するたびに、それが動作することのテストと、拒否すべきものを拒否することのテストを追加してください：古い参照、リビジョンとエポック、期限切れの確認、再利用された冪等性キー、タイムアウト、送信前と送信後のキャンセル、切断、失われた確認応答、部分的な変更、失敗した補償、その間に Live で行われた変更、そして取り消し。テストが何を主張するかについて、偽の Live、シミュレーター、本物の Live を区別してください（`fake-live`、`simulator`、`real-live` の来歴）。フィクスチャは小さく、個人データを含まないようにし、テストが本物の Live のフォルダーや `~/.kumi` に決して届かないようにしてください。
