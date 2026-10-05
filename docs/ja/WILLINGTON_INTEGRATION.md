# Willington

[English](../en/WILLINGTON_INTEGRATION.md) · [简体中文](../zh-CN/WILLINGTON_INTEGRATION.md) · 日本語

Willington は、Live の Python API では届かない部分に手が届くネイティブプロバイダーのセットです。Session クリップの Follow Action、ラックのマクロのマッピングと名前、ラックのチェーンのゾーンです。Kumi のブリッジは Willington のランタイムファイルを Remote Script の中に収めていて、`/willington` でオンにするまではオフです。各プロバイダーは、接続している Live のビルドそのものに合うバインディングを選びます。それ以外のビルドではツールが表示されないだけで、Kumi のほかの部分はこれまでどおり動作します。

## 追加されるもの

| Kumi のツール | ブリッジのツール | 編集の種類 | プロバイダー |
| --- | --- | --- | --- |
| `set_clip_follow_actions` | `live_follow_actions_preview/apply` | Session クリップの Follow Action の 10 個のフィールドすべて | WillingtonBindings |
| `edit_rack_mapping` | `live_willington_device_preview/apply` | `macro-name`、`variation-name`、`macro-mapping` | WillingtonDeviceTools |
| `edit_rack_mapping` | `live_willington_device_preview/apply` | `selector-zone`、`key-zone`、`velocity-zone` | WillingtonRackZones |

ブリッジが提供するのは、プロバイダーが書き込み有効の状態で読み込まれている編集の種類だけです。`live_willington_device_preview` は、それらの `kind` の値だけを並べます。どの編集でも、トランスポートが止まっている必要があります。

ラックの改善の中には、Willington が要らないものもあります。Live 自身のマクロのレイアウトでのラックの読み取り、インデックスによるバリエーションの呼び出しや削除、Live の Browser に Modulators カテゴリがないときの標準のモジュレーターデバイスへのフォールバックです。これらはどのブリッジでも動作します。

## オンとオフ

Kumi で `/willington` と入力します。`Remote Scripts/AbletonMcpBridge` の中、ブリッジの `__init__.py` の隣に `willington.json` を書き込み、すべてのプロバイダーを編集つきでオンにします。ブリッジは Live を動かしたまま、1 秒以内にそれらを読み込みます。もう一度 `/willington` と入力するとファイルを削除し、ブリッジは DeviceTools と RackZones をアンインストールして、Follow Action の書き込みをオフにします。バインディングがオフのあいだは、Kumi が起動時にそう伝えます。

Kumi はモデルにも伝えます。バインディングがオフのときにその編集が必要な依頼があると、`/willington` でオンになることを一言添えます。オンのときは、Live で自分でマッピングするよう頼む代わりに、Kumi がマクロをマッピングします。

どの Live ビルドに対応するかは、Kumi が収めている Willington のリリースによって決まり、ブリッジのフォルダーの `willington/release.json` に書かれています。Willington の検証済みのバインディング：macOS ARM64 の Live 12.4.15b4 と b5 用の Follow Action と DeviceTools、macOS ARM64 の b5 用の RackZones、そして Windows x64 の Live 12.4.15b5 用の 3 つすべてです。Intel macOS には対応していません。各プロバイダーは、動いている Live プロセスの OS、アーキテクチャ、バージョン、実行ファイルのハッシュからバインディングを選び、ネイティブライブラリは動いている実行ファイルそのもの（macOS では Mach-O の UUID、Windows では CodeView の GUID）も確認します。

Follow Action を編集するには、選ばれたライブラリのセルフテストに合格していることも必要です。WillingtonBindings のフォルダーに、`"status": "passed"` と、そのライブラリの SHA-256 に等しい `library_sha256` を持つ `self-test.json` が必要です。これがないと Follow Action の編集だけがオフのままで、マクロ、名前、ゾーンの編集は動作します。

検証済みのプロファイルがないときは、そのコンポーネントだけが飛ばされます。1 つの設定で、b4 では Follow Action と DeviceTools を、b5 ではさらに RackZones も使えます。こうしたプロファイルがないことによる型付きの拒否は、Live のプロセスごと、コンポーネントごとにキャッシュされ、ログに出るのも 1 回だけです。

