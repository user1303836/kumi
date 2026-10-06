# 開発者ガイド

[English](../en/DEVELOPER_GUIDE.md) · [简体中文](../zh-CN/DEVELOPER_GUIDE.md) · 日本語

リポジトリの各部分がどうつながっているか、それぞれの部分でどう作業するか、どうリリースするかを説明します。テストのコマンドと CI が実行する内容はすべて[テスト](TESTING.md)にあります。

## 構成

| フォルダー | 内容 |
| --- | --- |
| `crates/kumi` | ネイティブの `kumi` コマンドとターミナルアプリ（`src/tui/`）、そしてモデルや実際の Live に対して任意で実行するチェック（`examples/`） |
| `crates/kumi-runtime` | エージェントループ、プロバイダー、サインイン、セッション、ライブラリー、Live との連携、メディアのツール、MCP クライアント |
| `crates/kumi-common` | ランタイムの共通処理と、ソース互換の値の扱い |
| `crates/ableton-mcp-server` | ネイティブの MCP ブリッジ、解析ワーカー、ライフサイクル、セットアップ、移行、診断 |
| `remote-script` | Live の中で動く、ブリッジの Remote Script（`ableton_mcp_remote_script.py`、`AbletonMcpBridge/` エントリーポイント、Python のテスト） |
| `apps/live-extension` | Live 12.4 以降向けの Kumi の Live 拡張機能。Live の Extensions SDK の上に作られています（TypeScript で、独自の `package.json` を持ちます） |
| `protocol` | `ableton-live-v1.operations.json`。ブリッジと Remote Script が共有する操作のレジストリ |
| `scripts` | リリースのビルダー、移行用のパッケージング、Mac のヘルパーのビルド、分離テストランナー、そして `npm run kumi` と `npm run setup` が使う `native-kumi.mjs` |
| `install.sh`、`install.ps1` | インストーラー |

4 つのクレートはルートの Cargo ワークスペースを共有します。npm のワークスペースはありません。ブリッジは Kumi なしでも動きます。クレートの移植元である TypeScript 実装は、git タグ `v1.7.6` に残っています。

## 各部分のやりとり

```text
kumi (crates/kumi, crates/kumi-runtime)
  │  MCP over stdio: Kumi starts the bridge as a child process
  ▼
bridge (crates/ableton-mcp-server)
  │  ableton-loopback/v1: authenticated TCP on 127.0.0.1
  ├──► Remote Script inside Live (remote-script/)
  │  local channel to the Extension Host
  └──► Kumi's Live extension (apps/live-extension), Live 12.4+
```

- Kumi は、`full` デプロイメントポリシーと、使うツールだけをちょうど並べた許可リスト（`ABLETON_MCP_TOOL_ALLOW`）を指定してブリッジを起動します。モデルはいくつかのブリッジの読み取りを直接呼び出し（`crates/kumi-runtime/src/mcp/allowed_tools.rs` の `MODEL_TOOLS`）、残りは Kumi 自身のツールが呼び出します。
- ブリッジのルーター（`crates/ableton-mcp-server/src/bridge/router.rs`）は、各操作を Remote Script に送るか、その操作を拡張機能しか持っていない場合は拡張機能に送ります。Live の Developer Mode のせいで拡張機能が起動されない場合、ブリッジは Live の Extension Host を自分で起動できます。
- `kumi bridge` はブリッジのライフサイクルコマンドを通じてブリッジを Live にインストールし、その後は `Remote Scripts/AbletonMcpBridge/bridge-reference.json` を通じてブリッジを見つけます。

## セットアップ

Rust、Cargo、Python 3.11 以降、git を用意して：

```sh
cargo build --release --locked --workspace --bins
cargo run --release -p kumi --
cargo run --release -p kumi -- bridge --allow-dirty   # Live を閉じた状態で
```

`npm run setup` と `npm run kumi -- ...` も引き続き使えます。Cargo がインストールされていればこのチェックアウトを Cargo でビルドし、なければ対応する公開済みのネイティブ版に引き継ぎます。Node.js が必要なのは、これらのコマンド、Live 拡張機能、一部のテストで、ネイティブのアプリケーションには必要ありません。

