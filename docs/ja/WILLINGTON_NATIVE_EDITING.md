# Willington ネイティブ編集

[English](../en/WILLINGTON_NATIVE_EDITING.md) · [简体中文](../zh-CN/WILLINGTON_NATIVE_EDITING.md) · 日本語

Willington のリポジトリは非公開です。このガイドの上流リンクを開くにはアクセス権が必要です。

このガイドはネイティブ Python API を説明します。Kumi の保護されたツールと、記録を確認する `/willington` の設定は [Willington 統合](WILLINGTON_INTEGRATION.md#ネイティブ編集とセルフテスト)を参照してください。ネイティブ編集にはインストール済みの検証済み **Live 12.4.15b5 または b6 macOS ARM64** コンポーネントが必要です。他のビルドと Windows は対象外です。

b6 には Willington `03b6efccd1c72ef816ae5be6f28ad33115ef3e9a`、または b6 プロファイルを含む後続のバンドルが必要です。古い Kumi バンドルは更新してください。[b6 の上流検証](https://github.com/xonedsp/willington/blob/03b6efccd1c72ef816ae5be6f28ad33115ef3e9a/evidence/live-12.4.15b6-arm64/README.md)と [Kumi CI バンドルの検証](../evidence/willington-b6-import.json)を参照してください。

使い捨てのテスト用 Set での開発では、上流の[ビルドとインストール手順](https://github.com/xonedsp/willington/blob/main/integrations/WillingtonEditing/README.md)に従ってください。昇格済みパッケージをインストールすると、`api.install()` がビルドに一致するライブラリを自動で選択・検証します。書き込みは明示的に有効にするまで無効で、トランスポートは停止している必要があります。インストール後、full ポリシーが Python を許可していれば、`run_python`（ホスト側では `live_run_python`）からメソッドを呼べます。メソッドがない場合やビルドの検証で拒否された場合は、Kumi の Willington スイッチにかかわらず利用できません。

Python 呼び出しは Kumi の `HISTORY` に**記録されません**。ネイティブメソッドには独自の Live の取り消し境界があり、1 本のスクリプト内の複数の呼び出しが複数の取り消しステップになることがあります。全体の Follow Action スイッチは Live の取り消しに入りません。失敗したスクリプトの補償として、無条件に `song.undo()` を呼ばないでください。

`run_python` には `song` が渡され、現在のオブジェクトの `ref` を指定すると `obj` も渡されます。再接続後は参照を取得し直し、別の Set のオブジェクト番号を流用しないでください。

## グループ作成

`song.group_tracks(*tracks)` は UI の選択とは独立して新しいグループを返します。Song 内の順序どおりに並んだ、重複のない連続した最上位のオーディオ/MIDI トラック 1～128 本を指定します。既存のグループ、入れ子、マスター/リターントラック、別の Song のトラックは拒否されます。

`song.ungroup_track(group)` は、オーディオ/MIDI のメンバーを持ち、デバイスも入れ子のグループもない最上位のグループを受け付けます。テスト直後の復元には使えますが、一般的な履歴の逆操作ではありません。その後のルーティング、オートメーション、デバイス、メンバー構成の編集を失ってはいけません。専用の履歴では、それらをすべて記録し、変更されていないことを確認する必要があります。

## シーンと全体の Follow Action

`scene.get_follow_actions()` は JSON を返します。`scene.set_follow_action(field, value)` は、`enabled`、`action_a`、`action_b`、`chance_a`、`chance_b`、`jump_a`、`jump_b`、`time`、`linked`、`loop_count` を設定します。アクションは 0～9、確率は 0～100 で、合計が 100 になるよう連動します。ジャンプ先は 1 始まりのシーン番号（0 は未設定）で、上限は 8388608 です。時間は四分音符単位の拍数で最小 0.25、ループ回数は 1～1073741823 です。リンク時は最も長いクリップとループ回数を使います。状態全体を保持し、書き込み後に読み直してください。

`song.get_follow_actions_enabled()` は全体のスイッチを読み、`song.set_follow_actions_enabled(True)` または `False` で変更します。以前の真偽値を保持して明示的に復元してください。シーン/全体の監視 API はありません。テストでは UI からの起動でスケジューリングを検証しましたが、`Scene.fire()` では同じ UI の動作を再現できませんでした。

## ノート単位の MPE

MIDI クリップを読み直して得た安定したノート ID を使います。`clip.get_note_expression(note_id, dimension)` は JSON を返し、dimension は `pitch`、`slide`、`pressure` です。`clip.replace_note_expression(note_id, dimension, state_json)` は、真偽値の `exists` と `events` リストを含む状態を受け付けます。

イベントは `[time, value, x1, y1, x2, y2]` です。時間はノート開始からの拍数、ピッチはセント（±4800）、slide/pressure は MIDI 単位（0～127）です。上限は 65536 イベント、時間は 0～1576800、同時刻のイベントは最大 2 件、カーブ係数は 0～1 です。レーンが存在しない状態と、存在する空のレーンは異なります。復元には `exists` とすべてのカーブ係数を保持し、同じクリップとノートがまだ存在することを確認してください。

## Arrangement オートメーション

所有するトラックと連続値のパラメーターを指定します。量子化されたパラメーターや別トラックのパラメーターは拒否されます。

- `track.get_arrangement_automation(parameter, start, end)` — 区間の JSON。
- `track.insert_arrangement_event(parameter, event_json)` — Song の絶対拍数（0～1576800）と公開パラメーター単位を使う、6 数値のイベント挿入。
- `track.delete_arrangement_events(parameter, start, end)` — 区間の削除。
- `track.get_arrangement_snapshot(parameter)` — 不透明な完全スナップショットの JSON。
- `track.restore_arrangement_snapshot(parameter, snapshot_json)` — 明示的な復元。

変更前に完全なスナップショットを保持してください。区間の読み取りは隠れた初期イベントと区間外のイベントを含まず、完全な取り消しデータにはなりません。スナップショットはネイティブの生の値、カーブ、同時刻のイベント、エンベロープの非存在を保持します。挿入時に Live がカーブのハンドルを正規化し、最後の点のハンドルをリセットすることがあります。指定した値が残ると仮定せず、読み戻して確認してください。

スナップショットは署名され、元のパラメーターとアダプターのインスタンスに結び付いています。アダプターの再インストールや Live の再起動で無効になります。永続的な Kumi の履歴には使えません。内容を書き換えたり、署名の拒否後に Live の取り消しで代用したりしないでください。未完了のオートメーション変形がある場合は拒否されます。UI の選択とイベントオブジェクトの同一性は保証範囲外です。

元の所有オブジェクトが削除された場合、またはアダプターの 128 所有者の FIFO キャッシュから追い出された場合も、スナップショットは失効します。ポインターの再利用で所有権が引き継がれることはありません。

## 検証とリリースの境界

上流の[テスト用 Set の検証記録](https://github.com/xonedsp/willington/blob/main/evidence/native-editing/b5/README.md)には、読み戻し、ネイティブの取り消し/やり直し、保存と再オープン、Max 呼び出し、MPE 再生とノート通知、Arrangement 再生、グループのルーティング、UI からのシーン起動が含まれます。未完了変形の拒否はネイティブのフラグコントローラーでテストしており、実際の UI ドラッグではありません。これは上流の証拠であり、Kumi のツールの受け入れ検証ではありません。

Kumi は [Willington の更新手順](DEVELOPER_GUIDE.md#willington-のファイル)を通じてのみ Willington のファイルを取り込みます。
