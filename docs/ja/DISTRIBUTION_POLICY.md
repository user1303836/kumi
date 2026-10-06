# リリースと配布

[English](../en/DISTRIBUTION_POLICY.md) · [简体中文](../zh-CN/DISTRIBUTION_POLICY.md) · 日本語

Kumi とそのブリッジがどのように人の手に届くか、それによって何が証明され何が証明されないか、ブリッジのパッケージに何を含めてよいかをまとめます。リリースを作る手順は[開発者ガイド](DEVELOPER_GUIDE.md#リリース)にあります。

## 配布チャンネル

| 対象 | 場所 | 入手方法 |
| --- | --- | --- |
| Kumi | `user1303836/kumi` の GitHub Releases：ネイティブの `kumi-<target>.tar.gz`、互換用の `kumi.tar.gz`、`kumi-release.json`、`SHA256SUMS`。Installer ワークフローが各 `vX.Y.Z` タグに添付します | `install.sh` または `install.ps1`、その後は `kumi update` |
| ブリッジ（`@ableton-mcp/mcp-server`） | 各 Kumi バンドルの中に、ネイティブのブリッジ tarball と展開済みのパッケージが入っています | `kumi bridge`。ブリッジのライフサイクルを通じてインストールします（[ブリッジのインストール](DELIVERY.md)） |
| ブリッジ単体 | 独自のリリースはありません。`python3 scripts/build-native-release.py --bridge-only` でビルドします（[ビルドオプション](DEVELOPER_GUIDE.md#リリース)） | ライフサイクル CLI（[ブリッジのインストール](DELIVERY.md#スタンドアロンのブリッジ)） |

インストーラーのスクリプトは `main` ブランチから読み込まれ、それがインストールするバンドルは最新の公開リリース（または `KUMI_VERSION` で指定したリリース）から取得されます。リリースはメンテナーが公開するまでは下書きで、公開されたリリースだけが「latest」になります。npm には何も公開しません。すべてのパッケージが `private: true` なので、`npm publish` は拒否されます。

## 完全性は確かめるが、作成者は証明しない

アプリとブリッジには発行者の署名や公証がなく、`.pkg` や `.msi` のインストーラーもありません。macOS の Hands ヘルパーにはアドホック署名がありますが、発行者の身元は証明しません。インストーラーと `kumi update` はバンドルを `kumi-release.json` 内の sha256 と照合します。新規のネイティブインストールは Node を取得しません。`kumi bridge` はブリッジの tarball を、バンドルのビルド時に記録されたハッシュと照合します。ダウンロードと同じ場所から来たチェックサムは、バイト列が無傷で届いたことは証明しますが、誰が作ったかは証明しません。

ソフトウェアは [MIT ライセンス](../../LICENSE.md)です。このライセンスは Ableton の商標に関する権利を一切与えるものではなく、Kumi は Ableton と提携しておらず、Ableton の承認を受けたものでもありません。

## ブリッジのパッケージに含めてよいもの

- ネイティブ実行ファイル `ableton-mcp-server` と `ableton-mcp-analysis-worker`（Windows では `.exe`）
- Remote Script、その README、操作レジストリ、それらのハッシュマニフェスト
- Kumi の Live 拡張機能：そのマニフェスト、`package.json`、ビルドされた `extension.js` と、その sha256
- ブリッジのガイド（`README.md` と `release-docs/`）
- `release-manifest.json`、`package.json`、`LICENSE.md`

それ以外は含みません。ビルドスクリプト、テストのフィクスチャ、`node_modules`、認証情報、設定、ローカルの状態、ログ、キャプチャしたメディア、エビデンスは入りません。ネイティブのビルダーとライフサイクルは `release-manifest.json` の正確なファイル一覧とハッシュを検証します。

## リリースマニフェスト

`release-manifest.json`（スキーマ `ableton-mcp-native-release/v1`）には、パッケージ名とバージョン、ソースのコミットと未コミット変更の有無、Rust ターゲット、rustc と Cargo のバージョン、ランナーイメージ、`Cargo.lock` と CI ワークフローの SHA-256、ビルドレシピ、プロトコルのレジストリハッシュ、各ペイロードファイルの役割と SHA-256 を記録します。

配布フィールドは `channel: "local-native-tarball"` で、`published`、`signed`、`notarized`、`integrityIsIdentityProof` はすべて `false` です。ライフサイクルはこれらの値を要求します。tarball はローカルのパスからハッシュを確認してインストールされ、GitHub Releases 上の Kumi バンドルに入って届きます。パッケージレジストリには公開しません。

既存のインストールのアップグレードとロールバックのため、ライフサイクルは旧スキーマ `ableton-mcp-release/v2` と `ableton-mcp-private-release/v1` も受け付けます。これらの Node/npm/TypeScript のビルド記録と `local-npm-tarball` チャンネルは、Kumi 1.7.5 以前のブリッジのパッケージについてのものです。

## マージゲート

リポジトリにはルールセットが二つあります：

- **`main`：**
  - 変更はプルリクエストで入ります。承認のレビューは必要ありません。
  - 必須チェックは `Required CI` と `Willington files` の二つで、`main` に対して最新の状態にしたブランチでパスする必要があります。
  - `main` は削除もフォースプッシュもできません。
  - リポジトリの管理者ロールは、プルリクエストについてこれらのルールをバイパスできます。
- **`Release tags`：** `v*` タグを作成、移動、削除できるのはリポジトリの管理者ロールだけです。タグをプッシュすると Installer が実行され、リリースを公開します。

`Willington files` は、`vendor/willington/` を変えるのを、このリポジトリのブランチから出たリポジトリ所有者のプルリクエストだけに限り、そのプルリクエストがほかに何も変えないようにします。これらのファイルはすべてのプロデューサーに届き、ネイティブライブラリはレビューできないので、だれが送ったかを確認します。更新の手順は[開発者ガイド](DEVELOPER_GUIDE.md#willington-のファイル)にあります。

Installer ワークフローは必須チェックではありませんが、タグでは、その `publish` ジョブはバンドルが macOS、Linux、Windows でインストールできた後にだけ実行されます。すべてのジョブは[テスト](TESTING.md#ci)で説明しています。

## 未決のオーナー判断

- macOS と Windows での、Kumi のバンドルとインストーラーの**署名と公証**。
- **Extensions SDK の再配布。** Kumi の Live 拡張機能は、ローカルで用意したプレリリース版の Ableton Extensions SDK からビルドされます。SDK のライセンスがその再配布を制限しているため、リポジトリには決してコミットしません。ビルドされた `extension.js` は、拡張機能とそれが使う SDK のコードをバンドルしたもので、コミットされ、ブリッジのパッケージと Kumi のバンドルに入って配布されます。それが許されるかどうかはオーナーが決めることです。
- `main` のルールセットの**管理者バイパス**：残すか、取り除くか。
