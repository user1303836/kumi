# Logic Pro ブリッジ: M1

- macOS の Logic Pro 12 向けの実験的な基盤です。認証付きループバック接続で公開するのは `ping`、`status`、`project` イベントだけです。Logic のウインドウが示すプロジェクトを追跡し、検出ファイルを公開します。
- この段階ではプロジェクトの行、FOCUS、編集、試聴、レンダリングを公開しません。アプリには Logic のプロバイダーや DAW 選択項目もまだありません。単独のブリッジとホストの基盤です。
- `kumi-logic-hands` は別の子プロセスです。アクセシビリティ権限の不足は `busy` とは別に `permission: "accessibility"` として返します。最初の接続時に macOS がヘルパーの権限を求める場合があります。Logic 内には何もインストールせず、Logic の再起動も不要です。

リポジトリのルートで実行します。ビルドは同時に一つだけ実行してください。

```sh
python3 scripts/build-logic-hands.py
cargo build -p logic-kumi
sh scripts/test-isolated.sh --profile ci -p logic-kumi
```

ヘルパーのビルドには macOS と Xcode コマンドラインツールが必要です。ソースのダイジェストを名前に含むユニバーサルヘルパーを `target/logic-hands/` に作成します。分離された Rust テストは使い捨てのヘルパーを使うため、Logic もアクセシビリティ権限も不要です。

Logic を開いてから、開発用に直接実行する例:

```sh
target/debug/logic-kumi --kumi-home /absolute/path/to/disposable-kumi
cargo run -p logic-kumi --example logic_lab_client -- --kumi-home /absolute/path/to/disposable-kumi
```

- ビルド済みヘルパーは自動で見つかります。`--helper <path>` で指定もできます。直接実行したブリッジは Logic の終了、または SIGINT、SIGTERM、SIGHUP で終了します。標準入力の EOF では終了しません。
- ホストは `--hosted` を付けます。この場合は標準入力パイプを閉じても終了します。どちらの終了経路でも、検出ファイルを削除し、応答待ちの要求があってもヘルパーを停止して回収します。ブリッジは Logic を起動・終了しません。
- Logic を使わない任意のネイティブヘルパー確認: `python3 -m unittest discover -s daws/logic/logic-kumi/helper/tests -p test_protocol.py`。macOS ヘルパーをコンパイルして実行し、使い捨ての仮想 MIDI ポートを使います。