不正な設定、足りないアーティファクト、整合性のエラー、予期しない起動時の失敗、ほかに動いている持ち主のいずれかがあると、ネイティブの拡張は使えなくなりますが、通常のブリッジは動き続けます。Live のログ（Log.txt）には、原因と、実際に動いているプロバイダー、書き込みが有効なプロバイダーの名前が出ます。Follow のセルフテストがないか古いときは、Follow の書き込みだけがオフになります。

Remote Script が止まると、Follow Action の書き込みをオフにし、DeviceTools と RackZones をアンインストールします。Follow Action のバインディングはアンインストールできず、その Live プロセスに登録されたままになります。ブリッジが再び起動したときや、`/willington` のあとにプロバイダーを読み込み直したときは、書き込みをオフにした状態でそれを再利用します。

### willington.json

`/willington` がこのファイルを書き込みますが、自分で書くこともできます。通常のファイルで、オーナー専用、4 KiB 以下で、次のキーを持つ必要があります（`rackZones` は省略可）。`/willington` が書き込む内容：

```json
{"version": 1, "followActions": true, "deviceTools": true, "rackZones": true, "enableWrites": true}
```

| キー | 意味 |
| --- | --- |
| `version` | 常に `1` |
| `followActions` | WillingtonBindings を読み込む |
| `deviceTools` | WillingtonDeviceTools を読み込む |
| `rackZones` | 省略可。WillingtonRackZones を読み込む |
| `enableWrites` | 編集を許可する。`false` ではプロバイダーを読み込むが、編集は提供しない |

ブリッジは、このファイルが変わってから 1 秒以内に読み直します。Kumi を更新しても残り、ブリッジのインストールの確認にも影響しません。このファイルがなければ、素のブリッジです。

## Willington を自分でインストールする

Kumi がまだ収めていない Willington のビルド（たとえば新しい Live のバージョン用）を使うには、Willington のマルチバージョンのバンドルを Live の Remote Scripts フォルダーの AbletonMcpBridge の隣にインストールします。必須の `WillingtonRuntime` と、使いたいプロバイダー、`WillingtonBindings`（Follow Action）、`WillingtonDeviceTools`（マクロとバリエーション。`get_macro_mapping` と `get_selected_variation_name` を提供している必要があります）、`WillingtonRackZones`（ゾーン）です。手作業でコピーするときは、ランタイム、`build/<profile-id>/` フォルダー、マニフェストをそのまま残してください。そこにインストールしたプロバイダーは、Kumi 自身のコピーより優先されます。Live でスタンドアロンの Willington コントロールサーフェスをすべてオフにし、Live を再起動します。ブリッジはプロバイダーをほかの持ち主と共有しません。それから `/willington` でオンにします。

## Follow Action のセルフテスト