チェックアウトは、インストール済みの Kumi と `~/.kumi`（設定、サインイン、会話、ブリッジの状態）を共有します。`--allow-dirty` を付けると、コミットしていない変更のあるチェックアウトから `kumi bridge` がブリッジをインストールできます。Windows で開発者モードも管理者権限のシェルもない場合、シンボリックリンクを作るテストはスキップされるか失敗します。CI のランナーではシンボリックリンクを作れます。

## ビルド、テスト、計測

ネイティブのチェックはリポジトリのルートから実行します。分離ランナーは、一時的なホームで `cargo test --workspace --locked` を実行します：

```sh
npm ci --prefix crates/kumi-runtime/tests/support   # 初回のみ：一部のテストが使う公式 SDK
cargo build --locked --workspace --all-targets
sh scripts/test-isolated.sh                 # PowerShell: ./scripts/test-isolated.ps1
sh scripts/test-isolated.sh -p kumi-runtime --test hands_transport
python3 -m unittest discover -s scripts/tests -p 'test_*release.py'
```

性能の基準は、最適化したバイナリで、カバレッジやほかの重いビルドとは別に実行してください。ベンチマークを始める前に、隣接する解析ワーカーをビルドします：

```sh
cargo build --locked --release -p ableton-mcp-server --bins
cargo run --locked --release -p ableton-mcp-server --bin ableton-mcp-benchmark
```

ベンチマークは JSON の測定結果を出力し、基準を満たさないと 0 以外のコードで終了します。メモリーの欄は追跡した Rust の割り当て量で、V8 のヒープ、external、ArrayBuffer を分けて測ったものではありません。デバッグビルドの時間は、リリースの性能の根拠にはなりません。

そのほかのテストと、モデルや本物の Live に対するオプトインのチェックについては、[テスト](TESTING.md)を参照してください。

## Kumi の開発

**変更ファミリー**（独自の HISTORY の行と取り消しを持つ、変更の種類）は、`CHANGES`（`crates/kumi-runtime/src/integrations/ableton/changes.rs`）のエントリーです。メタデータは `assets/changes.json` から読み込み、要約は `changes/summaries.rs` と `changes/more_summaries.rs` にあります。エントリーには、Kumi のツール、ブリッジのプレビューと適用、ファミリー（HISTORY と NOW が描く、決まった絵柄のうちの一つ）、モデル向けの説明、そしてわかりやすい言葉の要約が入ります。古いブリッジが拒否する場合は `since`（それが動作する最初のブリッジのリリース。`bridge_version.rs` にあります）を、Live に元に戻す手段がない場合は `permanent` を付けます。テストはすべてのファミリーについて、ツールが一意であること、素のプレビューからタイトルが作れること、説明がモデルに確認を求めないこと、ブリッジのツールがホスト専用であることを確認します。提供する前に、実際の Live で取り消しも含めて実行してください（`accept_live` の example）。

**アクション**（Set への変更ではなく、取り消すものがないもの。再生など）は、`actions.rs` の `ACTIONS` に入れ、メタデータは `assets/actions.json` に置きます。

変更の評価（`eval_changes` の example）は、ブリッジのツールスキーマをそのカタログから読み込むので、スキーマが変わっても作り直すものはありません。

