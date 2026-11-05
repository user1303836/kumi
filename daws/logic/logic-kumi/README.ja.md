# Logic Pro ブリッジ: M2

- macOS の Logic Pro 12 向けの、実機の情報を読む実験的な基盤です。認証付きループバック接続で公開するのは `ping`、`status`、`focus`、`read`、`snapshot`、`focus_body` の6メソッドと、プロジェクト・トランスポート・トラック・選択のイベントです。実機への読み取りが待機していても、キャッシュ済みの状態と FOCUS は応答します。
- 読み取り対象は project、track、region、note、plugin、send、marker、tempo の行です。本文は `tree`、`notes`、`timeline`、`routing` に対応します。楽曲の編集、試聴、レンダリングは公開しません。アプリには Logic のプロバイダーや DAW 選択項目もまだありません。ホストでの公開と、実機・アプリの受け入れ確認は別の作業です。
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

実機記録の再生には `d821e669` の元の M2 シーン4件と26要求のベクトルを使います。`tests/fixtures/sources.json` に元のパスと SHA-256 を記録し、テストは元のバイト列の一致と実際の読み取り結果を、期待値を書き換えずに確認します。この再生は、現在の Logic やアプリでの実機受け入れ確認を意味しません。読み取りのために、ヘルパーが Logic のペインや選択を移動する場合があります。
