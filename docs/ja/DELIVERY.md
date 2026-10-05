# ブリッジのインストール

[English](../en/DELIVERY.md) · [简体中文](../zh-CN/DELIVERY.md) · 日本語

ブリッジは二つの部分からなります。Live が読み込む Remote Script `AbletonMcpBridge` と、Kumi（またはほかの MCP クライアント）が起動するローカルの MCP サーバーです。どちらも一つのパッケージ `@ableton-mcp/mcp-server` に入っていて、一つのツールでインストールします。それがブリッジのライフサイクル CLI、`ableton-mcp-server lifecycle` です。このツールは何かを変更する前に計画を立て、インストールしたものをレシートに記録し、まさにそれを修復、ロールバック、削除できます。Kumi を使う場合は、`kumi bridge` がこれを実行します。

## Kumi で使う

Live を終了してから `kumi bridge` を実行します。このコマンドは次のことを行います：

1. Live の実行中は拒否し、Live が閉じていることの確認を求めます（`--yes` で事前に確認できます）。
2. ブリッジのパッケージを Kumi のバンドルから専用のフォルダーにコピーし（チェックアウトでは代わりに `scripts/build-native-release.py --bridge-only` でパックします）、そのハッシュを確認します。
3. ライフサイクルの `install`（ブリッジがすでにある場合は `upgrade`）を実行します。まず計画、次に変更です。
4. Kumi の Live 拡張機能を Live の Extensions フォルダーに置きます。Live 12.4 以降はこれを実行します。
5. Live が新しいブリッジを通じて接続するのを最大 10 分間待ち、その間、数秒ごとにライフサイクルの `activate` を実行します。

そのあと初めて Live を開いたら、**Settings → Link, Tempo & MIDI** で **AbletonMcpBridge** を Control Surface として選びます。コミットされていない変更があるチェックアウトでは、`kumi bridge --allow-dirty` でそれでもインストールできます（開発者向け）。