Live のビルドを切り替えたときや、選ばれた Follow のライブラリを置き換えたときは、やり直してください。このテストは WillingtonBindings を単独のコントロールサーフェスとして動かすので、[ブリッジの隣にインストールした](#willington-を自分でインストールする)コピーが必要です。ブリッジの中にある Kumi 自身のコピーは、Live の一覧に出ません。この手順は配布されたバンドルで使えます。ソースのチェックアウトや `manage.py` は要りません。スタンドアロンのテストは、今の Set にフィクスチャのトラックを作り、ネイティブの書き込みと Live の取り消しを行うので、使い捨ての Set を使ってください。

1. Live のコントロールサーフェスの設定で `AbletonMcpBridge` とスタンドアロンの Willington サーフェスを無効にし、Live を終了します。ネイティブの Follow のプロパティは、プロセスが終わるまで登録されたままです。
2. 使う予定の Live のビルドを起動し、再生を止めた状態で使い捨ての Set を開き、Willington のコントロールサーフェスとして `WillingtonBindings` だけを選びます。MIDI の入力と出力は None にします。インストールされた `status.json` が `"status": "registered"` を示しているはずです。
3. 次のコマンドでテストをキューに入れます。フォルダーの引数は、インストールした Bindings のフォルダーに置き換えてください。保留中のコマンドがあれば拒否し、古いレシートを削除するので、今回の実行と取り違えることはありません。

   ```sh
   python3 - '/path/to/User Library/Remote Scripts/WillingtonBindings' <<'PYTEST'
   import json, os, sys
   from pathlib import Path
   folder = Path(sys.argv[1]).expanduser()
   assert (folder / '__init__.py').is_file(), 'Not an installed Bindings folder'
   command = folder / 'command.json'
   assert not command.exists(), 'A command is already pending'
   (folder / 'self-test.json').unlink(missing_ok=True)
   temporary = folder / 'command.json.tmp'
   temporary.write_text(json.dumps({'action': 'self_test'}) + '\n')
   os.replace(temporary, command)
   PYTEST
   ```

4. 新しい `self-test.json` が `"status": "passed"` と `library_sha256` を持って完了するのを待ちます。実行中や失敗の報告では、書き込みは有効になりません。コマンドが失敗したら `command-error.json` を確認してください。ハッシュは、選ばれた `build/<profile-id>/libwillington.dylib`（従来のインストールではルートのライブラリ）と一致する必要があります。`shasum -a 256 '/full/path/to/libwillington.dylib'` でそのダイジェストを表示できます。レシートは、インストールした Bindings のフォルダーに残しておいてください。
5. スタンドアロンの `WillingtonBindings` コントロールサーフェスを None にし、Live を終了して再起動してから、`AbletonMcpBridge` を再び有効にします。使い捨ての Set は破棄します。スタンドアロンの Willington サーフェスを Kumi と一緒に選ばないでください。両方がネイティブのバインディングを持とうとします。Kumi は、`enableWrites: true` で Follow の書き込みを有効にする前に、自分が選んだライブラリに対してレシートを確認し直します。

## Follow Action

`live_follow_actions_preview/apply` は、Session クリップ 1 つの 10 個のフィールドすべてを設定します。有効、リンク、アクション A と B、確率 A と B、ループ回数、時間、ジャンプ先 A と B です。

- アクションは数値です。0 none、1 stop、2 again、3 previous、4 next、5 first、6 last、7 any、8 other、9 jump。ジャンプ先は 1 から数えるシーン番号です。
- 2 つの確率の合計は 100 です。片方だけを指定すると、もう片方は残りに設定されます。
- リンクしたタイミングではループ回数を使い、リンクしないタイミングでは拍単位の `time` を使います。
- トランスポートが止まっていて、クリップが録音中でない必要があります。
- シーンの Follow Action や、Live 全体の Follow Action のスイッチは変更しません。起動時の Legato には、クリップ設定のツール（Kumi では `set_clip`）を使ってください。

プレビューは 10 個のフィールドすべてを記録します。書き込みが途中で失敗すると、それまでに書いたフィールドは元に戻ります。取り消しは記録したフィールドを復元し、そのあとクリップが変わっていれば拒否されます。

## マクロ、バリエーション、マッピング

`ref` でラックを指定した `live_willington_device_preview/apply`：

| 種類 | 引数 | 補足 |
| --- | --- | --- |
| `macro-name` | `macroIndex`（0–15）、`name` | マクロの名前を変える |
| `variation-name` | `name` | 選択中のバリエーションの名前を変える。名前の付いたバリエーションが選択されている必要があります |
| `macro-mapping` | `targetRef`、`mappingIndex`（0–15、または割り当てを外す `null`）、`minimum`、`maximum`、`mappingKind` | このラックの中のパラメータをマクロに割り当てる |

マッピングの種類：

- `continuous` と `enum`：`minimum` と `maximum` はパラメータ自身の単位で、その範囲内で指定します。範囲は反転していてもかまいません。`enum` の両端は整数です。
- `boolean`：0 から 127 までの整数のマクロのしきい値で、`minimum` ≤ `maximum`。

`targetRef` は最新のディスカバリーで得たもので、このラック、その中にネストされたデバイス、またはそのチェーンのミキサーの中になければなりません。トランザクションは、名前、あるいはマッピングとパラメータの値とマクロの値を、関係する同一性とあわせて記録します。Live はマッピングされたパラメータの値を 1 ティック遅れて設定するので、マッピングはその値ではなく、マッピングとマクロの値でフェンスされます。書き込みはすべて読み戻され、一致しなければ正確に元に戻されます。取り消しは記録した状態を復元し、そのあとラックが変わっていれば拒否されます。これはトランザクション自身の取り消しで、Live の取り消しではありません。

## ラックのチェーンのゾーン

`ref` でラックを、`targetRef` でその通常のチェーンの 1 つを指定した `live_willington_device_preview/apply`：

| ラック | ゾーン |
| --- | --- |
| Audio Effect Rack | `selector-zone` |
| Instrument Rack、MIDI Effect Rack | `selector-zone`、`key-zone`、`velocity-zone` |

Drum Rack とリターンチェーンは拒否されます。ゾーンには 4 つの整数の端点 `minimum`、`fadeMinimum`、`fadeMaximum`、`maximum` があり、0–127（ベロシティでは 1–127）の範囲で、この順序（`minimum` ≤ `fadeMinimum` ≤ `fadeMaximum` ≤ `maximum`）を保つ必要があります。省略した端点は今の値のままなので、範囲を動かすときは両方のフェードの端点も指定する必要があるかもしれません。

プレビューは 4 つの端点すべてを記録します。適用と取り消しは、ラックとチェーンの同一性と、ゾーンの状態全体でフェンスします。指定どおりに読み戻せない書き込みは、正確に元に戻されます。

## 証拠

| プロバイダー | 実行 | Live | ブリッジ | 対象 |
| --- | --- | --- | --- | --- |
| Follow Action | [kumi-clip-follow-actions-b5.json](../evidence/kumi-clip-follow-actions-b5.json)、2026-09-30 | 12.4.15b5、macOS arm64 | 1.0.53 | 保存したテスト Set での Kumi の変更と取り消し、トランスポート停止中 |
| Follow Action、マクロ、マッピング | [willington-kumi-chat.json](../evidence/willington-kumi-chat.json)、2026-09-30 | 12.4.15b4 ARM64 | 1.0.52 | 実際の Kumi のチャット：Follow Action、マクロ名の変更、マッピング、それぞれを取り消し |
| ラックのゾーン | [rack-zones-b5.json](../evidence/rack-zones-b5.json)、2026-10-01、完了 2026-10-02 | 12.4.15b5（2026-09-24 ビルド）、arm64 | 1.0.66（Kumi のトランザクション） | 読み戻し、書き込み、取り消しとやり直し、保存と再オープン、Kumi の取り消し。完了時の検証で、信号のゲーティング、フェード、実際の Max からの呼び出しを追加 |

バリエーション名の変更と、反転した continuous と enum のマッピングは、ブリッジを直接使ってテストしました。Follow Action のスケジューリングと、Follow Action とマクロの編集が Set の保存と再オープンのあとも残るかどうかは、テストしていません。

ラックのゾーンの完了時の検証結果とレシートのダイジェストは、[公開の検証サマリー](../evidence/rack-zones-b5.json)にあります。信号のゲーティングのチェック 42 件、フェードの測定 49 件とそれによる向きの比較 14 件、実際の Max の `live.object` による書き込み・読み取り・復元のサイクル 7 回です。正式に採用した `live-12.4.15b5-arm64` のライブラリは、テストした候補のライブラリとバイト単位で同一です。測定には正規化した Live のメーターを使っています。正確な線形のゲイン、フェードの端点での無音、ノートを押さえたままの編集、複数のチェーンが重なるクロスフェード、ほかのビルドやプラットフォームについては主張していません。ラックのゾーンは、b4 では引き続きサポートされていません。完了時の生のレシートとハーネスは、非公開の Willington リポジトリの、サマリーに記録した不変のコミットに保管されています。その生のファイルはここでは公開していません。

ブリッジの自動テストは、残りを Live なしでカバーしています。プロバイダーがない場合、不正な設定、古い編集や競合する編集、部分的な書き込み、持ち主の扱いと再接続、応答の喪失です。

## 意図的に提供していないもの

Willington にはこれらのためのネイティブメソッドがありますが、安全に取り消せるようになるまで、Kumi は提供しません。

- **バリエーションの上書き**：バリエーションの名前だけでなく、保存されたマクロの値と有効マスクの全体を読み取り、復元する必要があります。
- **Drum Sampler のサンプルの直接置き換え**：今のサンプルの同一性とパス、そして置き換えで変わるものの復元が必要です。Browser を通じたプリセットやサンプルのロードは使えます。
- **モジュレーターのマッピング**：ネイティブの変更はあとから落ち着くため、Live のスレッドと歩調が合いません。まず、落ち着いた状態の確認、ソースと対象の正確な持ち主の確認、キャンセル、復元が必要です。

ネイティブメソッドがあるだけでは、取り消せる操作には足りません。ランタイムの記述子やプロトコルのエントリを追加するだけで、それを提供しないでください。
