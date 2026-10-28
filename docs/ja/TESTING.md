# テスト

[English](../en/TESTING.md) · [简体中文](../zh-CN/TESTING.md) · 日本語

リポジトリの各部分のテストの実行方法、それに必要なもの、CI が何を実行するかをまとめます。通常のテストには、Live もサインインも必要ありません。

## クイックスタート

リポジトリのルートから実行します：

```sh
npm ci --prefix crates/kumi-agent/tests/support   # 初回のみ：一部の Rust テストが使う公式 SDK
cargo build --workspace --all-targets --locked
sh scripts/test-isolated.sh                        # Windows: ./scripts/test-isolated.ps1
python3 -m unittest discover -s daws/ableton/remote-script -p 'test_*.py'
npm ci --prefix daws/ableton/live-extension
npm test --prefix daws/ableton/live-extension
python3 -m unittest discover -s scripts/tests -p 'test_*release.py'
```

必要なもの：

| 必要なもの | 用途 |
| --- | --- |
| Rust と Cargo | クレートと移行テスト |
| cargo-nextest（`cargo install cargo-nextest --locked`） | Rust のテスト（一つずつ別のプロセスで実行） |
| PATH 上の Node.js（CI は 24 を使用） | 一部の Rust テスト、Live 拡張機能のテスト、移行テスト |
| PATH 上の Python 3.11 以降（`python3`、Windows では `python.exe`） | Remote Script のテスト、リリーススクリプトのテスト、Python を実行する Rust テスト |
| `vendor/` にローカルで用意した Extensions SDK | Live 拡張機能のビルドや型チェック（そのテストには不要） |

Windows では、いくつかのテストがシンボリックリンクを作成するため、開発者モードか管理者アカウントが必要です。それがないと、一部のテストはスキップされ、いくつかは `EPERM` で失敗します。CI の Windows ランナーにはその権限があります。

## Kumi とブリッジ

リポジトリのルートから実行します。