ターミナルアプリの設計と基盤については、[コマンド、キー、画面](KUMI_TUI.md#設計メモ)にあります。

## ブリッジの開発

以下のパスは `crates/ableton-mcp-server` からの相対パスです。

| パス | 内容 |
| --- | --- |
| `src/host.rs`、`src/host/` | MCP のディスパッチ、厳密なツールスキーマ、トランザクション、取り消しと回復 |
| `src/tool_catalog.rs` | 唯一のツールカタログ：スキーマ、アノテーション、各ツールに必要な機能、デプロイメントポリシークラス |
| `src/live.rs`、`src/registry.rs` | Live の型とアダプター。レジストリとそのハッシュの読み込みと検証 |
| `src/bridge/` | 認証付きループバッククライアント（`remote_adapter.rs`）、ルーター、拡張機能のチャネル、ランチャー、フォルダー |
| `src/transactions/` | バッチ、デバイスの状態、Session MIDI、ディスカバリのヘルパー |
| `src/mcp_protocol.rs`、`src/stdio.rs` | 両方のプロトコルバージョンの MCP ワイヤー処理 |
| `src/analysis*.rs`、`src/audio_*.rs`、`src/reference_analysis.rs` | 隔離されたワーカーでのオーディオ解析 |
| `src/delivery*.rs`、`src/lifecycle*.rs`、`src/setup.rs`、`src/migrate.rs`、`src/diagnostics.rs` | 設定、シークレット、インストール、アップグレード、ロールバック、診断 |
| `src/als.rs`、`src/project*.rs`、`src/library_search.rs` | 保存された Set、Set のスナップショットと差分、Live のライブラリデータベース |
| `src/follow_actions.rs` | オプションの [Willington](WILLINGTON_INTEGRATION.md) の Follow Actions |

**契約ルール。**

- ワイヤープロトコルは `ableton-loopback/v1` です。正規 JSON（キーはソート、負のゼロは正規化）、リクエストとレスポンスの HMAC-SHA256、上限のあるフレームとコレクション、シーケンス番号、そして Remote Script が起動するたびに変わるエポックを使います。詳細は `remote-script/README.md` にあります。
- `protocol/` のレジストリが唯一の操作一覧です。ホストと Remote Script はそれぞれこれをハッシュし、両者が一致しなければ Live は接続しません。ホストのテストが Remote Script のハッシュ処理を実行して、両者が等しいことを保ちます。操作名やハッシュをほかのソースファイルに決してコピーしないでください。
- 変更は、プレビュー、適用、取り消しを持つ用途別の操作を通じて行います。唯一の例外は `python.run`（`live_run_python`）で、Live のメインスレッドで Python を実行し、元に戻す手段は Live の取り消しだけです。これには独自のポリシークラス `python` があり、`full` プロファイルでのみ許可されます。
- ブリッジ内部の `get(ref)` は固定の行に対する上限付きのシリアライザーであり、Live のオブジェクトモデルの汎用リーダーではありません。MCP の読み取りは用途別のままにします。
- Remote Script は Live に関わる作業をすべて Live のメインスレッドで行います。Live の表示ティックの中で（ティックの合間は Live 自身のタイマーで）、時間の上限内に、ソケットを自分で処理します。ほかのスレッドは、診断ファイルの書き込みとリアルタイム UDP の受信だけです。新しいエポックは、それ以前のすべての参照とカーソルを無効にします。Remote Script が認識できない Live の形状は利用不可として報告し、決して偽装しません。
- stdout には MCP プロトコルだけを流します。診断は stderr に出し、リクエストのデータは含めません。
- プロセスを使う操作は `AsyncLiveAdapter` を使います。シミュレーターには同期メソッドも残っています。互換性の作業は両方の経路に対してテストしてください。
- テストは、実行中の Live、デバイス、特定のマシン、ローカルにしかない資料を決して必要としません。新しい操作にはすべて、不正な入力と回復を含めてテストを付けます。

**MCP のバージョン。** ブリッジは `2025-11-25`（initialize の後にリクエスト）と `2026-07-28`（リクエストごとの `params._meta` にプロトコルバージョンとクライアントの機能、さらに `server/discover`）の両方に対応します。一つのプロセスはどちらか一方を使います。未知のバージョンには `-32022`、不正なメタデータには `-32602` を返します。新しいバージョンの結果は `resultType: "complete"` を持ち、JSON を `structuredContent` でも返します。新しいバージョンにはプッシュがありません。`live_subscribe` は古いバージョンでのみ動き、新しいクライアントはポーリングします。クライアントのメタデータが Live へのアクセスを与えることは決してありません。[仕様](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning)を参照してください。

## Live 拡張機能の開発

`apps/live-extension` は TypeScript で書かれ、esbuild でバンドルされ、独自の `package.json` を持ちます。Live の Extensions SDK に対してビルドしますが、この SDK のライセンスは再配布を禁じているため、リポジトリには含まれていません。リポジトリのルートの `vendor/ableton-extensions-sdk-1.0.0-beta.1/` にコピーを置いてください（ビルドはその中の `package 3/dist/index.cjs` を読みます）。`apps/live-extension` で `npm ci` を実行してから `npm run build` を実行すると、`dist/extension.js` とその `.sha256` が書き出されるので、両方をコミットします。`npm run typecheck` にも SDK が必要です。SDK がない場合、ビルドは止まり、コミット済みのバンドルがそのまま残ります。`npm test` はバンドルをそのチェックサムと照合します。Live が拡張機能をどう実行するかの測定結果は[エビデンス](../evidence/live-extension.md)にあります。

## Willington のファイル

[Willington](WILLINGTON_INTEGRATION.md) のリポジトリは非公開です。Kumi が収めているのはそのランタイムファイルだけで、`vendor/willington/` にあります。このフォルダーが変わるのは、Willington の更新（後述）のときだけです。ファイルの隣には、Willington のライセンス表示（`LICENSE` または `LICENSE.md`。Kumi の MIT ライセンスはこれらのファイルには及びません）と `release.json` があります。

```json
{"schema": "kumi-willington-vendor/v1", "version": "0.4.0", "commit": "<Willington の 40 文字のコミット>",
 "files": {"WillingtonRuntime/__init__.py": "<SHA-256>", "LICENSE": "<SHA-256>"}}
```

`files` には、自分自身を除くフォルダー内のすべてのファイルが、どのプラットフォームのチェックアウトでも扱える名前で並びます。使えるのは ASCII の英字、数字、`.`、`_`、`-` だけで、Windows のデバイス名や末尾のドットは使えず、大文字と小文字だけが違う 2 つの名前も使えません。`scripts/build-native-release.py` は、`release.json` に載っていないファイル、SHA-256 が違うファイル、Willington のランタイムファイルではないファイルがフォルダーにあると、リリースを拒否します。認められるのは `WillingtonRuntime`、`WillingtonBindings`、`WillingtonDeviceTools`、`WillingtonRackZones` の中の `.py`、`.json`、`.md`、`.pyd`、`.dylib` だけなので、ソース、ヘッダー、デバッグファイル、バイトコードのキャッシュが出荷されることはありません。`test_native_release.py` は CI でリポジトリのフォルダーに同じ確認を行い、`.gitattributes` はフォルダーをバイト単位でそのまま保ち、空白のチェックからも外します。

リリースは、これらのファイルをブリッジの Remote Script の中、`AbletonMcpBridge/willington/` に置きます。ネイティブライブラリは自分のプラットフォームのもの（Windows では `.pyd`、macOS では `.dylib`）だけで、合計 16 MiB までです。Linux のバンドルには入りません。ブリッジのインストールはそのフォルダーも一緒にコピーし、Python ファイルの隣に `__pycache__` のブロッカーを置くので、インストールされたツリーはインストールのレシートが記録したとおりに保たれます。インストールされたブリッジのうち 2 つのファイルは、リリースのものではなくプロデューサーのものです。`willington.json` と、Follow Action のセルフテストのレシート `willington/WillingtonBindings/self-test.json` です。これらはブリッジのインストールの確認に影響せず、インストールのときに引き継がれます。ブリッジがこのフォルダーを Python のパスに加えるのは、`willington.json` が Willington をオンにしたときだけで、ブリッジの隣にインストールされたコピーより後になります。

**Willington の更新。** Willington の Bundle ワークフローは、ネイティブライブラリをそれぞれの対象プラットフォームでビルドし、マトリックスバンドルを各実行のアーティファクトとして残します。更新には、Willington の `main` へのプッシュで実行されたものを使います。

1. 実行を見つけます：`gh run list -R xonedsp/willington -w Bundle -b main -e push -s success`。
2. `main` から作った `willington/<任意の名前>` という名前のブランチで、`python3 scripts/vendor-willington.py --run <run>` を実行します。実行が Willington の `main` へのプッシュで成功した Bundle の実行で、そのコミットが `main` にあること、アーティファクトが GitHub の記録したダイジェストと、バンドルがその SHA-256 と一致することを確認し、そのコミットの Willington のライセンスを取得します。新しいフォルダーがリリースの確認をパスしたときだけフォルダーを置き換え、コミット、実行、アーティファクトのダイジェストを表示します。
3. ほかに何も変えないプルリクエストを、表示された内容を書いて開きます。`Willington files` がパスするのは、このリポジトリの `willington/` ブランチから出たリポジトリ所有者のプルリクエストだけで、Installer は 6 つのプラットフォームすべてでビルドとインストールを行います。

レビューする人は `python3 scripts/vendor-willington.py --check <run>` で更新を確認できます。実行からフォルダーを作り直し、ファイルごとに比べます。

ライブラリは CI がビルドするので、ハッシュが Live で検証されたものと違うことがあります。更新は、それを出荷するリリースの前に Live で確認し、そのライブラリについて [Follow Action のセルフテスト](WILLINGTON_INTEGRATION.md#follow-action-のセルフテスト)を実行してください。

## リリース

**コミット**の件名は、プロデューサーにとって何が変わったかを平易な英語で書きます（"Kumi: talk to it while it works"）。ブリッジまたは Remote Script の変更では、`crates/ableton-mcp-server/Cargo.toml` と `Cargo.lock` のバージョンを上げ、件名を新しいバージョンで始め（"Bridge 1.0.71: …"）、`CHANGELOG.md` の `## Unreleased` の下に `### Bridge x.y.z` ブロックを追加します。拡張機能が変わったら、そのバンドルを再ビルドしてコミットします。作業はブランチで行い、プルリクエストで `main` に入れます。

**Kumi のリリース**は、`gh` にリポジトリの管理者としてサインインした状態で、どのチェックアウトからでも実行できる一つのコマンドです。プルリクエストがマージされたらすぐに実行します：

```sh
python3 scripts/release.py          # ドライラン：バージョン、CHANGELOG の項目、リリースノート、チェックを表示
python3 scripts/release.py --go     # リリース
```

- **出荷するもの**は、前回のタグ以降の `main` から読み取ります。マージされた各プルリクエストの `Changelog:` の行を、書かれたとおりに使います（"none" は何も加えません）。プルリクエストがブリッジを変え、Kumi のクレートやインストーラーを変えていない場合、その行はブリッジの見出しの下に入ります。両方を変えるプルリクエストでは、ブリッジについての行を `Changelog (bridge):` と書きます。ドライランは各行の入る場所を表示します。
- **ブリッジ**は、プルリクエストが Live の読み込むもの（`crates/ableton-mcp-server`、ホストがリンクする `crates/kumi-common`、`remote-script`、`protocol`、`apps/live-extension`、`vendor/willington`。テストと Markdown は除く）を変えたときに新しいバージョンになります。`Cargo.lock` にあるホストの依存関係が変わるとドライランがそう伝えるので、そのときは `--bridge` でブリッジを新しいバージョンにします。
- **バージョン**は次のパッチです。`--minor` か `--version X.Y.Z` で別のものを選べます。
- **`--go` を付けると**、`release/vX.Y.Z` に "Kumi X.Y.Z: the changelog, READMEs and versions" をコミットします。ルートの `package.json`、`crates/kumi-runtime/src/version.rs`、`kumi`、`kumi-common`、`kumi-runtime` の Cargo マニフェスト、`Cargo.lock` のバージョン（パッケージングのテストがこれらが等しいことを確認します）と、変わった場合はブリッジのバージョン、3 つの README の Status の行、3 つの `KUMI_CHANGES.md` の「Bridge versions」の下にある同梱するブリッジの行、`CHANGELOG.md` の `## X.Y.Z — date` の項目です。続いてプルリクエスト "Kumi X.Y.Z" を開き、その CI を待たずに管理者のバイパスで "Kumi X.Y.Z (#PR)" としてマージし、マージコミットに `vX.Y.Z` のタグを付けてプッシュし、リリースノート付きの下書きのリリースを作ります（`--summary` でノートの冒頭に一文を加えられます）。
- **タグの Installer の実行**が、macOS、Linux、Windows の Intel と ARM 向けのネイティブバンドルをビルドし、インストールと移行をテストし、ターゲットごとのバンドルとマニフェスト、互換用の `kumi.tar.gz`、`kumi-release.json`、`SHA256SUMS` を添付し、`release.py` がノートを書いた場合（ノートの最後に `<!-- kumi:release-notes -->` があります）はリリースを公開します。そうして初めて、インストーラー、`kumi update`、更新確認がそのリリースを認識します。手でプッシュした「ノートは後で」のタグなど、ほかの下書きは、だれかが公開するまで下書きのままです。

**ネイティブリリースのローカルでの準備**（コミット済みの変更がない状態で）：

```sh
python3 scripts/build-hands.py              # macOS のみ。ユニバーサルでアドホック署名のヘルパーを target/hands/ に作成
MACOSX_DEPLOYMENT_TARGET=13.0 python3 scripts/build-native-release.py --target aarch64-apple-darwin --out release/native/aarch64-apple-darwin
python3 -m unittest discover -s scripts/tests -p test_native_release.py
```

ホストの Rust のターゲットトリプルを使ってください。Apple Silicon の Mac では、`--target x86_64-apple-darwin` で Intel 向けのバンドルをクロスビルドできます。ビルダーはロックしたリリースビルドを実行し、ブリッジのアーティファクトを、コミット、Cargo のロックファイル、ビルドレシピ、正確なファイルハッシュに結び付けます。CI がプルリクエストと `main` で使う `--profile ci-release` は、ビルドを速くするために、リリースプロファイルのプログラム全体の最適化を省きます。公開するバンドルは常に既定の `release` を使います。Mac のバンドルには `target/hands/` にある最新のヘルパーが必要で、ほかのバンドルはそれを含みません。サーバーと解析ワーカーは必ず一緒に配布します。`--binaries-dir` は既存のバイナリをパッケージするだけで、それがリリース用の最適化でビルドされたことは保証しません。

集約の手順では、既存の Node 24 インストールが読むマニフェストを保ちます：

```sh
python3 scripts/build-migration-release.py release/native/*/kumi-release.json --node 24.21.0 --out release/installer
export KUMI_LEGACY_APP="$(python3 scripts/fetch-legacy-release.py)"
KUMI_NATIVE_RELEASES="$PWD/release/installer" python3 -m unittest discover -s scripts/tests -p test_migration_release.py
```

リリースのワークフローが選んだ実際の Node 24 のバージョンを使ってください。移行テストは、最後の JavaScript リリースである Kumi 1.7.5 を実行します。`fetch-legacy-release.py` はその公開済みのバンドルをダウンロードし、SHA-256 を確認して展開し、そのフォルダーを出力します。すべてのテストが実行されるよう、Node 24 で、両方の変数を設定して実行してください。

新規インストールはネイティブのターゲットを選び、Node をダウンロードしません。既存の Node 24 インストールは、`kumi update` の際に小さな互換起動処理（`scripts/migration/kumi.mjs`。どのバンドルにも `apps/kumi/bin/kumi.mjs` として入っています）を受け取り、それが自分のプラットフォーム用のネイティブバンドルだけをダウンロードして展開します。管理下の Node は、ロールバックと任意の YouTube チャレンジ処理用に残ります。Windows では、ネイティブ版の初回起動で古いランチャーを置き換えます。それを実行中の cmd は、新しいランチャーの詰め物の行から再開して終了します。古い Node メジャーでは、同じ `KUMI_HOME` でインストーラーの再実行が必要な場合があります。

**ブリッジ**には独自のリリースはありません。ブリッジは各 Kumi リリースに同梱されます。リリースに何が含まれ、どう確認されるかは[配布](DISTRIBUTION_POLICY.md)にあります。

## ドキュメント

`docs/en` のすべてのドキュメントには、日本語版と中国語版が `docs/ja` と `docs/zh-CN` にあり、README には `README.ja.md` と `README.zh-CN.md` があります。ある言語への変更は、同じプルリクエストで 3 言語すべてに入れます。ブリッジのドキュメント（その README と、`scripts/build-native-release.py` の `DOCUMENTS` に並んでいる `docs/en` の 14 ページ）はブリッジと一緒にパックされるので、名前は固定で、リンクは解決できなければなりません。パッケージングのテストがそれを確認します（[テスト](TESTING.md#ドキュメント)を参照）。

## 先行事例

ブリッジは、ほかの Ableton MCP サーバーを出発点にしています：[bschoepke/ableton-live-mcp](https://github.com/bschoepke/ableton-live-mcp)、[uisato/ableton-mcp-extended](https://github.com/uisato/ableton-mcp-extended)、[Simon-Kansara/ableton-live-mcp-server](https://github.com/Simon-Kansara/ableton-live-mcp-server)、[jasper-zheng/ableton-sdk-mcp](https://github.com/jasper-zheng/ableton-sdk-mcp)、[ahujasid/ableton-mcp](https://github.com/ahujasid/ableton-mcp)。