`kumi update` は、ブリッジの更新が必要で Live が閉じているときに、`kumi bridge` を実行します。ネイティブ版のアプリは初回起動時に、同じブリッジバージョンの旧 JavaScript 版ブリッジも移行します。レシートに結び付いた設定、シークレット、ポートはそのまま保たれます。`kumi uninstall` は、ライフサイクルの `uninstall` を通じてブリッジと拡張機能を Live から取り除くかどうかを尋ね、Live がまだブリッジのファイルを読み込んでいる間はそれを残します。`kumi doctor` はつながり全体を確認します。プロデューサー側から見た説明は [Kumi ガイド](KUMI_GUIDE.md#live-につなぐ)にあります。

| 対象 | 場所 |
| --- | --- |
| ブリッジのパッケージ | `~/.kumi/bridge/<version>-<time>/package`（以前の Node 世代は、元の `node_modules` の配置のままです） |
| その状態：シークレット、設定、レシート、ジャーナル | `~/.kumi/bridge/state`、または既存の所有者のレシートにある状態フォルダー |
| Remote Script | User Library の Remote Scripts フォルダー内の `AbletonMcpBridge`（[Live のフォルダー](#live-のフォルダー)を参照） |
| Kumi の Live 拡張機能 | Live の Extensions フォルダー内の `kumi.kumi` |

`KUMI_REMOTE_SCRIPTS_DIR` と `KUMI_LIVE_EXTENSIONS_DIR` は二つの Live のフォルダーを上書きし、`KUMI_HOME` は `~/.kumi` の場所を移し、`KUMI_BRIDGE_WAIT_SECONDS` は Live を待つ時間を設定します（`0` で待ちません）。Kumi は、ライフサイクルが Remote Script の隣に書き込む `bridge-reference.json` を通じて、インストールされたブリッジを見つけます。

## スタンドアロンのブリッジ

Kumi 以外の MCP クライアントには、自分のプラットフォーム向けのネイティブのアーカイブを使います。Node のランタイムを別に用意する必要はありません。クリーンなチェックアウトから次のようにビルドします：

```sh
python3 scripts/build-native-release.py --bridge-only --out release/bridge
```

出力には、ターゲット別の `.tar.gz` と、その正確な SHA-256 を記した `prepared.json` が含まれます。コミットされていない変更からビルドしたパッケージには `--allow-dirty-private-build` が必要です。

アーカイブを、ずっと置いておくディレクトリーに展開します。`ableton-mcp-server` と `ableton-mcp-analysis-worker` は、パッケージのアセットとリリースマニフェストと一緒に置いたままにしてください。macOS での例：

```sh
ARTIFACT=/absolute/path/to/ableton-mcp-server-1.0.74-aarch64-apple-darwin.tar.gz
ARTIFACT_SHA="$(shasum -a 256 "$ARTIFACT" | awk '{print $1}')"
INSTALL_ROOT="$HOME/Library/Application Support/AbletonMcp/package"
STATE="$HOME/Library/Application Support/AbletonMcp/state"
REMOTE_SCRIPTS="$HOME/Music/Ableton/User Library/Remote Scripts"
mkdir -p "$INSTALL_ROOT" "$REMOTE_SCRIPTS"
tar -xzf "$ARTIFACT" -C "$INSTALL_ROOT"
PACKAGE_ROOT="$INSTALL_ROOT/package"
SERVER="$PACKAGE_ROOT/ableton-mcp-server"

"$SERVER" lifecycle install --remote-scripts-dir "$REMOTE_SCRIPTS" --state-dir "$STATE" \
  --package-root "$PACKAGE_ROOT" --artifact "$ARTIFACT" --artifact-sha256 "$ARTIFACT_SHA"
# 計画を読み、Live を終了してから：
"$SERVER" lifecycle install --remote-scripts-dir "$REMOTE_SCRIPTS" --state-dir "$STATE" \
  --package-root "$PACKAGE_ROOT" --artifact "$ARTIFACT" --artifact-sha256 "$ARTIFACT_SHA" \
  --apply --confirm-live-stopped
```

Windows では、対応するアーカイブを `tar -xzf` で展開してから、同じオプションと Windows の絶対パスを使って `& "$PackageRoot\ableton-mcp-server.exe" lifecycle install` を実行します。アーカイブのハッシュは `Get-FileHash -Algorithm SHA256 $Artifact` で得られます。User Library の Remote Scripts フォルダーを使ってください（[Live のフォルダー](#live-のフォルダー)を参照）。

Live を開き、**AbletonMcpBridge** を Control Surface として選んでから、同じ `--remote-scripts-dir`、`--state-dir`、`--package-root` を付けて `activate` を実行します。MCP クライアントには、インストールしたサーバーを `--config <state>/bridge-config.json` で指定します。[ユーザーガイド](USER_GUIDE.md)を参照してください。

アップグレードするには、新しい tarball を新しいディレクトリーに展開し、そのパッケージのルート、アーティファクト、ハッシュを指定して `upgrade` を実行します。既存の状態、設定、シークレットのパスはそのまま使います。確認済みの旧 Node のインストールは、同じブリッジバージョンのネイティブパッケージに移行できます。それ以外のアップグレードでは、バージョンが上がる必要があります。ロールバックのため、前のパッケージは残しておいてください。パッケージのディレクトリーを削除する前に `uninstall` を実行します。ライフサイクルは旧 Node のリリースのアーティファクトとレシートも引き続き受け付けますが、それらには Node 22 または 24 が必要です。

## ライフサイクル CLI リファレンス

```text
ableton-mcp-server lifecycle <action> --remote-scripts-dir DIR [options]
```

| アクション | 内容 | 必要なもの |
| --- | --- | --- |
| `install` | 所有者だけがアクセスできるシークレットとブリッジ設定を作成し、Remote Script をインストールし、レシートを書き込みます | `--artifact`、`--artifact-sha256`。Live が停止していること |
| `activate` | Live にもインストールにも変更を加えずに、Live がこのブリッジを読み込み、ブリッジを通じて応答することを確認します。結果をレシートに記録します | Live が実行中で、Control Surface が選ばれていること |
| `upgrade` | ブリッジを新しいパッケージに置き換えます。シークレットと、`rollback` 用に前のバージョンを残します | より新しいアーティファクト、または確認済みの同じバージョンでの Node からネイティブへの移行。その SHA-256 とパッケージのルート。Live が停止していること |
| `repair` | インストールされているものをレシートと比べます。`--apply` を付けると、変更されたファイルを隔離場所に移し、パッケージ本来のファイルを復元します | — |
| `rollback` | 最後のアップグレードが残したバージョンに戻します | Live が停止していること |
| `uninstall` | レシートが所有するファイルを削除します。変更されたファイルや不明なファイルは隔離場所に移します。シークレットは残します | Live が停止していること |
| `status` | 読み取り専用のレポート：レシート、ファイルの完全性、ドリフト、権限、ロールバックが可能かどうか | — |

| オプション | 意味 |
| --- | --- |
| `--remote-scripts-dir DIR` | Live の Remote Scripts フォルダー（必須） |
| `--state-dir DIR` | シークレット、設定、レシート、ジャーナルを置く場所。デフォルトは `~/.config/ableton-mcp`、Windows では `%APPDATA%\ableton-mcp` |
| `--package-root DIR` | 使用するインストール済みのパッケージ。デフォルト：この CLI が属するパッケージ |
| `--artifact FILE`、`--artifact-sha256 HEX` | tarball とそのハッシュ。ライフサイクルは、インストールされたパッケージを tarball 自身のマニフェストと照合します |
| `--config FILE`、`--secret FILE` | 設定とシークレットの別のパス（デフォルト：状態フォルダー内の `bridge-config.json` と `bridge.secret`） |
| `--host`、`--port`、`--realtime-port` | 新しいインストールのループバックアドレスとポート。デフォルトは `127.0.0.1`（または `::1`）、9765、9766 |
| `--timeout-ms N` | 設定に書き込まれる、ブリッジのリクエストのタイムアウト（デフォルト 5000） |
| `--apply` | 変更を実行します。これがなければ、どのアクションも計画を立てるだけです |
| `--confirm-live-stopped` | Live を終了したことの確認。`--apply` を付けた `install`、`upgrade`、`rollback`、`uninstall` で必要です |
| `--purge-secret` | `uninstall` と一緒に使い、シークレットも削除します。ライフサイクルが作成したシークレットの場合だけです |
| `--enable-bridge-diagnostics` | `install` と一緒に使い、Remote Script の診断ログを有効にします |
| `--allow-dirty-private-build` | コミットされていない変更からビルドしたパッケージを受け入れます（開発者向け） |

各実行は stdout に JSON の結果を一つ出力します（`ableton-mcp-lifecycle/v1`）。その `state` は `planned`、`completed`、`activation-required`、`blocked`、`failed` のいずれかです。拒否された場合は、代わりに stderr に `ableton-mcp-lifecycle-error/v1` を、パスを取り除いて出力します。blocked、failed、拒否された実行は終了コード 2 で終わります。レシートのステータスは、install、upgrade、repair、rollback の後は `installed-restart-required`、`activate` がブリッジを通じて Live に到達した後は `activated`、削除の後は `uninstalled` になります。

ライフサイクルは、Live を終了したり起動したりせず、Control Surface を選ばず、Live のフォルダーを推測せず、与えられたパス内のシンボリックリンクやジャンクションをたどりません。作業中はロックを保持し、最後の変更のジャーナルを残します。途中で失敗した場合は、元の状態に戻します。実行が中断されたら、再試行する前に `status` とジャーナルを読み、それらが示すとおりに `repair` または `rollback` を使ってください。

各アクションの詳細：

- **Install** は何かを変更する前に、tarball のバイト列をそのハッシュと、パッケージを tarball のマニフェストと照合し、ポートが空いていることを確認します。Remote Script のフォルダーに `__pycache__` という名前の空のファイルを置き、Live がコンパイル済みのコピーを書き込んだり読み込んだりできないようにします。その場所にほかのものがあると、ドリフトとして扱われます。
- **Activate** は、期待されるレジストリハッシュを持つ本物の Live から認証済みの応答があった後でのみ `activated` を記録します。シミュレーター、古いまたは誤ったレジストリ、応答なしの場合は `activation-required` になり、次に何をすべきかを伝えます。記録されたアクティベーションは履歴であり、いま Live が接続している証拠ではありません。
- **Upgrade** は厳密に新しいバージョンを必要とし、ドリフトしたファイルがあると拒否します。`rollback` 用に前のバージョンと設定を残します。
- **Repair** は、欠けているシークレットを決して作成しません。新しいシークレットは、ブリッジに対する新しい権限になるからです。もう一度実行しても何も変わりません。
- **Uninstall** は、`--purge-secret` がない限りシークレットを残し、診断ログも残します。削除は通常の unlink であり、安全な消去ではありません。

**診断ログ。** インストール時に `--enable-bridge-diagnostics` を付けると、Remote Script は状態フォルダー内の `bridge-diagnostics.log` に、短く秘匿処理した記録を書き込みます。所有者だけがアクセスでき、キューに入れてバックグラウンドで書き込まれ、最大 16 MiB です。このフラグがなければログはありません。

## Live のフォルダー

| フォルダー | macOS | Windows |
| --- | --- | --- |
| Remote Scripts（デフォルトの User Library） | `~/Music/Ableton/User Library/Remote Scripts` | `Documents\Ableton\User Library\Remote Scripts`（または `OneDrive\Documents` の下） |
| Extensions（Live 12.4 以降） | `~/Library/Application Support/Ableton/Extensions` | `%LOCALAPPDATA%\Ableton\Extensions`（未確認） |
| Control Surface の設定 | Live → Settings → Link, Tempo & MIDI | Options → Settings → Link, Tempo & MIDI |

User Library を移動した場合は、Live の **Settings → Library** にその場所が表示されます。Kumi は Live の環境設定から自分で見つけます。Live のアプリケーションフォルダーには決してインストールしないでください。

## インストールの確認

```sh
ableton-mcp-server diagnostics --config /absolute/path/to/bridge-config.json
```

JSON のレポートを出力します。対応していないシステムでは終了コード 1 で終わり、Live に到達できない場合でも 0 で終わるので、各フィールドを読んでください：

| フィールド | 意味 |
| --- | --- |
| `runtime`、`runtimeVersion`、`runtimeSupported`、`platformSupported` | ネイティブ Rust のランタイム、ブリッジのバージョン、プラットフォームへの対応 |
| `readiness.package` | パッケージとその Remote Script のファイルが存在し、損なわれていない（Live がそれを読み込んだという意味ではありません。インストールされたコピーは `status` で確認します） |
| `readiness.configured` | 設定が有効で、ブリッジを指定しており、読み取れるシークレットがある |
| `readiness.authenticatedBridge` | Remote Script が認証済みの接続で応答し、ディスカバリーが成功した（`registryHash` がそのレジストリを示します） |
| `readiness.realLiveOperational` | その応答がシミュレーターではなく本物の Live（`real-live` の来歴）から来た |
| `ready` | 上記のすべて |

シークレットは決して出力されません。再インストールは接続を直す方法ではありません。Live を再起動したか、Control Surface が選ばれているか、設定、シークレット、ポートが一致しているかを確認してください。

## 設定をバージョン 2 に移行する

`ableton-mcp-server migrate` は、デフォルトでは古い（レガシーまたはバージョン 1 の）クライアント設定をそのまま残します。ブリッジのすべてのフィールドと、所有者だけがアクセスできる既存のシークレットを与えると、バージョン 2 のブリッジ設定を書き込みます：

```sh
ableton-mcp-server migrate --input /absolute/old.json --output /absolute/bridge-v2.json \
  --bridge-host 127.0.0.1 --bridge-port 9765 --realtime-port 9766 \
  --secret-file /absolute/bridge.secret
```

シークレットを作成することはなく、ループバックのホストだけを受け付け、`--force` がなければ既存のファイルを置き換えません。