| コマンド | 内容 |
| --- | --- |
| `sh scripts/test-isolated.sh`（Windows では `./scripts/test-isolated.ps1`） | テスト専用のホームで、すべてのテストを cargo-nextest で一つずつ別のプロセスで実行します。引数は `cargo nextest run` に渡します（`KUMI_NEXTEST=0` では `cargo test`。クレートのテストが一つのプロセスで動きます。`KUMI_NEXTEST_ARCHIVE` では別の場所でビルドしたテストを `cargo nextest archive` から使い、チェックアウトの `target/` に展開します） |
| `sh scripts/test-isolated.sh -p kumi-daw-ableton -E 'test(/^hands_transport::/)'` | 一つのクレートの、一つのテストファイルだけを実行します。クレートのテストは一つのバイナリ（`tests/it/`、ファイルごとに一つのモジュール）です |
| `cargo fmt --all --check` | CI と同じようにフォーマットを確認します |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Lint。CI では今のところ参考扱いです |
| `cargo clippy --workspace --lib --bins --examples --locked -- -A clippy::all -D clippy::await_holding_refcell_ref` | await をまたいで保持される `RefCell` の借用。Lint の CI ジョブを失敗させます |
| `sh scripts/test-isolated.sh --profile ci` | `Rust / Linux` の各パートが実行するテストを cargo-nextest で実行します（一つのパートのものは `-p <crate>` か `--exclude <crate>`、`--profile ci-os`：macOS と Windows が実行するもの、`--profile extended`：すべてのテスト） |
| `python3 scripts/dev.py check` | このブランチが `origin/main` から変えたものについて、Required CI のチェックを実行します（別の起点は `--base`）：変更の空白、すべてのファイルがクレートかエリアに属すること、クレートの層（`dev.py deps --check`：依存は下の層へ向かい、ある DAW のクレートは別の DAW のクレートを使わない）、続いて影響を受けるクレートの `cargo fmt --check`、`RefCell` の lint、`ci` テスト（cargo-nextest）、さらに変更があれば Remote Script、Live の拡張機能、Renoise のツールのテスト、そしてリリーススクリプトのテスト。`--dry-run` は何を実行するかだけを示します |
| `cargo run --locked --release -p ableton-mcp-server --bin ableton-mcp-benchmark` | ブリッジの性能の基準。[開発者ガイド](DEVELOPER_GUIDE.md#ビルドテスト計測)を参照してください |

分離ランナーはテスト専用のホームを用意します。`HOME`、`USERPROFILE`、`APPDATA`、`LOCALAPPDATA`、`XDG_CONFIG_HOME`、`KUMI_HOME` は新しい一時フォルダーの中を指し、`KUMI_REMOTE_SCRIPTS_DIR` と `KUMI_LIVE_EXTENSIONS_DIR` は取り除かれるので、どのテストもあなたの Live のフォルダーや `~/.kumi` には届きません。

公式の MCP とモデルの SDK を Kumi 自身のクライアントやプロバイダーと並べて実行するテストは、`npm ci --prefix crates/kumi-agent/tests/support` を実行するまで失敗します。ラウドネスとトゥルーピークのテストは、生成した音声に対する FFmpeg の `ebur128` の結果と解析を照らし合わせます。

`testdata/oracles/<クレート>/` にある JSON のオラクルファイル（そのクレートのテストが `kumi_testkit::oracle` で読みます）は、TypeScript 実装から記録したゴールデンファイルです。その実装と、ファイルを生成したスクリプトは git タグ `v1.7.6` に残っていますが、このツリーにはファイルを再生成するものがありません。軽量なワークツリー（`python3 scripts/dev.py worktree new <名前> --lean`）には、Extended のテストだけが読むもの（`testdata/oracles/extended-only.txt` の一覧）が含まれません。意図して動作を変えたときは、影響を受けるオラクルの項目をネイティブの出力から書き直します。失敗したアサーションが実際の出力を表示します。SHA-256 の項目には、その出力のハッシュを書きます。差分を確認し、ゴールデンファイルを変えたことと、その理由をコミットメッセージに書いてください。ホストのテストは、ゴールデンファイルを記録したときのブリッジのバージョン（`ORACLE_VERSION`）に固定して動くので、ブリッジのバージョンを上げてもゴールデンファイルは変わりません。

## Remote Script

リポジトリのルートから：

```sh
python3 -m unittest discover -s daws/ableton/remote-script -p 'test_*.py'
python3 -m compileall -q daws/ableton/remote-script/AbletonMcpBridge
```

テストは Remote Script を偽の Live オブジェクトに対して実行します：認証、シーケンス、メインスレッドのキュー、レジストリとそのハッシュ、ディスカバリー、トランザクション、キャプチャとリアルタイムの安全性、そしてオプションの Willington プロバイダーです。

## Kumi の Live 拡張機能

`daws/ableton/live-extension` には独自の `package.json` があります。`npm ci --prefix daws/ableton/live-extension` を実行した後、`npm test --prefix daws/ableton/live-extension` は、コミットされた `dist/extension.js` を偽の Live に対して読み込み、記録された sha256 と照合します。そこでのビルド（`npm run build`）と型チェック（`npm run typecheck`）には `vendor/` 内の Extensions SDK が必要です。それがなければ、コミットされたビルドがそのまま残ります。再ビルドした後は、`dist/extension.js` をその `.sha256` と一緒にコミットしてください。

## リリーススクリプト

`python3 -m unittest discover -s scripts/tests -p 'test_*release.py'` は、パッケージングと移行のテストを実行します。パッケージングのテストは、バンドルの内容とバージョン、そしてパッケージされたガイドのすべてのリンクが解決できることを確認します。移行テストは、最後の JavaScript リリースの更新処理をネイティブのバンドルに対して実行します。次の変数がない場合、それを必要とするテストはスキップされます：

| 変数 | 指すもの |
| --- | --- |
| `KUMI_LEGACY_APP` | 展開済みの Kumi 1.7.5。`python3 scripts/fetch-legacy-release.py [folder]` がダウンロードし、SHA-256 を確認して展開し、そのフォルダーを出力します |
| `KUMI_NATIVE_RELEASES` | ビルド済みのリリースアーティファクトのフォルダー（[リリース](DEVELOPER_GUIDE.md#リリース)を参照） |

`KUMI_LEGACY_APP` は、ブリッジ自身の移行テスト `daws/ableton/ableton-mcp-server/tests/it/lifecycle_migration.rs` にも使われます。このテストは変数がなくてもスキップされず、公開済みの Kumi 1.7.5 のバンドルをダウンロードして SHA-256 を確認します。変数を設定しておけば、バンドルを一度取得したあとはオフラインで実行できます。

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

このテストは、ブリッジと一緒に配布されるガイドのすべてのリンクを確認します。対象は、ブリッジの README と、`scripts/build-native-release.py` の `DOCUMENTS` に並んでいる `docs/en` の 14 ページです。3 言語の内容が一致しているかを確認するものはないので、手作業でそろえてください。リリーススクリプトのテスト（`test_release.py`）は、REAPER の拡張機能と Renoise のツールの ships-with の行が、各言語のドキュメント（`KUMI_GUIDE.md`）に一つずつあり、このチェックアウトのバージョンを指していることを確認します。

## CI

プルリクエストのたびに次のワークフローが実行され、CI、Willington files、Installer、Network は `main` へのプッシュのたびにも実行されます：

| ワークフロー | ジョブ | 実行内容 |
| --- | --- | --- |
| **CI** | `Rust / Linux`、`Rust / Linux (Kumi's crates)`、`Rust / Linux (bridges)` | `.config/nextest.toml` の `ci` セットを分離ランナーで、テスト時間がほぼ同じになるようクレートごとに三つのパートに分けて同時に実行します（アプリ、Kumi のほかのクレート、`ableton-mcp-server` と `daw-bridge` と `daw-mcp-server` と `reaper-kumi`）。どのランナーもすべてのテストバイナリをビルドすることはありません。Extended ワークフローが実行する長いテスト（REAPER と Renoise の流れ全体のシミュレーションを含み、設定ファイルに理由とともに列挙）以外のすべてのテストです。プルリクエストでは、各パートは変更が影響する自分のクレートだけをテストし（`scripts/dev.py` が `dev.py check` と同じように選びます）、該当するものがないパートは省かれます。30 秒を超えたテストは名前つきで失敗します。続いて `git diff --check`。各 Rust ジョブ（Extended のものも）は、選んだテストの一覧をアーティファクト（`nextest-list-*`）として残し、実行どうしでテストを比べられるようにします |
| | `Rust / macOS`、`Rust / Windows`、`Rust / Windows (Kumi's crates and bridges)` | `ci-os` セット：プラットフォームによって異なるテスト（インストール、起動、更新、ターミナル、stdin と Windows のコンソール、Hands、パス、ファイル、子プロセス、ストア）。ビルドするのもそれらだけです。プルリクエストでは、macOS は Linux のパートと同じように、そのうち変更が影響するパッケージだけをテストし、Required CI に数えられるのは macOS だけです。Windows は main へのプッシュごとに、テスト時間がほぼ同じ二つのパート（アプリのテストと、それ以外）に分けて同時に実行し、最も遅いテストは Extended に任せます。`full-tests` ラベルのついたプルリクエストでは、Extended がすべての Windows テストを実行します |
| | `Rust / Lint`（Ubuntu） | `cargo fmt --check`、サンプルのビルド、Clippy（参考扱い。ただし await をまたいで保持される `RefCell` の借用はジョブを失敗させます） |
| | `Python Remote Script / ubuntu-24.04`、`macos-15`、`windows-2025`（Python 3.11） | Remote Script のテストを実行し、パッケージをコンパイルします |
| | `Live extension`（Ubuntu、Node 24） | コミットされたビルドに対する、拡張機能のテスト |
| | `Release scripts`（Ubuntu） | この変更の空白のチェック、続いてパッケージングのテスト、リリーススクリプトのテスト、`scripts/check-installers.sh`（install.sh が POSIX sh であること、install.ps1 が構文解析でき ASCII であること）。`main` へのプッシュでは、`main` の raw のインストーラーと最新リリースのものも確認します |
| | `REAPER extension loads / macos-15`、`ubuntu-22.04`、`windows-2025` | 配布するとおりの REAPER の拡張機能：リリースビルドとプラットフォーム自身の strip、バンドルに入れてよいものかのリリーススクリプトによる確認（REAPER のエントリーポイント。macOS では署名と自身の名前、Windows では C ランタイムの組み込み、Linux では Kumi より新しい glibc を要しないこと）、システムのローダーでの読み込み、C++ シムを通じた REAPER からの呼び出し |
| | `Renoise tool`（Ubuntu） | 固定した LuaJIT でのツールの Lua テスト（Renoise の API のモックに対して）と、Renoise の API 6.2 の定義に対する `lua-language-server --check` |
| | `Required CI` | 上記がすべてパスした場合にだけパスします |
| **Extended** | `Extended / Linux / build`、`(1/3)` から `(3/3)`、`Extended / macOS / build`、`(1/2)`、`(2/2)`、`Extended / Windows / build`、`(1/6)` から `(6/6)`、`Extended / documentation and eval runner tests` | 各システムのテストを一度だけビルドしてアーカイブ（`cargo nextest archive`）し、そのアーカイブから分割して実行します。各分割は自分のシステムのビルドが終わるとすぐに始まります（`extended-system.yml`）。公式 SDK をインストールした状態での全テスト（`extended` セット）：`ci` が省くもの（記録されたソースのオラクル、長いリスニング・フォーム・レンダリングのシミュレーション、ffmpeg を使うビデオ、ベンチマーク、公式の MCP とモデルの SDK と並べて実行する相互運用テスト、Kumi 1.7.5 からの移行、タイミングのビルディングブロックの長いスイープ）と、各プラットフォームの全テスト。続いてドキュメントテスト。毎週、手動で、また `full-tests` ラベルの付いたプルリクエストで実行され、何もブロックしません |
| | `Extended / REAPER SDK, newest header` | Cockos が公開している最新の SDK ヘッダーで REAPER の拡張機能をビルドし、生成した関数テーブルを確認し、それに移ると増える関数を一覧します |
| **Network** | `Network / What changed`、続いて `Network / Linux`、`macOS`、`Windows` | 各システム自身の信頼ストアに入れた認証局と、システム自身の設定にあるプロキシを通した、Kumi の HTTP クライアント（`network.rs` の ignored のテスト）。各システムのジョブは、クライアントの作り方を変える変更（`scripts/areas.toml` の `network` エリア）のときだけ実行されます |
| **Willington files** | `Willington files`（Ubuntu） | `vendor/willington/` を変えられるのは、このリポジトリの `willington/` ブランチから出たリポジトリ所有者のプルリクエストだけで、そのプルリクエストはほかに何も変えません。`main` にあるこのチェックが、プルリクエストのコードではなくファイルの一覧を読んで実行されます |
| **Installer** | `Native bundle / <target>`、`Aggregate native and existing-installer releases`、続いて `Install / <system>` と `Existing installer transition / <system>`。`Installer edges / <system>`（3 つ）はバンドルを必要としません | `main` へのプッシュと、インストールを変えるプルリクエストでは、x64 の Linux と Windows、Apple Silicon の macOS のバンドルをビルドし、それぞれで以下のとおりインストール、確認、アンインストールします。タグ、手動の実行、`full-tests` ラベルの付いたプルリクエストではすべてを行います：macOS、Linux、Windows の Intel と ARM 向けのネイティブバンドル（Mac のバンドルには、Kumi が Live のメニューを使うためのヘルパー（ユニバーサル、アドホック署名）が入り、Intel 向けは Apple Silicon でクロスビルドします）、続いて既存のインストールが更新に使う互換リリースをビルドし、インストーラーと一緒に、GitHub がリリースのファイルを配信するのと同じ形でローカルで配信します。各システムで：そこからワンライナーでプロデューサーと同じ方法でインストールし（Windows では、非 ASCII 文字の名前のフォルダーにある `KUMI_HOME` で、Windows PowerShell 5.1 を使います）、バージョン、`doctor`、ブリッジとその解析ワーカーを確認し、修復として再インストールし、使い捨ての Remote Scripts フォルダーに `kumi bridge --yes` を実行し、プロデューサー自身のファイルをそれぞれ置いた使い捨ての DAW フォルダーに REAPER の拡張機能と Renoise のツールを入れ（`kumi --daw reaper|renoise bridge --yes`）、それぞれがバンドルのコピーで、リファレンス、レシート、残した世代があることを確かめ、拡張機能を REAPER のエントリーポイントで読み込み、それらのための Kumi のホストを起動します。続いて `kumi update`（両方のブリッジがそのまま）、`kumi update --rollback` を 2 回（両方を前の Kumi のコピーへ、そしてまた戻す）、`kumi uninstall`（両方を取り除き、プロデューサーが変えたツールのコピーを残し、プロデューサー自身のファイルはそのまま）を実行します。続いて各システムの 1 つのインストール（macOS arm64、Linux x64、Windows x64）が、空白と非 ASCII 文字を含むホームで次を行います：このビルドのインストーラーで GitHub の最新リリースを、続いて `KUMI_VERSION` でこのビルドの 1 つ前のリリースをインストールし、そのリリースのブリッジを入れ、このビルドへの更新と 2 回のロールバックのたびに Kumi とブリッジを確認し（REAPER と Renoise のブリッジも入れた状態で。そのリリースはどちらも知らず、そのままにします）、アンインストールします。PATH の行は macOS では zsh の `ZDOTDIR` に、Linux では fish の `conf.d` に入り、そのシェルが `kumi` を見つけ、アンインストールで取り除かれます。Windows ではこれを PowerShell 7 で実行します。このビルドのインストール、ブリッジ、ロールバック、アンインストールは非 ASCII 文字の名前のフォルダーにある `KUMI_HOME` で、1 つ前のリリースからの更新は空白だけを含むホームで行います（そのリリース自身の更新機能が展開するため）。移行のジョブは、Kumi 1.7.5 と新しいバンドルで移行テストを実行します。edges のジョブは、install.sh と install.ps1 を代わりのリリースに対して実行します：リリースがない、応答がない、サインインページが返る、チェックサムが合わない、Kumi が起動しない、Ctrl-C。`v*` タグのプッシュでは、これらがすべてパスした後に `publish` がバンドルとインストーラーをリリースに添付し、`scripts/release.py` がノートを書いていればリリースを公開します。 |

プルリクエストでは、実行される範囲が狭くなります：

- `Python Remote Script` は macOS を省きます。
- Rust のビルドキャッシュは `main` からだけ保存されます。プルリクエストは `main` のものを復元し、何も保存しません。
- それぞれの世界のジョブは、`scripts/dev.py` がクレートと `scripts/areas.toml` から読み取るとおり、テストする対象が変わったときに実行されます（`python3 scripts/dev.py affected --base origin/main` で、ブランチが何を実行するかがわかります）。Rust のジョブは Renoise のツールの Lua だけの変更では省かれ、`REAPER extension loads` は拡張機能のクレート、それがビルドに使うもの（`daw-bridge`、ロックファイル）、リリーススクリプトの変更で、`Renoise tool` は `daws/renoise` の変更で実行されます。`Release scripts` は、追跡されているすべてのファイルがクレートかエリアに属することを確認します。
- Installer が各システムの 1 台でビルドしてインストールするのは、インストールに関わる部分を変える変更の場合だけです：インストーラー、インストール、更新、リリース、Willington のファイル、Mac のヘルパー、Kumi が使う `tar` と PowerShell、ダウンロードの方法（HTTP クライアントとネットワークの設定）とパスの組み立て、Kumi の拡張機能を Live に入れる処理、REAPER と Renoise のブリッジのビルドと配置、そして Installer のワークフロー（それぞれのテストは含まない）。それ以外のプルリクエストでは何もビルドせず、マージ後の `main` の実行で確認します。6 つすべて、GitHub のリリースからの更新、edges、移行は `full-tests` ラベルで実行されます。
- プルリクエストと `main` へのプッシュでは、Installer のバンドルを `ci-release` プロファイルでビルドします。これはリリースプロファイルのプログラム全体の最適化を省き、最適化も控えめにします。タグは `release` でビルドします。

`main` へのプッシュでは、インストールを変えるプルリクエストと同じものが実行され、タグでは Installer のすべてが実行されます。Extended ワークフローは毎週すべてのテストを実行します。Installer は毎晩 `release` プロファイルで 6 つのバンドルをすべてビルドし（失敗すると issue「The nightly Installer build failed」を開きます）、次のタグのためにそのプロファイルのビルドキャッシュを温めておきます。Caches ワークフローは毎日、Rust のビルドキャッシュを種類ごとに最新のものだけ残して削除します。古いロックファイルの世代が、プルリクエストの復元するキャッシュをリポジトリの 10 GB から押し出さないようにするためです。

`main` にマージするには、`Required CI` と `Willington files` がパスする必要があります。Installer は必須ではありません。残りのルールは[リリースと配布](DISTRIBUTION_POLICY.md#マージゲート)にあります。

## パスが意味すること

パスは、コードがテストに書かれたとおりに動作すること、パッケージが macOS、Linux、Windows でインストールでき動作すること、インストーラーが GitHub のランナー上で動作することを示します。Remote Script があなたの Live で読み込まれること、Live の API が偽物と同じ形をしていること、何かがどう聞こえるか、ターミナルやスクリーンリーダーが Kumi と一緒に動作することは示しません。本物の Live については、上記のオプトインのチェックと、[実装状況](IMPLEMENTATION_STATUS.md#エビデンス)にある記録がカバーしています。

## テストを書く

新しいプロトコルメソッドや Live への変更を追加するたびに、それが動作することのテストと、拒否すべきものを拒否することのテストを追加してください：古い参照、リビジョンとエポック、期限切れの確認、再利用された冪等性キー、タイムアウト、送信前と送信後のキャンセル、切断、失われた確認応答、部分的な変更、失敗した補償、その間に Live で行われた変更、そして取り消し。テストが何を主張するかについて、偽の Live、シミュレーター、本物の Live を区別してください（`fake-live`、`simulator`、`real-live` の来歴）。フィクスチャは小さく、個人データを含まないようにし、テストが本物の Live のフォルダーや `~/.kumi` に決して届かないようにしてください。
