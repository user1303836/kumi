# Logic Pro ブリッジの契約（M2〜M6）

[English](../en/LOGIC_CONTRACT.md) · [简体中文](../zh-CN/LOGIC_CONTRACT.md) · 日本語

`logic-kumi` が `kumi-daw/1` を通じて何を答えるか、そしてそれぞれの答えがどの macOS のチャンネルから来るかをまとめます。ワイヤー、ハンドシェイク、ディスカバリーファイル、ライフサイクルは daw-bridge のもので、このページで扱うのは Logic の形だけです。ここにあることはすべて 2026-10-18 のラボ（`docs/evidence/logic-lab-2026-10-18.md`）で計測し、行については M2 の間に Logic Pro 12.3.1 の Scratch プロジェクトでも計測しました。正確な JSON は、ホストがシミュレートする Logic（`crates/daw-mcp-server/src/simulator/logic.rs`）と、ブリッジが本物の Logic から記録するベクター（`crates/daw-mcp-server/tests/vectors/logic-reads.json`）のとおりです。両者が異なるところでは、ベクターが Logic の示すものであり、その理由をこのページで説明します。記録には、それを作った Mac のホームの名前は入りません：ブリッジのレコーダーは `/Users/<name>` を `/Users/me` として保存し、プロジェクトの id（そのパスのハッシュ）は、秘匿化したパスのハッシュです（`logic_kumi::fixtures::redacted`）。

## チャンネル

| チャンネル | 用途 | ルール |
| --- | --- | --- |
| アクセシビリティの読み取り | すべての行、フォーカス、ボディ | ポーリングする。ポップアップボタンの子は決して読まない。`AXWindows` より `AXMainWindow` と Window メニューを優先する。Logic 自身のウィンドウ（Control Surface Setup）がメインのときは、プロジェクトのウィンドウをタイトルで指定する。読み取りに必要なペイン（Mixer、List Editors）はそのために開き、後で閉じる |
| アクセシビリティの書き込み | テキストフィールドかスライダーを持つ値、ボタン、行 | 設定してから `AXConfirm`。答える前に読み戻す |
| キーコマンド | キーが割り当てられている操作 | 先に Logic をアクティブにする。修飾キーはそれぞれ独立したイベントとして送る |
| メニュー項目 | キーが割り当てられていない操作 | 先に Logic をアクティブにする。タイトルを照合してから、インデックスで押す |
| Mackie Control | 選択、トランスポート、レベルのイベント。LCD の名前。トラックの選択。Logic の再生中に追うタイムコード表示（聴き取り）。ストリップのソロボタン（聴き取りのパスのソロ） | Mackie Control（デバイス 0x14）としてだけ応答し、バージョンの問い合わせにも応答する。選択は、LCD がストリップの名前を示す Pan ビューから行う。タイムコードの数字は ASCII（0x31 が "1"）で、最初の二つのコントローラーはアサインメント表示（"PN"） |
| パネルサービス | オープンパネルと保存パネル | `com.apple.appkit.xpc.openAndSavePanelService` にキーを送る。「フォルダへ移動」（go-to-folder）のフィールドは AX で設定する |
| Spotlight | 最近のプロジェクトの場所（`read {kind: "recent"}`） | プロジェクトのファイル名で `mdfind`。すべての名前を一回のクエリで。見つかるのは、macOS がブリッジに見せるものだけ |
| 環境設定 | 中央 C のオクターブ（プロジェクトの行の `middleC`） | `defaults read com.apple.logic10 <key>`。環境設定デーモンを通すので、Logic の Settings で変えたばかりの設定もすぐに読める。書き込みはしない |
| プロセスタップ | Logic を聴く（`hear`、`audition`） | Logic の pid に対するヘルパーの `tap.*`。リクエストのフォルダーの下の 32-bit float の WAV ファイルに記録する。一連のパスの前後で開始と停止をし、動かしたままにはしない。その間、`mute` は Logic の音をスピーカーから出さない |

## 識別情報

- `pid`：Logic のもの。`resource`：Logic のバンドル識別子（`com.apple.logic10`、Creator Studio 版では `com.apple.mobilelogic`）。
- `project.id`：すべてのプロジェクトに必ずある UUID はないので、id は保存されたパッケージのパスの SHA-256 の先頭 32 桁（16 進）です。保存されていないプロジェクトは、ウィンドウのタイトルから取った `identity` を持ちます。
- `status` は REAPER と同じくフラットです：`{project, busy, permission}`。`busy` は、空いているときは `false`、そうでなければ `modal`（メインまたはフォーカスのあるウィンドウがダイアログか、シートが開いている）、`away`（Logic がウィンドウを報告しない。Logic の Space（操作スペース）が現在のものではないため）、`loading`（プロジェクトのウィンドウにまだタイトルもドキュメントもない）のいずれかです。その間も、最後のプロジェクトは保たれます。
- `status.permission`：`null`、またはヘルパーが信頼されていない間は `"accessibility"`。そのとき読み取りは `failed` を答え、`busy` にはなりません。識別情報（daw、bridge、dawVersion、pid、resource、project）は、Kumi 独自のすべてのブリッジと同じく welcome に入っています。ディスカバリーファイルは、同じフィールドを持つ `logic-<pid>-<key>.json` です。

## 行（`read`）

| 種類 | フィールド | 取得元 |
| --- | --- | --- |
| `project` | id、identity、path、tempo、timeSignature、keySignature、sampleRate、trackCount、middleC（3 または 4） | ウィンドウのドキュメントと control bar の LCD。パッケージが読めるときは、サンプルレートを `MetaData.plist` から（macOS は Files and Folders の権限のないプロセスに `~/Music/Logic` を読ませない：そのときは `null`）。`middleC` は Logic の環境設定から。Logic のエディターがノート 60 をどのオクターブで呼ぶか（Settings > View > General の "Display Middle C As"：キー `MiddleC` がオンなら C4（Roland）、オフか一度も書かれていなければ C3（Yamaha。Logic のデフォルト））。読み直すのは多くとも 5 秒に一回 |
| `track` | ref、index、name、kind（audio、instrument、external、aux、output）、mute、solo、armed、monitoring、volumeDb、pan、color（常に `null`：ヘッダーは色を示さない）、output、inputs、selected、stack（Track Stack のヘッダーにだけ：`true`）、stripUnread（Mixer がそのトラックのストリップを描画していなかったときだけ：`true`。「キャッチアップの問い」を参照） | トラックヘッダー（name、mute、solo、armed、monitoring、volume、pan、selected。スタックの開閉用の三角形）。kind、output、inputs は Mixer のストリップから。必要なときに読み、5 秒間保持する |
| `region` | ref、trackRef、name、startBeats、endBeats、kind（audio、midi）、selected、looped | Tracks area のレイアウト項目とそのヘルプテキスト（"Region starts at 1 bar  and ends at 5 bars , MIDI region…"） |
| `note` | ref、regionRef、position（bar、beat、division、tick）、pitch、velocity、lengthTicks、channel、selected? | リージョンの Event List。Event List は選択中のリージョンのイベントを示すので、選択されていないリージョンは、まずその項目から選択する |
| `plugin` | ref、trackRef、slot、name、bypassed、kind（instrument、effect） | ストリップのスロット。インストゥルメントのものが先頭。ストリップはスロットを下から上へ並べ、行は上から数える。出力のプラグイン（Stereo Out のもの）は `trackRef` `<epoch>:output:<n>` の下 |
| `send` | ref、trackRef、slot、destination、level | ストリップのセンドスロット：センドのグループがそのバスの名前を示し、その直後に並ぶノブがレベルを示す（"-6.0"。"-∞" は −96 として） |
| `param` | ref、pluginRef、index、name、value、text、min、max、normalized | プラグインウィンドウの Controls View（読み取りのためにスロットの開くボタンから開き、後で閉じ、ビューを元に戻す）：パラメータごとに一行。コロンを除いた名前、スライダーの値と範囲（`normalized` は範囲の中での値の割合）、その横のテキスト。選択肢の値はそのオプションの位置、メニューの値は `null` で、値を設定するコントロールのないままビューが示す値も `null`（Compressor の Knee：テキストだけで、その変更は `bad-params` で拒否）。`parent`（プラグイン）が必要 |
| `smart` | ref、trackRef、index、name、value（0〜1）、text | トラックの Smart Controls ペイン（読み取りのために開き、後で閉じる）。トラックは読み取りのために選択し、選択は元に戻す。`parent`（トラック）が必要。ペインが割り当てるものがストリップにないトラック（Channel EQ だけのオーディオトラック）では、ペインはコントロールのないヘッダーを示し、行は一つも読めない。一方、ヘッダーを描画していないペインは `failed` |
| `automation` | ref、trackRef、index、parameter、number、events（positionBeats、value） | トラックの Track Automation Event List（そのキーコマンド、コマンド 1032、工場出荷時のセットでは Control-Command-E が、選択中のトラックについて独立したウィンドウとして開く。後で閉じる）：その "Fader" の行をパラメータごとにまとめ、パラメータの名前は Length/Info セルから、番号は Num から、各イベントの値（0〜127）は Val から。`parent`（トラック）が必要 |
| `marker` | ref、position、name、length（拍）、text | Marker List |
| `recent` | index（1 から。最も新しいものが先）、name、path（わからないときは `null`）、open | File > Open Recent の項目（メニューを開かずに読む：各項目は `_openRecentDocument:` で、タイトルはプロジェクトのファイル名）。`open` は、開いているプロジェクトのファイル名を持つ最初の項目だけで、その `path` はプロジェクトのウィンドウから。ほかの項目の `path` は、開いているもの以外にその名前のプロジェクトを Spotlight が一つだけ見つけたときに Spotlight から（「最近のプロジェクト」を参照） |
| `tempo` | bpm、tempos（position、bpm）、signatures（position、time、key） | Tempo List（`tempos` はそのすべてのイベントで、最初はプロジェクトの開始時のテンポ、続いて位置順のテンポの変化）と Signature List |

ブリッジが Logic のウィンドウの外で読むもの（環境設定、Spotlight、ディスク上のファイル）は一か所の継ぎ目（`src/disk.rs`）を通るので、記録はそれをヘルパーの返答と並べて残し、リプレイは同じように答えます：M6 の Scratch の読み取りの記録は、`MiddleC` を "0" として残しました（ラボでその設定を切り替えてキーを見つけて以来、`false` と書かれています：C4 を選ぶと `true` として現れ、C3 に戻すと `false` として残ります）。

Logic 自身の言葉はそのまま保ちます：調号は LCD の表示どおり（"C Major"）、出力はスロットの名前どおり（"Stereo Output"）、プラグインはそのスロットの名前どおりです。参照（ref）は Renoise と同じく、エポック付きの位置です（`<epoch>:<kind>:<n>`）：トラックは Tracks area での位置、リージョンはプロジェクト全体を通しての位置（トラックごとに順に）、マーカーはリストの中の位置、ノートはそのリージョンの下（`1:note:1.3`）、プラグインとセンドはそのトラックの下（`1:plugin:1.1`）、パラメータはそのプラグインの下（`1:param:1.1.7`）、Smart Control とパラメータのオートメーションはそのトラックの下（`1:smart:2.1`、`1:automation:1.1`）です。どのトラックも持たない Mixer の出力ストリップは、Mixer での順に `<epoch>:output:<n>`（Stereo Out は `1:output:1`）、そのプラグインは `<epoch>:plugin:0.<n>.<slot>`、そのプラグインのパラメータは `<epoch>:param:0.<n>.<slot>.<i>` です。エポックは、ブリッジがポーリングの合間に観測するトラックやリージョンの変更のたびに進みます（ヘッダーが現れた、消えた、名前が変わった、移動した。リージョンが追加された、削除された、移動した）。ノート、プラグイン、センド、マーカー、テンポでは進みません。前のエポックの ref、parent、around、cursor は、何のためのものであっても `stale` として拒否されます。ページング：`limit`（50、最大 200）、`cursor` `<epoch>.<n>`、そしてすべての答えに `nextCursor`、`total`、`epoch`。`ref` だけなら一行を読みます。`parent` は絞り込みます。`note` にはそのリージョンを `parent` として指定する必要があります。

### 一括読み取り（`snapshot {rows}`）

`snapshot {rows: [kinds], limit?}` は、指定した種類（`project`、`track`、`region`、`plugin`、`send`、`marker`、`tempo` のうち）のすべての行を、一回のリクエストで、一つのエポックの中でプロジェクトを一度見て読みます：`{epoch, rows: {<kind>: [rows]}, totals: {<kind>: n}, truncated?: [kinds]}`。各種類の行は、`read` がその種類についてページ単位で返すものとまったく同じ（同じ行が同じ順）で、`limit`（10000。これが上限）で切られ、その種類は `truncated` に挙げられるので、`read` でページ単位に読みます。`rows` がなければ、`snapshot` はこれまでどおりに答えます。トラック、プラグイン、センドを求められると Mixer を一度表示し（それらの種類、ルーティング、スロットは Mixer にあります）、マーカー、テンポ、拍子のためには List Editors を一度表示します（各タブを順に表示し、その後タブとペインを元に戻します）。一方、ページ単位の読み取りは List Editors をリストごとに一度表示します：Scratch では、キャッチアップが行う六つの読み取り（project、track、region、plugin、marker、tempo。1 ページ 200 行）に 4.4〜4.5 s かかり、そのうち Mixer が 0.7 s、Marker List が 1.2 s、Tempo List と Signature List が 2.4 s でした。同じ行の一括読み取りは 2.7 s でした。どちらも、Logic がまだ表示していないものを開きます：Mixer なしにストリップを、List Editors なしにリストを読める読み取りはありません。プロデューサーが開いているペインは開いたままです。答えは一つのフレームです（ワイヤーは 64 MB まで受け付けます）。

### 最近のプロジェクト

Logic は最近のプロジェクトを macOS の共有ファイルリストに保存していますが、macOS はそれをほかのプロセスに読ませません（"Operation not permitted"）。Logic の環境設定にもプロジェクトのパスはありません（ラボの Mac で 518 個のキー。`DefaultDir1` から `4` は、オープンパネルと保存パネルが最後に開いたフォルダーです）。File > Open Recent はそれらを表示し、アクセシビリティはメニューを開かずにその項目を読めますが、項目が持つのはプロジェクトのファイル名だけです（`AXTitle`。URL もドキュメントもヘルプもありません：Logic Pro 12.3.1 でアクセシビリティが示すのは、そのタイトル、識別子、フレームだけです）。そのため、開いているプロジェクトをパス全体で照合することはできません：開いているプロジェクトは、そのファイル名を持つ最初の項目で（メニューは最新のものから始まり、プロジェクトを開くとそこに置かれます）、`open` なのはその項目だけです。同じ名前のほかの項目（別のフォルダーにある同じ名前のプロジェクト）は `open` ではありません。`read {kind: "recent"}` は項目を順に答え、ブリッジが知り得るところでは `path` を付けます：いま開いているプロジェクト（そのウィンドウのドキュメント）と、開いているもの以外で、ほかのパッケージの中とゴミ箱を除いたところに、そのファイル名で Spotlight が一つだけ見つけるプロジェクトです。開いているもの以外に同じ名前の項目が二つあると区別できないので、どちらにもパスは付きません。Spotlight はブリッジのプロセスが読めるフォルダーについてしか答えないので、`~/Music`（Logic 自身のフォルダー）の下のプロジェクトは、開いているものでない限り見つかりません：ラボの Mac では、八つの最近のプロジェクトを、Scratch のパスはそのウィンドウから、七つのゲート用のコピーのパスは Spotlight から（コピーはホームの外の共有フォルダーにあります）、101 ms で読めました（二回目の読み取りは 66 ms）。

### キャッチアップの問い（M6）

Scratch のコピーに、Vocal の下に 38 のオーディオトラックを加えて計測しました（40 トラック。Keys とそのリージョンが最後）：

- **表示範囲の外のトラックとリージョンも読める**：ヘッダーのグループは、表示されているかどうかにかかわらず 40 個のヘッダーすべてをフレームとともに並べ（Tracks area が y 194〜1117 を表示しているときに、ヘッダー 1 は y -1420）、コンテンツはすべてのリージョンを並べます（トラック 40 にある Keys のリージョンは表示範囲の下）。**Mixer は表示したことのあるものしか描画しない**：Mixer はすべてのストリップを並べますが、表示範囲の右にあるストリップ（40 番目は x 1728、ペインの右端。その後に Stereo Out と Master）は、説明が数字（"224 40 4"）で、コントロールとスロットも汎用のもの（"output"、"input"、"audio plug-in"、それぞれバイパス）です。一方 Keys には Piano、Channel EQ、Compressor、ChromaVerb があります。左にスクロールして通り過ぎたストリップ（x -885 の Vocal）は正しく読めます。そのようなストリップへの `AXScrollToVisible` は応答されても何も動かさず、Mixer はアクセシビリティにスクロールバーを示しません。そのためブリッジは、描画されていないと判断したストリップ（説明が名前ではなく数字だけ）からはルーティング、プラグイン、センドを取らず、トラックの行は `stripUnread: true` を示します：スナップショットは、読めなかったストリップと空のストリップを区別できます。トラックの kind は、それでもスロットの形に従います（Keys は `instrument` と読めました）。
- **40 トラックの読み取り**：六つのページ単位の読み取りは 6.5 s（プロジェクト 0.3 s、トラック 1.4 s、リージョン 0.3 s、プラグイン 0.3 s、マーカー 1.5 s、テンポ 2.7 s）、一括読み取りは 3.3 s でした。
- **リージョンのノートは示されない**：Tracks area のリージョンの項目が持つのは、ヘルプテキストとサイズ（"4 bars"）のほかには、名前のフィールドと三つのハンドル（左端の移動、右端の移動、ループ）です。ノートの数やフィンガープリントはなく、それを与えるのは Event List だけです（リージョン一つにつき約 0.7 s、そのリージョンを選択して）。
- **`looped` はリージョンの幅にしか表れない**：ループさせても、Keys のヘルプテキスト（"starts at 1 bar and ends at 5 bars"）とサイズ（"4 bars"）は変わらず、項目の幅だけがループとともにプロジェクトの終わりまで伸びました（ループなしの 192 px に対して 13,421,293 px）。リージョンの読み取りはフレームを含まない安価な属性だけを求めるので、Tracks area からの `looped` は `false` のままです。選択中のリージョンについては、Region inspector の Loop チェックボックス（`region-loop`）がそれを示します。
- **Project Alternatives** は切り替えていません：File > Project Alternatives はメニューが開いて初めてサブメニューを埋め（その前はアクセシビリティにタイトルのない項目が一つ見えるだけです）、そこに届くキーコマンドもなく、ブリッジはメニューバーの項目を押しません（M3 のゲートで起きた固まり）。ウィンドウのドキュメントはパッケージ（`…/Scratch M6.logicx/`）で、オルタナティブはその中のフォルダー（`Alternatives/000`、…）なので、id（ドキュメントのパスのハッシュ）は、ドキュメントがパッケージである限りパッケージのものです。切り替えは観察していません。

## フォーカスとボディ

- `focus`：`{epoch, view, editor, selection, playhead, transport, changed, ageMs}`。ランループが 150 ms ごとに四つの小さな読み取り（control bar、トラックヘッダー、リージョン、List Editors のタブ）で更新するキャッシュから答え、ウィンドウを走査して答えることはありません。`view` は `tracks` です。`editor` は、List Editors が Event タブを表示しているときは `eventList`、Mixer ペインが開いているときは `mixer`、エディターのペインがそのどちらかを表示しているときは `pianoRoll` か `stepSequencer`、それ以外は `null` です。`selection` は `trackRef` と `regionRef`（ないときは `null`）を持ち、その横に、ランタイムが読む、選択中のトラックの `track: {name, kind}` と選択中のリージョンの `regionName` を持ちます。`transport` は `{playing, recording, cycle, tempo, beat, beatsPerBar}` です。Logic が作業を受け付けられない間は、status のコードとして `busy` が加わります。
- `focus_body`：`tree`（ストリップ：name、kind、volumeDb、pan、mute、solo、armed、inputs、output、`pluginRef` 付きの inserts、`sendRef` 付きの sends）、`notes`（リージョンのノート。ページ単位で、`startBeats`、`endBeats`、`name`、`timeSignature` 付き）、`timeline`（拍の範囲の中で、トラックをレーンとしてそのリージョンとともに示す。レーン単位でページ分けし、`beatsPerBar`、`playheadBeats`、`playing`、それまでに読んだマーカー付き）、`routing`（inputs、sends、output）。ボディは `ref` でトラックかリージョンを指定し、なければ選択中のものを使います。

## イベント

`project`（パスまたは identity が変わった）、`transport`（変わるたびの control bar の状態。Mackie の LED からは即座に：`{playing, recording, cycle, playhead, tempo, timeSignature}`）、`tracks`（エポックが進んだときに `{epoch}`：ヘッダーが現れた、消えた、名前が変わった、移動した、リージョンが追加された、削除された、移動した）、`selection`（フォーカスの選択とその `epoch`。変わったとき）。

## M2 がそのままにしていること

- 外からトラックを選択できるのは、Mackie Control のセレクトボタンを通じてだけです：ヘッダーの `AXSelected` は設定できず、その "Has Focus" ボタンは押せず、Tracks area はアクセシビリティからキーボードフォーカスを受け取りません。セレクトがそのトラックを意味するには、サーフェスが Pan ビューになっている必要があります。Logic はサーフェスを初めて追加するときに Control Surface Setup ウィンドウを開き、そのウィンドウがメインの間、ブリッジはプロジェクトのウィンドウをタイトルで読みます。
- Logic がボタン（センドスロット、インサートスロット、出力スロット）から開くポップアップメニューは、出ている間アクセシビリティの呼び出しを止めたままにし、メニューの要素を示しません。M4 の第 2 スライスで、それでもそのメニューが Logic に送ったキーを受け取ることがわかりました（「Logic が REAPER との違いを強いるところ」を参照）。
- リストのテキストセル（マーカーの名前）はテキストフィールドではありません：マーカーの名前は外から変えられません。Event List のリージョンレベルにあるリージョンの名前はテキストフィールドです。
- スクラバーとスライダーは、値の設定では 1 ずつ、インクリメントでは 10 ずつ動きます（テンポ、再生ヘッドの小節、リストの位置のセグメント）。M3 の変更はこれを計算に入れなければなりません。

## ヘルパーのプロトコル（`kumi-logic-hands`）

ブリッジはヘルパーを子プロセスとして実行し、その stdin と stdout で JSON lines をやりとりします：一行に一つのリクエスト、一行に一つの返答、その合間にイベント。リクエストは `{"id": n, "op": "…", …}`、返答は `{"id": n, "ok": true, "ms": n, …}` または `{"id": n, "ok": false, "error": "<code>", …}`、イベントは id のない `{"event": "…", …}` です。リクエストは一度に一つ：ヘルパーは 2 s 以内に答えるか、`timeout`（作業は始まっていた）または `busy`（前のリクエストがまだ実行中）で答えます。ログは stderr にだけ出し、頼まれない限りアプリをアクティブにしません。正確なリファレンスはヘルパーの README（`crates/logic-kumi/helper/README.md`）で、この表はその要約です。

| 操作 | パラメータ | 返答 |
| --- | --- | --- |
| `trusted` | `prompt`（bool、デフォルト false） | `trusted` |
| `apps` | `bundles`（id） | `apps: [{bundle, pid, name, active, hidden}]`。その後、それらのバンドルについての `app` イベント |
| `processes` | `contains`（実行ファイルのパスの部分文字列） | `pids`（昇順） |
| `activate` | `pid` | `active` |
| `windows` | `pid`、`menu`（Window メニューのタイトル。デフォルトは "Window"） | `windows: [{title, role, subrole, main, focused, document, minimized, fullscreen, modal}]`、`listed`（Window メニューの最後の区切り線より後のタイトル。ウィンドウでない項目を含むこともある） |
| `tree` | `pid`、`root`、`path`、`depth`（1）、`children`（100）、`attrs`（`set` と `act` 以外のすべて）、`popups`（false） | `node`、`truncated` |
| `get` | `pid`、`root`、`path`、`attrs`、`popups` | `attrs: {"AXName": value}`、`truncated` |
| `set` | `pid`、`root`、`path`、`attr`、`value`、`settleMs`（50）、`popups` | `result`、`before`、`after`。AX の失敗は `ok: false` で、AXError の名前を `error` とし、その三つのフィールドは残す |
| `action` | `pid`、`root`、`path`、`name`、`settleMs`、`popups` | `set` と同じ |
| `menu` | `pid`、`titles`（メニューバーから下へ） | `result`、`title`、`active`。`no-menu-item` は見つからなかった最初のタイトルを示す |
| `keys` | `pid`、`key`、`mods`（`cmd`、`shift`、`opt`、`ctrl`） | `posted`、`active` |
| `type` | `pid`、`text`（最大 200 文字） | `posted`、`active` |
| `click` | `pid`、`x`、`y`、`mods`、`count`（1） | `posted`、`active` |
| `midi.open` | `name`、`uid`（int、またはハッシュして int にする文字列） | `opened`、`source`、`destination`。開いている間は `midi-open` |
| `midi.send` | `hex` | `sent` |
| `midi.close` | | `closed`（冪等） |
| `tap.start` | `pid`、`path`（新しい WAV ファイルのフルパス）、`mute`（false）、`maxSeconds`（600、最大 3600） | `sampleRate`、`channels`、`hostTime`。タップの実行中は `busy`、`denied`（システムオーディオ録音の権限がない）、`unsupported`（macOS 14.2 より前）、`step` と `detail` 付きの `failed`（プロセスがまだ音を出していない間は `step: "process"`） |
| `tap.status` | | `running`、`frames`、`seconds`、`truncated` |
| `tap.stop` | | `file`、`frames`、`seconds`、`sampleRate`、`channels`、`peak`、`rms`（リニア）、`anchors`（`[frame, hostTime]`）、`timebase`（`{numer, denom}`）、`dropped`、`truncated`。タップが動いていないときは `failed` |
| `quit` | | `ok` の後に終了 |

エラーコード：`malformed`、`unknown-op`、`bad-request`、`untrusted`、`no-process`、`no-app`、`timeout`、`busy`、`popup`、`unknown-attr`、`no-menubar`、`no-menu-item`、`disabled`、`unknown-key`、`unknown-mod`、`too-long`、`bad-hex`、`midi-closed`、`midi-open`、`denied`、`unsupported`、`failed`、そして読み取り、書き込み、アクションの失敗には AXError の名前。

`root` は `"app"`、`"main"`（`AXMainWindow`）、`"focused"`（`AXFocusedWindow`）、`"menubar"`、`{"title": "substring"}` のいずれかです。`path` は、その root からの子のインデックスをドットでつないだものです。`depth` と `children` は `tree` の範囲を制限します。`attrs` は欲しい属性を指定します（すべてなら `"all"`。ノードのフィールド名か AX の名前で）。`popups` が false ならポップアップボタンの子は決して読まず、ポップアップボタンを通るパスは `popup` で拒否されます。

ノードは `{i, role, subrole, title, desc, value, valueDesc, help, id, enabled, selected, pos: {x, y}, size: {w, h}, set: [attributes], act: [actions], c: [nodes], more: n}` です。空のフィールドは省かれます。`more` は `children` を超えた子の数、または深さの上限ではすべての子の数です。値は JSON（数値、文字列、真偽値）で、点、サイズ、矩形、範囲はオブジェクト、そのほかの要素は `{"ref": "role title"}` です。

イベント：MIDI デスティネーションに届いたバイトには `{"event": "midi", "hex": "…", "ms": n, "hostTime": n, "timeStamp": n}`（長い SysEx はいくつかに分かれて来ることがあります。`hostTime` はヘルパーがそれを受け取った時刻、`timeStamp` はパケット自身のホストタイムスタンプで、送り手が付けなかったときは 0）。最後の `apps` リクエストで指定したアプリには `{"event": "app", "state": "launched" | "terminated" | "activated" | "deactivated", "pid": n, "bundle": "…"}`。

プロセスタップ（`tap.*`）は、プロセスの出力をステレオにミックスダウンして、ヘルパー自身が書く WAV ファイルに記録します：フォーマット 3（IEEE float）、32-bit、タップ専用のアグリゲートデバイスのレート（デフォルトの出力デバイスがクロック。別のレートで再生しているプロセスはそのレートにリサンプリングされる）で、ヘッダーのサイズは毎秒と最後に書き直されます。`mute` は、タップを読んでいる間、プロセス自身の出力がスピーカーから出ないようにします。`anchors` は、ファイルのフレームと、そこから始まるバッファに Core Audio が与えたホスト時刻（`mach_absolute_time`。CoreMIDI がパケットに刻むのと同じクロック）の組です：最初のバッファ、少なくとも 4 分の 1 秒ごとに一つ、欠落の後に一つ、そして最後のもの。ホスト時刻 `t` は、`t` と同時かそれより前のアンカー `(f, h)` から、フレーム `f + (t - h) × numer / denom / 10⁹ × sampleRate` に当たります。`dropped` は失われたフレームを数えます。`maxSeconds` に達したキャプチャは書き込みをやめ、`truncated` を示します。EOF、`quit`、SIGTERM は、実行中のタップのファイルを仕上げます。タップとそのアグリゲートデバイスはヘルパー専用で、ヘルパーとともに消えます。

## 変更（`change_preview`、`change_apply`、`change_restore`）

M3 で Kumi はプロジェクトを変更し、取り消せるようになり、M4 は下の表の `key-command` から下の種類と、`project_save`、そして REAPER の `run_action` と同じ形の `key_command` を加えます。ワイヤーは REAPER 拡張のものと同じです：ホストのジャーナル（`crates/daw-mcp-server/src/changes.rs`）が変更をプレビューし、保持し、一つの名前付きステップ（`desc`、`Kumi: … (k<nonce>)`）で適用し、別のステップとして取り消します。ブリッジは `change_preview`、`change_apply`、`change_restore` に、REAPER のものと同じフィールドで答えます。各種類は一つの macOS のチャンネルを通じて書き込み、値を読み戻します：`applied` は Logic が新しい値を示したときだけで、そうでなければ `uncertain` です。プレビューは何も書き込まず、参照のエポックと、いま読んだ対象の値をフェンスします。適用は書き込む前にフェンスを確かめ直し、合わなければ `changed` で拒否します。復元は、値を設定できるときは値を元に戻し、そうでなければ Logic 自身の取り消しに頼ります。ダイアログが出ていれば `busy`、アクセシビリティの許可がなければ言葉で理由を述べる `failed`（busy にはなりません）です。計測したのは Logic Pro 12.3.1 の Scratch プロジェクトで、ホストがシミュレートする Logic（`simulator/logic.rs`）も同じ形で答えます。

### 種類、そのチャンネルと読み戻し

| ワイヤーの種類 | ツール | チャンネル（書き込み） | 読み戻し元 | Logic の取り消しステップ |
| --- | --- | --- | --- | --- |
| `track-volume` | `track_volume` | ヘッダーの Volume スライダー。dB のテキストに合うまでステップ | スライダーの `AXValueDescription` | なし |
| `track-pan` | `track_pan` | ヘッダーのパンスライダー（0–127、64 が中央） | スライダーの値 | なし |
| `track-mute` | `track_mute` | ヘッダーの Mute チェックボックスのフレームをクリック | チェックボックス | なし |
| `track-solo` | `track_solo` | ヘッダーの Solo チェックボックスをクリック | チェックボックス | なし |
| `track-arm` | `track_arm` | ヘッダーの Record Enable チェックボックスをクリック | チェックボックス | なし |
| `track-rename` | `track_rename` | Inspector の "Track:" フィールド、`AXConfirm` | ヘッダーのラベル | "Renaming" |
| `track-add` | `track_add` | Track メニュー（New Audio/Software Instrument/External MIDI Track） | 新しいヘッダー | "Create Track" |
| `track-delete` | `track_delete` | Track > Delete Track。そのシートに Delete で答える | ヘッダー | "Delete Tracks" |
| `marker-add` | `marker_add` | 再生ヘッドをその位置に動かし、Marker List の Create new Marker | リストの行 | "Create Marker" |
| `marker-move` | `marker_move` | 行の Position スクラバー。セグメントを一つずつ | リスト | "Value Change" |
| `marker-delete` | `marker_delete` | 行の Delete アクション | リスト | "Delete" |
| `tempo-set` | `tempo_set` | control bar の Tempo スライダー（整数の BPM） | LCD のテンポ | なし |
| `notes-add` | `notes_add` | Event List の Create new Event。そのセルをステップ | リストの行 | "Create Event"/"Value Change" |
| `notes-change` | `notes_change` | ノートの行のセル（ピッチ、ベロシティ、長さ、チャンネル、位置） | リスト | "Value Change" |
| `notes-delete` | `notes_delete` | 行の Delete アクション | リスト | "Delete" |
| `fx-bypass` | `fx_bypass` | Mixer のストリップのスロットのバイパスチェックボックス | スロット | なし |
| `key-command` | `key_command` | キーコマンドのキーの組み合わせ。名前または id で `com.apple.logic10.plist`（なければ工場出荷時の U.S. セット）から引き、Logic を前面にして送る | Edit メニューの Undo のタイトル（動いたかどうか） | コマンド自身のもの、またはなし |
| `region-move` | `region_move` | リージョンレベル（Leave Folder）の Event List、リージョンの Position スクラバー | Tracks area の項目 | "Value Change" |
| `region-resize` | `region_resize` | 再生ヘッドを新しい端に置き、Set Region Start/End to Playhead（Cmd-[ / Cmd-]） | Tracks area | "Set Start to Playhead Position"、"Set End to Playhead Position"（端ごとに一つ） |
| `region-split` | `region_split` | リージョンをクリックし、再生ヘッドを分割位置に置き、Split at Playhead（Cmd-T） | Tracks area：右側の部分が `created` | "Split Regions" |
| `region-loop` | `region_loop` | Region inspector の Loop チェックボックス | チェックボックス（Tracks area は `looped` を決して示さない） | "Loop Parameter Change" |
| `region-rename` | `region_rename` | Region inspector の名前のフィールド、`AXConfirm` | Tracks area | "Renaming" |
| `region-delete` | `region_delete` | リージョンをクリックし、Delete キー | Tracks area | "Delete" |
| `loop-range` | `loop_range` | LCD を Custom にし、その二つのロケーターグループのスライダー、Cycle ボタン | ロケーターとボタン | なし |
| `notes-quantize` | `notes_quantize` | Event List：Edit > Select > All、ヘッダーの Quantize ポップアップ | グリッド上の行 | "Quantize" |
| `midi-import` | `midi_import` | `/tmp/kumi-logic` の下のフォーマット 0 のファイル、File > Import > MIDI File…、パネルの「フォルダへ移動」 | 新しいリージョンとトラック | "Import" |
| `tempo-marker-set` | `tempo_marker_set` | Tempo List：再生ヘッドの位置で Create new Tempo Event、Tempo セルのセグメント | リスト | "Insert Tempo" + "Value Change" |
| `tempo-marker-delete` | `tempo_marker_delete` | 行の Delete アクション | リスト | "Delete" |
| `signature-set` | `signature_set` | Signature List：Create new Time/Key Signature Change、分子のスライダー、分母のポップアップ | リスト | "Value Change" |
| `track-stack` | `track_stack` | ヘッダーをクリック（ほかのヘッダーは Shift を押しながら）、Create Summing/Folder Stack（Cmd-Shift-G/H） | ヘッダー：スタックの開閉用の三角形 | "Create Track Stack" |
| `track-stack-flatten` | `track_stack_flatten` | スタックのヘッダーをクリックし、Flatten Stack（Cmd-Shift-U） | ヘッダー | "Flatten Stack" |
| `fx-add` | `fx_add` | Mixer：ストリップの最後のエフェクトの下の空きスロット（または、前に入れるエフェクトの上端に沿ったインサートバー）を押し、そのメニューの検索にキーを一つずつ打ち、Down、Return。Logic が開くプラグインウィンドウは閉じる | ストリップのスロット：求めた名前がその位置にあること | "Insert Plug-in in Channel Strip"（空きスロット）、"Change Plug-in in Channel Strip"（インサートバー） |
| `fx-delete` | `fx_delete` | Mixer：プラグインのリストボタンを押し、No Plug-in（Down、Down、Return） | ストリップのスロット | "Remove Plug-in in Channel Strip" |
| `fx-param` | `fx_param` | プラグインウィンドウ（スロットの開くボタン）、View > Controls、パラメータのスライダーを設定（正確に）、またはその選択肢を押すかスイッチを切り替える。ビューを元に戻し、ウィンドウを閉じる | Controls View の値とテキスト | なし（プラグインウィンドウ自身の取り消しにある） |
| `fx-params` | `fx_params` | `fx-param` と同じ。一つのウィンドウで複数 | Controls View | なし |
| `fx-preset` | `fx_preset` | エディター表示のプラグインウィンドウ（ヘッダーにボタンが出るまで大きくする）：次または前の設定（setting）ボタンを `step` 回、または設定メニューが `preset` を示すまで次へ | 設定ポップアップの名前 | なし（プラグインウィンドウ自身の取り消し。復元はそれを押す） |
| `patch-load` | `patch_load` | トラックを選択し、Library（Y）：その検索フィールドにパッチの名前を設定し、結果の行を選択。検索をクリアし、Library を閉じる | Inspector のストリップの設定ボタン（パッチの名前）とそのプラグイン | "Load Patch" |
| `send-add` | `send_add` | Mixer：ストリップの空きセンドスロットを押し、Down、Down（Bus）、Right、そのバスまで Down、Return。その後レベルを `send-volume` と同じようにステップ | ストリップのセンド："Bus n" という名前のグループ（と、Logic が作った AUX。`created.aux` の下） | "Change Send in Channel Strip" |
| `send-remove` | `send_remove` | Mixer：センドのリストボタンを押し、No Send（6 番目の項目） | ストリップのセンド | "Change Send in Channel Strip" |
| `send-volume` | `send_volume` | Mixer：センドのノブ。粗くはインクリメントで、細かくは値の設定で（0 dB 付近では 0.1 dB） | ノブのテキスト | なし |
| `smart-control-set` | `smart_control_set` | トラックを選択し、Smart Controls ペイン（B）：コントロールのスライダー。10 分の 1 単位はインクリメントで、100 分の 1 単位は値の設定で | スライダーの値とテキスト | なし |
| `automation-add` | `automation_add` | トラックを選択し、その Track Automation Event List（コマンド 1032、Control-Command-E）：イベントの種類を Fader にし、再生ヘッドを各イベントの拍に置き、Create new Event、新しい行の Num を Length/Info セルがそのパラメータを示すまで（または求めた番号まで）ステップし、Val をステップ。種類、再生ヘッド、選択を元に戻し、リストを閉じる | リストの行 | "Create Event"/"Value Change" |
| `automation-change` | `automation_change` | リスト：イベントの行を削除し、求めた位置と値で作り直す | リスト | "Delete"、その後 `automation-add` と同じ |
| `automation-delete` | `automation_delete` | リスト：行の Delete アクション | リスト | "Delete" |
| `metronome-set` | `metronome_set` | control bar の Metronome Click チェックボックス。求めた状態と違うときだけ押す | チェックボックス | なし |
| `sample-load` | `sample_load` | パネルサービスを通じた File > Import > Audio File… で、再生ヘッドの位置のオーディオトラックへ。その後 Track > Convert Regions to New Sampler Track…（そのシート：zones from Regions、Sampler、名前、OK） | 新しいヘッダーと、Inspector のそのストリップ（インストゥルメントスロットのプラグイン） | "Insert"、その後 "Convert Regions to New Sampler Track" |

ヘッダーのコントロール（ボリューム、パン、ミュート、ソロ、アーム）と Mixer のスロットのバイパスは、トラックが選択されていなくても、Logic が前面になくても書き込めます。ミュート、ソロ、アームのチェックボックスは `AXPress` も値の設定も受け付けず、受け付けるのはそのフレームへのクリックだけで、クリックは Logic を先に前面に出してから、システムのイベントタップに送ります。名前のフィールド、Track メニュー、リストへの押下は、Logic が前面のアプリである間しか効かないので、これらの種類は Logic をアクティブにし、ほかのアプリが前面を保ったままなら `busy` で拒否します。リストの読み取りはそのペインとタブを開き、元に戻します。マーカーは再生ヘッドの位置に作られ、再生ヘッドはそこへ動かしてから元に戻します。

`transport_play`、`transport_stop`、`cursor_set`（ツールの `play`、`stop`、`set_cursor`）は、control bar の Play ボタンと Stop ボタン、LCD の小節と拍のスライダーを使います。どれも Logic の取り消しのステップにはならず、`cursor_set` は `positionBeats` または `position`（プロジェクトの一定のテンポでの秒数）を受け付けます。再生ヘッドは、どこに置くときも（`cursor_set`、そしてマーカー、分割、リサイズの端、テンポや拍子のイベント、インポート、オートメーションのイベントを作る位置）、求めた拍にちょうど置かれます：LCD の小節と拍のスライダーは再生ヘッドが持っていたディビジョンとティックを保ち、停止はそれらを任意の値のまま残します（停止の後、Logic は 2.1.3.162 から再生し、小節 3 で求めた分割はそれより後ろになりました）。そのため、ブリッジが最後に置いてから Logic が再生または停止したとき、あるいは Mackie Control のタイムコードが 1 を超えるディビジョンかティックを示すときは、まず Stop ボタンで再生ヘッドをプロジェクトの先頭に移します。Stop ボタンは、Logic が先頭以外で停止している間は "Go to Beginning" と表示されます（先頭では "Stop" と表示され、再生ヘッドはすでに拍の上にあります）。その後、小節と拍をステップします。`undo_step_begin`/`undo_step_end` は数えるだけです（Logic には取り消しのブロックがありません）：`undo_step_begin` は `{stepId: "sN"}` を、`undo_step_end` は `{desc, entry: false, changes: n}` を答え、プランの各変更はそれぞれ Logic の取り消しの独立したステップのままです。

`bounce {epoch, startBeats, endBeats, path, format?}` と `export_tracks {epoch, path}` はアクションで、`project_save` と同じくブリッジのメソッドです（変更でも行でもなく、取り消しのステップもありません）。`bounce` は File > Bounce > Project or Section… を使います：Logic の Bounce ダイアログで Uncompressed だけにチェックを入れ（その行を選択するので、File Type と Bit Depth のポップアップはその行のものになります）、フォーマットのファイルタイプとビット深度（デフォルトは `wav24`、ほかに `wav16`、`wav32f`、`aiff24`）を選び、Normalize を Off にし、Add to Project と Add to Apple Music Library のチェックを外し、Start と End の位置のセグメントをステップし、OK。続いて保存パネルをそのサービスを通じて操作し（フォルダーは「フォルダへ移動」のシートで、そして名前）、ディスク上にファイルができるのを待ってそのヘッダーを読みます（`{epoch, bounced, format, bytes, seconds, rate, channels, bits, startBeats, endBeats}`。長さがプロジェクトのテンポでの範囲の長さと合わなければ `uncertain`）。`path` は、まだ存在しない `.wav`（または `.aif`）へのフルパスで、そのフォルダーは存在している必要があります。ダイアログでチェックされていた Add to Project は、ファイルをプロジェクトのオーディオに入れ、それは Kumi がそれまでに行ったことの上に積まれる Logic の取り消しのステップ（"Bounce Project"）になります：そのとき、Logic 自身の取り消しで戻そうとした Kumi のステップ（分割、パッチ）は拒否され、その Edit メニューには "Undo Bounce Project" と表示されていました。チェックを外せば、バウンスはプロジェクトの何も変えず、Logic の取り消しもそのまま残します（分割、バウンス、その後 Logic の取り消しで分割を取り消す：記録済み）。ダイアログは、フォーマットと同じく、これらの選択を次のバウンスのために保ちます。バウンスには Logic が鳴らすものすべてが入り、control bar の Metronome Click がオンの間はメトロノームのクリックも入ります（リージョンが何も鳴らない Scratch の小節 6 と 7 は、クリックをオンにしてバウンスするとピーク 0.42 でした）：`bounce` はクリックをプロデューサーが設定したままにし、`render`（M5。下の「聴き取り」）はそのバウンスのためにクリックをオフにします。`export_tracks` は File > Export > All Tracks as Audio Files… を使います。これはフォルダーを受け付けるオープンパネルです（そのオプションはプロデューサーが設定したままにします）：フォルダー（存在している必要があります）へは「フォルダへ移動」のシートで移動し、Export を押し、答えには Logic がそこに書いたファイルが並びます（`{epoch, folder, files: [{name, bytes, seconds}]}`）。ブリッジがディスク上で見るものは一か所の継ぎ目（`src/disk.rs`）を通るので、記録したシーンにはそれが残り、リプレイはそれをもとに答えます。

`project_save {epoch, path?, version?}`（ツールの `save_project`）は REAPER と同じ形です：`path` がなければ Logic の Save キーコマンド（Cmd-S）でその場に保存し、ウィンドウのクローズボタンが保存するものがないことを示しているときは、すぐに `dirty: false` で答えます。`path`（まだ存在しない `.logicx` へのフルパス）があれば、macOS のパネルサービスを通じた File > Save As…（「フォルダへ移動」のシート、名前のフィールド、Save）で、以後プロジェクトはそのファイルになります。ウィンドウのドキュメントとその編集済みのフラグから読み戻します。Kumi から見えるファイルの上には決して保存せず、プロジェクト自身のファイルをパスにすることもなく（`bad-params`）、存在しないフォルダーにも保存しません（パネルの "where" ポップアップがそのフォルダーを示さなければ `bad-params` で、パネルはキャンセルされます。Logic 自身は、パネルに表示した名前の空の `<name>.logicx/Alternatives` パッケージを残します）。`version` は REAPER のもので、拒否されます。保存は Logic の取り消しのステップにはなりません。答えは `{saved, dirty}` です。

### フェンスと結果

- ヘッダーのプロパティは現在の値を、名前の変更は名前を、トラックの削除は `{name, index, regions}` を、トラックの追加は `{tracks, names}`（数とすべての名前）を、マーカーはその状態全体か数を、テンポは BPM を、ノートはリージョンのノートの数とその値のハッシュをフェンスします。リージョンの種類はリージョンの `{startBeats, endBeats, name, track, looped}` とリージョンの数を、`loop-range` はサイクルの `{on, startBeats, endBeats}` を、`notes-quantize` はノートの数とハッシュを、`midi-import` と `track-stack` は `{tracks, names, regions}` を、テンポイベントや拍子はその行（`exists`、`bpm` または `timeSignature`）とリストの数を、`key-command` はコマンドの id と名前を、`metronome-set` はクリックの `{on}` をフェンスします。
- `key-command` は REAPER の `run_action` と同じように答えます（ツールは `key_command`）：`applied: {command, name, changed}`。Undo のタイトルが動かなかった（Logic がステップを作らなかった）ときは `display.unchanged` と `undo.entry: false` を付けます。動いたときは、そのステップを Logic 自身の名前のまま保持し、その復元は `daw-only`（Logic で Cmd-Z）です。Save、Save As、Undo、Redo、Quit、プロジェクトのコマンド、そして名前が "…" で終わるすべてのコマンド（ダイアログ）は拒否されます。
- `region-split` は `applied: {left, right}` と `created: {ref}`（右側の部分。新しいエポックで）を答えます。`midi-import` は `applied: {notes, startBeats, endBeats, track, trackName, file}` と `created: {ref, track, trackAdded}` を答え（Logic は MIDI ファイルを、指定したトラックの中ではなく、その下の新しいトラックにインポートします）、Event List がファイルのノートより少ない行しか並べないときは `rows` と注記を付けます（Event List が並べるのは画面上の行です。残りも入ったことはリージョンの長さが示します）。`track-stack` は `applied: {index, name, kind, tracks}`（サブトラックの参照）と `created: {ref, stack}` を答えます。`tempo-marker-set` と `signature-set` は、行を作ったときに `made: true` を答えます。
- `fx-add` は `applied: {index, name, slot, plugins}`（変更後のストリップのエフェクト。上から順）と `created: {ref, kind: "plugin"}` を答え、そのフェンスはストリップの `{plugins, instrument}` です。`fx-delete` は `applied: {deleted, plugins}` と `material: {name, index, strip, step}` を答え、インストゥルメントは拒否します。`fx-param` と `fx-params` は `{plugin, params: [{index, name, value, text}]}` でフェンスし、それで答えます。`fx-preset` は `material: {steps}` とともに `{plugin, preset, steps}` を答えます。`send-add` は `applied: {destination, slot, level}` と `created: {ref, kind: "send", bus, aux?}` を、`send-remove` は `material: {destination, level, slot, step}` を、`send-volume` は `applied: {level}` を答えます。`patch-load` は `{patch, plugins}`（ストリップの設定の名前とプラグイン）でフェンスし、それで答えます。`smart-control-set` は `{name, value, text}` でフェンスし、それで答えます。オートメーションの種類は `{ref: track, parameter: <name as the list's Length/Info cell writes it, "Volume"> | <its number>}` を受け取り、`events: [{positionBeats, value}]`（`automation_add`。整数の拍、値は 0〜127）、`changes: [{index, positionBeats?, value?}]`（`automation_change`）、または `indexes: [...]`（`automation_delete`）を付けます。イベントは、読み取りが並べる順で、そのパラメータのイベントの中での 0 からの位置で指定します。これらはトラックのオートメーション全体 `{events: [{positionBeats, value, number, parameter}]}` をフェンスし、`applied: {made, removed, events}` を答えます。
- `sample-load`（`sample_load {kind: "sampler", file, ref?, positionBeats?, name?}`）は、Kumi から見えるオーディオファイル（wav、aif、aiff、aifc、caf、mp3、m4a）のフルパス、それを入れるオーディオトラック（`ref`。なければ選択中のトラックがオーディオトラックならそれ、そうでなければ最初のオーディオトラック。ほかの種類のトラックなら `bad-params`）、位置（`positionBeats`、整数の拍。なければ再生ヘッド）、インストゥルメントの名前（なければ拡張子を除いたファイル名）を受け取ります。`{tracks, names, regions}` をフェンスし、`applied: {file, kind, track, name, instrument, startBeats, endBeats, sourceTrack, answered}`（読み戻した新しいトラックの位置、名前、インストゥルメント、そのトリガーリージョンの範囲、オーディオトラックの位置）と `created: {ref, kind: "track", region, source}`（新しいトラック、そのトリガーリージョン、ミュートされたオーディオリージョン。新しいエポックで）を答えます。`kind: "quick-sampler"` は拒否されます：Logic がオーディオリージョンを変換できるのは Sampler、Alchemy、Drum Machine Designer のトラックだけで（シートにある三つのインストゥルメント）、Quick Sampler はそのウィンドウへのドラッグでサンプルを受け取るからです。
- `metronome-set`（`metronome_set {on}`）は、対象を `{ref: null, kind: "project"}` として、クリックを `{on}` でプレビューし、フェンスし、答えます。すでに求めたとおりのクリックは `display.unchanged`（ステップも行もなし）、control bar に表示されていないクリック（Customize Control Bar and Display で外されたもの）は `failed` です。その復元は、値によって押し戻します。
- `applied` は、書き込みの後に Logic が示す値を持ちます。Logic が読み戻しで示さない書き込みは `uncertain`（"Logic Pro shows … after Kumi asked for …"）で、ステップに名前を付けません。適用の `undo` は `{desc, entry}` で、Logic の履歴がステップを取ったときは `title`（適用時の Edit メニューの Undo のタイトル）と `entry: true` を、取らなかったときは `entry: false` を持ちます。`track-add` は新しいエポックと `created: {ref: "<new epoch>:track:<n>"}` を答えます。`track-delete` は `{name, kind, index, volumeDb, pan, mute, solo, arm, step}` を収めた `material` を答えます（`step` は適用の `desc`）。
- 値を設定できるもの（すべてのヘッダーのプロパティ、テンポ、マーカー、ノート、バイパス）の復元は値によるもので、Logic の取り消しの新しいステップになり、`undo: {desc, entry: false}` と答えます。削除したトラックは値によって作り直せないので、その復元は Logic 自身の取り消し（Edit メニューからの Cmd-Z）で、その削除がまだ Logic の一番上の Undo ステップである間だけ送ります（監視は、変更の種類と対象の組、そしてステップの `desc` でそれを追います）。そのときは `restored`、`undo: {desc, entry: false, how: "undo"}`、Logic 自身の取り消しで戻したという注記を答え、そうでなければ `failed`（"Logic Pro's undo has other steps on top of this one …; Cmd-Z in Logic Pro can, step by step"）です。`marker-delete` の復元は、そのステップが一番上にある間はまず Logic 自身の取り消しを使い、そうでなければマーカーを値によって作り直します（番号は Logic が自分で付けます）。M4 の種類のうち、移動、リサイズ、ループ、名前の変更、サイクル、クオンタイズ、テンポイベント、拍子は値によって戻ります。分割、リージョンの削除、インポート、フラット化したスタック、削除したテンポイベントは、一番上にある間に Logic 自身の取り消しでしか戻りません（`undo: {how: "undo"}`）。Kumi が作ったスタックは、そのステップが一番上にないときは値によってもう一度フラット化されます。第 2 スライスの種類のうち、追加したプラグインは値によって外され、削除したプラグインは一番上にある間は Logic 自身の取り消しで（設定ごと）、そうでなければ名前で（Logic のデフォルトの設定で。インストゥルメントは削除されません）戻ります。追加したセンドは一番上にある間は Logic 自身の取り消しで（Logic が作った AUX も一緒に）、そうでなければ値によって（No Send。AUX は Mixer に残ります）戻り、外したセンドも同様です。パッチは一番上にある間に Logic 自身の取り消しでしか戻りません。パラメータ、センドのレベル、Smart Controls は値によって戻ります。プリセットはプラグインウィンドウ自身の Undo で、Kumi がステップした回数だけ押して戻します。オートメーションのイベントは値によって戻ります（Kumi が作ったイベントは削除し、Kumi が削除したイベントは作り直します）。
- 復元の参照は変更自身のエポックのもので、トラックやリージョンが今ある位置まで運ばれます：ブリッジはエポックが進むたびに（ヘッダーの追加、削除、名前の変更、移動。M2 のオブザーバーのルール）それを見て、進むごとに各位置がどこへ行ったかを保持するので、トラックを追加または削除した後にフェーダーや名前の変更を取り消しても `stale` とは**なりません**。プレビューと適用は、前のエポックの参照を引き続き `stale` として拒否します。

### 取り消しのスタックと `undone_in_daw`

Logic は拡張機能に自身の取り消しについて何も伝えず、その Undo History は外から読めません。また、Edit メニューの Undo と Redo のタイトルが一番上のステップの名前を示すのは、Logic がそのメニューを検証（validate）した後だけで、Logic 自身のキーコマンド（プロデューサーの Cmd-Z もその一つ）はその検証をしません。そのためブリッジは、Kumi のステップのスタックを Logic の履歴が保持する順に持ち、それぞれに、適用時に Logic が付けたタイトル、ホストが送った `desc`、そして（フォーカスのティックがどのみち読むものについては）プローブを付けます：ヘッダーのミュート、ソロ、アーム、ボリューム、パン、名前、トラックの名前、またはテンポです。ティックのたびに（フォーカスのティック。メニューバーは Logic をアクティブにせずに読みます）、二つの方向から見ます：

- プローブ：値が Kumi の変更前の値に戻っていれば、その変更は手で取り消されたもの（`how: "undo"`）、Kumi が設定した値に戻っていればやり直しです。これらはタイトルが何を示していても即座に発火します。
- タイトル（そのままの状態で読む。約 0.6 ms）：Redo のタイトルがあるステップの名前になれば、そのステップとその上のすべてのステップが取り消されたもの、Undo のタイトルがその名前に戻ればやり直しです。プローブがまだ適用済みを示しているステップは、タイトルだけでは取り消されたとみなしません。Logic のタイトルには nonce がなく、プロデューサー自身のステップが同じ名前を持つこともあるからです。ティックが自分でタイトルを検証することはありません：タイトルが更新されるのは、Logic が前面にある間の Kumi 自身の書き込みの後だけで、Logic が使っていないキーの組み合わせ（key equivalent。何も開きません）によって行います。Logic が背面にあるときは未検証のまま読み、そのとき適用は、そのステップを種類に対応する Logic 自身の名前で呼び、`title` を持ちません。

ブリッジはメニューバーの項目を押しません。以前は、Logic が背面にある間に、項目を押してキャンセルすることで Edit メニューを検証していました（メニューバーが画面外にあるので見えません）。M3 のゲートで、一回の押下が、キャンセルすべきメニューのないまま項目を `selected` の状態に残し、その後 Logic は本物のクリックがあるまで、クリック、キー、選択、アクティブ化を一切受け付けず、Apple Event にも答えなかったため、それ以降の変更はすべて失敗しました。読み取りは何も固まらせることがなく、プローブはほとんどの取り消しをすぐに特定するので、危険よりも損失のほうを受け入れます。その損失とは、Logic が背面にある間にプロデューサーがマーカーやノートのステップに対して押した Cmd-Z が、Logic が次に Edit メニューを検証したとき（Kumi が前面で次に書き込むとき、またはプロデューサーがメニューを開いたとき）に初めて特定されることです。タイトルを初めて未検証のまま読んだとき、ログに一度そう書きます。残しているのは対処のほうです：書き込みが前面を得られずに `busy` で諦める前に、`selected` のまま残っているメニューバーの項目が見つかれば、そのメニューにキャンセルを送って閉じ（最大 2 回）、もう一度 Logic に前面に出るよう求めます。開いたままのメニューはログに記録され、その変更は `busy` になります。Logic 自身の取り消しによる復元は、まず Logic を前面に出し、その後で初めて検証済みのタイトルを確かめます。

ブリッジは、タイトルやプローブが動いた変更の `desc` を付けて `undone_in_daw {descs, how, epoch}` を送るので、Logic のタイトルに nonce がなくても、ホストの `by_descs` による照合が Kumi の変更を特定できます。Logic では `plan_steps` は false です：プランは Logic の取り消しの複数のステップで、それぞれタイトルで名前が付きます。

### Logic が REAPER との違いを強いるところ

- Logic は、ミキサー的な操作（ボリューム、パン、ミュート、ソロ、アーム、テンポ、バイパス）には**取り消しのステップを作りません**：これらは `undo.entry: false` を答え、Logic 自身の取り消しではなく、常に値によって取り消されます。**メトロノーム**（`metronome-set`）にも作りません：Logic の履歴が空の状態で Metronome Click をオフにし、またオンにした後も（それぞれの押下を更新用のキーで検証）、Edit メニューはグレーの "Undo" のままでした。クリックは Logic がプロジェクトと一緒に保存する設定なので、切り替えるとサイクルと同じくプロジェクトが変わります：閉じるときに Logic が保存するかを尋ねます（クリックをオフにしてまたオンにする、聴き取りのパスの後と同じです）。
- テンポはスライダーから**整数の BPM** しか受け付けません：小数の `bpm` は丸められ、注記が付きます。
- **マーカーには外から名前を付けられません**（その名前のセルはテキストフィールドではありません）：`marker-add` は `name` を拒否し、マーカーの番号は Logic が自分で付けます。`marker-rename` は作っていません。
- `fx-move`、`send-mode`、`track-move`、`track-color`、`marker-rename` は**作っていません**：Logic はプラグインをドラッグでしか動かしません。センドのモードはセンドのメニューの中のマークで、アクセシビリティはそれを読めないので、決して読み戻せません。ヘッダーはドラッグできません（トラックを並べ替えるキーコマンドはなく、ヘッダーはメニュー以外のアクションを持ちません）。Color パレットの色見本はアクセシビリティにとっては単なる位置で、ヘッダーは色を示しません。どれも理由を述べて `bad-params` で拒否されます。
- **Mixer のスロットのメニューは Logic 独自のもの**です（インサートスロット、インサートバー、センドスロット、スロットのリストボタンのメニュー）：アクセシビリティはそれらの要素を示さず、メニューが出ている間は押下が止まったままなので、ヘルパーはその押下に、1.5 s のメッセージングのタイムアウトの後 `cannotComplete` と答え（遅い Mac では `timeout`）、メニューは出たままです（ウィンドウサーバーはそれをポップアップメニューのレベルで表示します）。そのとき Logic に送ったキーはメニューに届きます：プラグインメニューの検索はキーを一つずつ打った名前を受け付け（キーコード。文字として送ったテキストも届きますが、ブリッジはキーを打ちます）、Down で最初の結果が強調され、Return でそれが選ばれ、Logic がそのストリップに選ぶフォーマットで入ります。使用中のスロットのメニューでは、No Plug-in は 2 番目です。センドのメニューは矢印キーでたどり（Down 2 回で Bus、Right でその中へ、Down でそのバスまで）、使用中のセンドのメニューでは、No Send はルーティングモードの後の 6 番目です。ルーティングモードのマークはアクセシビリティでは読めません。Escape でどれも閉じます。Logic が取り込んだプラグインやセンドは名前で読み戻し、違うもの（別の検索結果）なら外します。
- Logic は**プラグインを挿入するとそのウィンドウを開きます**（ストリップの名前をタイトルにしたダイアログ）：ブリッジはそれを閉じ、消えるのを待ちます。それが出ている間、Edit メニューはプロジェクトではなくそのウィンドウのためのもの（"Undo"、"Redo"）になるからです。
- **プラグインのパラメータにはそのウィンドウの Controls View で届きます**（ビューメニュー。ポップアップを通じて読む標準のメニュー）。Controls View のスライダーは正確な値を受け付けます。Logic 自身のプラグインはエディタービューでもスライダーを公開していますが、すべてのプラグインのスライダーを並べるのは Controls View です。`value` として求められた数値（ビューが示すもの："-3.0 dB" なら -3）は、スライダーの生の値を二分探索して見つけます。一つずつ設定してそのテキストを読み、ある値がその数値をちょうど示すまで続けます（Stereo Out の Gain は生の値 930 で -3.0 dB を示します。以前は、4 分の 1 dB 以内に入った最初の読み、-3.1 dB で探索を終えていました）。どの値もそうならないときは、その数値自身の精度の範囲で最も近い値を設定し、`display.note` がそこでのスライダーのステップを伝えます。そこで動かしたパラメータと読み込んだプリセットは、プロジェクトのではなく、プラグインウィンドウ自身の取り消し（その Undo ボタンと Redo ボタン）のステップなので、`outside_undo` がそれらを保持し、復元は値によります（プリセットの復元はウィンドウの Undo で）。設定（プリセット）のメニューも Logic 独自のものです：プリセットには次と前のボタンで届きます。**幅の狭いヘッダーは収まらないものを隠します**：Controls View のヘッダーには Undo と Redo がなく、小さなエディター（75% の Gain）には設定のボタンさえないので、`fx-preset` はエディターで作業し、ヘッダーにボタンが出るまでサイズを一段ずつ大きくし（ビューメニューのサイズ）、後でサイズを元に戻します。"Default Preset" はどの設定ボタンの道筋にもありません（そこから次へ進むと最初のプリセットになり、最初のプリセットから前へ戻っても動きません）：それを戻せるのはウィンドウの Undo だけです。
- **独自のエディターを持つプラグインは、そのコントロールをヘッダーのコントロールと並べます**：Compressor のウィンドウ（Logic Pro 12.3.1）はトップレベルに 44 の要素を並べ、そのノブ、回路タイプのボタン、フィールドがヘッダーのものに混じり、プラグインとストリップの名前を示すテキストが最後に来ます。ブリッジはウィンドウのトップレベルを 40 ずつ読み、それで切れていたときは全体をもう一度読むので、ウィンドウは見つかります。そのヘッダーは Side Chain のポップアップ（"Internal"）を設定のポップアップ（"Custom"）より前に並べるので、設定はヘッダーの最後のポップアップです。そのエディターは、ビューメニューの 50%〜200% とは別に独自のサイズ（71%）を持ち、一段大きいサイズは、メニューのサイズのうちそれを上回る次のものです。その Controls View は 25 のパラメータを並べます：Threshold（"-17.5 dB"）、Ratio（"3.9:1"）、Attack、Release、Make Up（"0.0 dB"）、Knee（"0.7"）、Peak/RMS、Auto Gain、Distortion、Circuit Type、Side Chain Detection、Limiter Threshold、Limiter On、Auto Release、サイドチェインフィルターの State、Mode、Frequency、Q、Gain、Mix、Gain Reduction、Input Gain、Output Gain、Display Select、View Select（Scratch の Keys）。Knee、Peak/RMS、フィルターの Gain、Gain Reduction はコントロールなしで表示されます（テキストのグループには範囲のない無効なスライダーがあります）：これらはテキストと `null` の値で読まれ、設定はされません。
- この Mac では、**Logic が作るセンドは −∞ から始まります**：`send-add` は `volumeDb` が与えられればそれを設定し、与えられなければそう伝えます。新しいバスの AUX は Logic 自身が作るもので、`created.aux` に名前が入ります。
- **パッチはストリップ全体を置き換えます**（インストゥルメント、エフェクト、センド）：パッチは Library の検索を通じて読み込みます。検索結果の行はアクセシビリティによる選択を受け付けます（Library のブラウザの項目はクリックしか受け付けません）。パッチを戻せるのは Logic 自身の取り消しだけです。
- **Smart Controls とセンドのレベルはステップで動きます**：値の設定は、求めた値に向けて Smart Control を 100 分の 1、センドのノブを細かいステップ一つ分動かし、インクリメントは 10 分の 1、または粗いステップ一つ分動かします。センドの細かいステップは -6 dB より上では 0.1 dB、それより下では 1 dB で、インクリメントは 0 dB 付近で 1 dB、それより下では 10 dB です（そのためレベルを通り過ぎてから戻ります）。こうしてレベルには細かいステップの半分以内で届き（−6.5 を求めると −6.1、−20 では −20.1）、Logic の表示どおりに答えます。複数のセンドのノブは、すべてのセンドのグループの後にその順で並び（センドが一つだけならそのグループの直後）、順番で対応づけます。
- **Smart Controls はペインに表示される位置の順に並べます**（フレームから、行ごとに、その中で左から）：アクセシビリティはペインのスライダーを独自の順で並べ、その順はペインを表示するたびに変わります。
- **オートメーションは Track Automation Event List を通じて読み書きします**。これは独立したウィンドウで（そのキーコマンドを押すたびに新しいウィンドウが開くので、ブリッジは自分が開いたものを閉じます）、その行は、Logic 自身のパラメータ番号で番号付けされた Logic の "Fader" イベントです（Num：Volume は 7）。パラメータの名前は Length/Info セルにあり、値は 0〜127 です（dB のテキストはありません）。イベントは再生ヘッドの位置に作られ、LCD は再生ヘッドを拍の上に置くので、イベントは整数の拍になります。名前で求められたパラメータは、新しい行の Num を 0 から上へステップしていき、セルがその名前を示したところで見つかります（最大 127）。イベントの位置や値の変更は、イベントを取り除いて作り直すことです。どれもエポックを進めません。各イベントは Logic の取り消しのステップです（"Create Event"、"Value Change"、"Delete"）。
- **バウンス**は Logic 自身の Bounce ダイアログを通ります：そのポップアップメニューはポップアップを通じて読める標準のメニューですが、別のメニューが閉じた直後に開いたメニューはまだ出ていないことがあるので、もう一度探してもう一度押します。**書き出しは書き込みの間 Logic のメインスレッドをふさぎます**：ヘルパーの読み取りはタイムアウトし、ブリッジはファイルを待つ間それを busy とみなします。書き出しのオプション（ファイル名、フォーマット）はプロデューサーのものです：この Mac では、パターンがファイルに番号で名前を付け（"01.wav"）、音のないトラックはファイルを書きません。
- **Library の検索結果**はそのペインの 6 階層下に、その名前はさらに 3 階層下にあり、ヘルパーの一回の読み取りが届く深さ（8）より深いところにあります：まずテーブルを見つけ、それから読みます。
- **サイクル**（`loop-range`）は LCD の Custom 表示モードを通じて書き込み、そのロケーターのスライダーは Logic のティックの上にあります。サイクルは Logic の取り消しのステップにはならず、オフのサイクルもロケーターを保ち、復元はそれも元に戻します。
- Logic の**クオンタイズはイベントのパラメータ**で、移動ではありません：`notes-quantize` はリージョンのすべてのイベントについて Event List の Quantize ポップアップを設定し、すると行はノートをグリッド上の位置で示し（Logic もそう鳴らし）、録音された位置はその下に残ります。復元はポップアップを元に戻すので、ノートはまた録音されたとおり正確に読めます。グリッドはポップアップの音価か三連符のどれかで、強さはありません（100 のみ）。
- Event List は、編集したイベントが選択中のイベントの中にあると、**値の変更を選択中のすべてのイベントに適用します**：ほかと一緒に選択されているノートの行は、書き込みの前にそれだけを選択し直します（`notes-change`、`notes-delete`。クオンタイズの選択は後で元に戻します）。
- リージョンの**端は整数の拍で動きます**（`region-resize`：LCD の小節と拍で再生ヘッドを置き、その後 Logic の Set Region Start/End to Playhead）。分割と削除にはリージョンのクリックが必要です（キーコマンドはキーボードフォーカスに対して働き、`AXSelected` の設定ではキーボードフォーカスは得られません）。
- **サンプルは変換によって Sampler トラックになります**：Logic はオーディオリージョンからサンプルを Sampler に読み込む（Track > Convert Regions to New Sampler Track…）ので、`sample-load` はまずファイルをオーディオトラックにインポートし、Logic が選択したリージョンを変換します。Logic はそのオーディオトラックの下に、インストゥルメントの名前を付けたソフトウェア音源トラックを、一つのゾーンとそのトリガーノートの MIDI リージョンとともに作り、オーディオリージョンはミュートして元の場所に残します。Logic は、保存されているかどうかにかかわらず**ファイルをすぐにプロジェクトのパッケージにコピーし**（インポートは `Media/Audio Files/<file>`、ゾーンは `Media/Samples/<instrument>/<file>`）、その取り消しはコピーを残します：この種類は Scratch ではなく Scratch のコピーで計測しました。Scratch のパッケージだとコピーが残ってしまうからです。復元は、変換が一番上にある間はその変換の、続いてそのすぐ下のインポート（"Insert"）の Logic 自身の取り消しで、これで新しいトラックとオーディオリージョンがなくなります。どちらも読み戻し（Sampler トラックのヘッダーが消えたこと、そしてインポートが作ったオーディオリージョンを、そのトラックと範囲で）、反映されなかったステップは名前を挙げて `uncertain` になります（"its “Undo Insert” didn't land: the audio region of “kumi-tone” is still on “Vocal”"）。
- **MIDI のインポートはトラックを作ります**：File > Import > MIDI File… は、指定したトラック（"Studio Grand"）の下の新しいトラックにファイルを置くので、`created.trackAdded` は `true` で、インポートの取り消しはリージョンと一緒にトラックも取り除きます。
- 外から作った**テンポイベント**は Logic の取り消しの二つのステップになります（"Insert Tempo"、"Value Change"）。Tempo List と Signature List はステップごとにセルを作り直すので、ブリッジは各ステップをセルのパスで読み戻します。拍子の行には Delete アクションがありません：Kumi が作った行は、選択してからリストの Edit > Delete で削除します。どちらのリストも、位置には整数の拍を取ります。
- **Track Stack** は、選択したヘッダーに対して Logic 自身のキーコマンド（Create Summing/Folder Stack）で作り、ダイアログは決して使いません。名前は Logic が付け（Sum 1、Sub 1）、`name` があれば後で名前を変えるので、作成の上に "Renaming" が積まれます。新しいフォルダースタックは閉じた状態で表示されるので、読むために開きます（Open Track Stack）。サミングスタックをフラット化しても、その作成を取り消しても、サブトラックの出力はスタックのバスのまま残ります（Logic 自身の動作です）。どのトラックがスタックの下にあるかは読めないので、フラット化したスタックは Logic の取り消しでしか戻りません。
- **キーコマンド**は Logic が前面にあるときしか動きません。`key_command` は名前か id を受け取り、プロデューサーのキーセット（`com.apple.logic10.plist`、プロデューサーが選んだプリセット）から、その後ろに工場出荷時の U.S. セットを控えさせて解決し、割り当てのないコマンドは拒否します。
- `track-add` はトラックを**選択中のトラックの下に**置くので（Logic の New Tracks の動作）、`index` は 1 から数えます（すべてのトラックの上の 0 には置けません）。種類（`audio`、`instrument`、`external`）を指定できます。

## 聴き取り（`hear`、`audition`、`render`）

M5 で Kumi は Logic を聴けるようになります。三つはブリッジのメソッドで、変更と同じくランループのキューに入ります。どれも変更でも行でも Logic の取り消しのステップでもなく、それぞれ Logic を見つけたときのまま残します：停止した状態で、再生ヘッドは元の小節と拍に（拍の上に）、すべてのトラックのソロ、メトロノーム、サイクルは元のままです。何があっても（失敗したパスも含めて）これらは元に戻され、戻せなかったものは `notPutBack` に並びます。Logic が再生中か録音中なら `busy`（"Kumi listens only while it is stopped"）、ダイアログが出ていれば `busy`、アクセシビリティの許可がなければ、ほかのすべてのメソッドと同じく言葉で理由を述べる `failed` です。オーディオがワイヤーを通ることはありません：各答えは、リクエストの `folder`（存在するフォルダーへのフルパス）にあるファイルを示します（Kumi はフォルダーを作らず、ファイルを決して上書きしません：`kumi-hear-<n>.wav`、`kumi-audition-<n>-<k>.wav`、`kumi-render-<n>.wav` で、そこにあるものより後の番号を付けます）。Kumi はメトロノームを決して聴きません：control bar の Metronome Click は、オンならパスの間オフにして読み戻し、後でまたオンにします（`metronome: "off for the pass"`。オフだったときは `"off"`）。メトロノームの状態は Logic がプロジェクトと一緒に保存する設定なので、オフにしてまたオンにすると（パスの一連の実行は毎回そうします。クリックの手順でオンにするからです）、中身が何も変わっていなくてもプロジェクトは編集済みになります：プロデューサーが次にプロジェクトを閉じるときに、保存するかを尋ねられます（再生ヘッドを動かしても編集済みにはなりません。Scratch のコピーで計測）。

| メソッド | パラメータ | 答え |
| --- | --- | --- |
| `hear` | `fromBeats`、`beats`（0.25 以上、プロジェクトのテンポで 600 s 以下）、`folder`、`mute?`（false）、`solo?`（パスの間それだけをソロにするトラックの参照。なし：ミックス全体で、どのトラックもソロにしない）、`epoch?` | `{file, seconds, sampleRate, channels, lead: 0, peak, rms, aligned: {by, latencyMs, errorMs}, fromBeats, beats, mute, solo, metronome, epoch, note?, notPutBack?}` |
| `audition` | `candidates`（`{ref}`（そのトラックだけをソロ）または `{mix: true}`（ミックス全体）を 1〜16 個）、`fromBeats`、`beats`、`folder`、`mute?`、`epoch?` | `{candidates: [{ref` または `mix: true, name, file, seconds, sampleRate, channels, lead: 0, peak, rms, note?}], aligned, fromBeats, beats, mute, metronome, epoch, notPutBack?}`：候補はその順に、一回のキャプチャの中で一つにつき一回のパスで聴く |
| `render` | `ref?`（バウンスの間それだけをソロにするトラック。なし：ミックス全体）、`fromBeats`、`beats`、`folder`、`epoch?` | `{file, seconds, sampleRate, channels, bits: 32, lead: 0, fromBeats, beats, metronome, epoch, ref?, note?, notPutBack?}` |

`peak` と `rms` はファイルのすべてのサンプルにわたるリニアの値です（1.0 がフルスケール）。`seconds`、`sampleRate`、`channels` はファイルのものです。パスはデフォルトの出力デバイスのレートでステレオの 32-bit float としてキャプチャされ（ラボの Mac では 44.1 kHz）、レンダーはプロジェクトのレートでの Logic のバウンスで、32-bit float です。`lead` は 0 です：ファイルは部分の最初の拍から始まります。プロデューサーがミュートしたトラックをソロにしても無音のままで、そのテイクの `note` がそう伝えます。部分の終わりまで行かずに止まった再生（プロデューサーが Stop を押した）は再生した分を残し、`note` が Logic の止まった位置を伝えます。参照はこのエポックのものでなければならず（そうでなければ `stale`）、リクエストが `epoch` を与えるときは、それはプロジェクトのエポックでなければなりません。

### パスの一連の実行

1. Logic をそのままの状態で読みます：トランスポート（再生中は `busy`）、すべてのヘッダーのソロとミュート、Metronome Click、サイクル、再生ヘッド、リージョン（曲がどこで終わるか）。
2. サイクルをオフ、メトロノームをオフにし、ヘルパーのタップを Logic の pid に対して開始して、`folder` の下の `kumi-tap-<n>.wav` に記録します（`mute` のときはプロデューサーには聞こえないようにミュート）。長さは最大で、パス全体の長さと余裕分です。
3. 各パスについて：そのパスが求めるトラックだけをちょうどソロにします。Mackie Control のストリップがトラックと並んでいるとき（Logic が最後に書いた名前で、LCD が最初の八つを順に示しているとき）はそのソロボタンで、そうでなければ各ヘッダーの Solo チェックボックスで（Logic を前面にしてクリック）行い、ヘッダーから読み戻します。再生ヘッドを部分の最初の整数の拍にちょうど置きます（拍の途中から始まる部分は前の拍から再生し、始まる位置で切ります）。Play。再生ヘッドを、部分の終わりから 150 ms 過ぎるまで、表示の更新を八回以上見ながら追います。Stop。
4. クリック：再生ヘッドを曲の終わりの一小節後（何も鳴らないところ）に置き、メトロノームをオンにし、一拍と少しの間 Play（二回のクリック）、Stop。
5. タップを止め、Logic を元に戻し（停止、ソロ、メトロノーム、元の小節と拍の再生ヘッド、その後でサイクル。サイクルがあると、Stop ボタンはその左のロケーターへ移動します）、各パスの部分をキャプチャから切り出してそれぞれのファイルにし、キャプチャを削除します。

### 再生ヘッドの追跡と、部分の切り出し方

再生ヘッドは Mackie Control のタイムコード表示で追います。Logic は再生中、変わった桁を約 33 ms ごとに、桁ごとに一つの CoreMIDI パケットで、上位の桁から送ります。ヘルパーは各パケットに、それが届いたホスト時刻を刻みます（Logic 自身のパケットのタイムスタンプは 0 です）。上位から下位へ続く桁の連なりが一回の更新で、その最初のパケットの時刻を持ちます。サーフェスの PLAY と BEATS の LED が点いている間、各更新は一つの位置（小節、拍、ディビジョン、ティック）です。一回の再生の位置は、ホスト時刻をティックに対してとった直線の上に 0.4 ms 以内で乗り（四分音符が 960 ティック。ディビジョンのティックは、それらが直線になるような除数として求めます）、ときどき数ミリ秒遅れる更新がありますが、フィットはそれを除きます。Play から 1.5 s 以内にサーフェスが何も送らないとき（サーフェスがない、またはその表示が SMPTE 時間）は、代わりに control bar の小節と拍をポーリングし（`by: "transport"`）、拍の変化はそれぞれ前後二回のポーリングの中間の時刻とします。その精度はポーリング間隔の半分です。

タップは同じクロックでフレームに時刻を刻む（アンカー）ので、拍のホスト時刻はキャプチャのフレームになります。タイムコードからわからないのは、Logic の表示が示すものをタップがどれだけ遅れて聴くかです：ラボの Mac では、そのレイテンシーはタップごとに変わり、40 回のタップで 9〜21.5 ms でした（一回のタップの中では、再生ごとの違いは約 1 ミリ秒）。どんな固定値でも 10 ms 以内には収まりません。そのため各実行は手順 4 のクリックでそれを測ります。クリックは、ほかに何も鳴らないところで、拍からそのレイテンシーだけ後に記録されるので、キャプチャのすべての部分をそれで切ります（`by: "timecode+click"`、`latencyMs`）。クリックが聞こえないとき（メトロノームを無音に設定している）は 15 ms とし、`errorMs` は 10 です（`by: "timecode"`）。`errorMs` は切り出しの不確かさです：二回のクリックの一致、直線のばらつき、そして一回のタップの中で再生ごとに変わる幅（1.5 ms）。

160 BPM の Scratch で、各ファイルを削除する前にレコーダーが読んで計測しました：`listen-align` での小節 1 の六回の hear と `listen-hear` の二回では、Piano リージョンの四つのノートは、ファイルの中で拍が示す位置より 0.4〜1.8 ms 後に始まりました（32 ノート。クリックが測ったレイテンシーは 20.1〜21.2 ms、`errorMs` は 1.7〜1.8）。その部分を Logic 自身がオフラインでバウンスしたもの（`listen-render`、Keys のみ）では、同じノートは拍の 0.2〜0.9 ms 後に始まる（ピアノのアタック）ので、聴いた部分は、バウンスが拍を置く位置から約 1 ミリ秒以内で切り出されています。audition のテイクでは、ノートは拍の 0.5 ms 前から 0.7 ms 後の間に始まりました。クリックを使う前、ラボではタイムコードと固定のレイテンシーで切り出していました：タップのレイテンシーはタップごとに 9〜21.5 ms の間で動いたので、固定値では最大 11 ms ずれました。同じタップの別の再生の中でクリックによって測ると、ノートは 2 ms 以内に収まりました。

### レンダー

`render` は、その部分に対する M4b の `bounce`（上記）です：32-bit float の WAV、Normalize はオフ、Add to Project はオフ、メトロノームはオフ（そうしないとバウンスにクリックが入ります）、そしてリクエストが求めるソロ。トラック自身のオーディオとは、そのトラックだけをソロにしたもので、その音は Stereo Out のプラグインを通ります（その `note` がそう伝えます）。ソロとメトロノームは元に戻します。

### 権限

ラボの Mac（macOS 26.6.1）では、タップに確認のプロンプトは出ませんでした：ヘルパーはブリッジが動いているターミナルの下で動き、そのターミナルのアクセシビリティの許可を使います。システムオーディオ録音の許可は一度も求められませんでした。macOS がタップを拒否する環境では、ヘルパーの `tap.start` は `denied` を、`hear` と `audition` は `failed` を答え、Kumi が動いているアプリにシステムオーディオ録音を許可するよう伝えます（システム設定 › プライバシーとセキュリティ › Screen & System Audio Recording）。macOS 14.2 より前では、`failed` は聴き取りには 14.2 が必要だと伝えます。

### かかる時間と、ホストの呼び出しが許すべき時間

Scratch（160 BPM、部分の長さは 1.5 s）で計測しました：`hear` は 4.3〜4.9 s かかったので、一回の実行のオーバーヘッドは 2.8〜3.4 s です（そのうち約 1 秒がクリック）。三つの候補の `audition` は 9.7 s かかり、最初のパスの後は、パスごとにその部分の長さと約 1.2 s が加わりました。Keys だけの `render` は 8.2 s、ミックスの 4 小節（オーディオ 6 s）の `render` は 6.4 s かかり、そのほとんどは Bounce ダイアログと保存パネルでした。

ブリッジは聴き取りのリクエストを、その部分が再生される時間と余裕分だけ、フォーカスが最後に読んだプロジェクトのテンポで（なければ 60 BPM）待ちます：`hear` は部分の秒数と 28 s、`audition` は各候補の部分と 8 s ずつ、さらに 20 s、`render` は部分と 45 s（`listen::bound_seconds`）で、期限が何と言っていても最大 2 時間です。自分の期限を与えたリクエストは、その期限まで待ちます。ホストの呼び出しも、部分の長さと余裕分を許す必要があります：1.5 s の部分は約 5 s で答えが返るので、ホストの 30 s の変更タイムアウトで足りるのは約 20 s までの部分で、それより長い部分（または複数の候補の audition）にはこの上限の長さが必要です。

### リージョンのオーディオファイル

Logic はオーディオリージョンのファイルを名前でしか示さず、パスは決して示しません。Scratch のコピーにインポートしたところ（Vocal にトーンを）、リージョンの項目はリージョンの名前（"import-tone"）を示し、そのヘルプテキストはどこで始まりどこで終わるか、そしてオーディオリージョンであることを伝えます。Region inspector はリージョンの名前を示します。Browsers ペインの Project タブはファイルを名前で（"import-tone.wav"）、その下にリージョンを並べますが、それらのテキストフィールドにはパスも URL もドキュメントもありません。そのためリージョンの行には `file` フィールドがなく、ランタイムの `audio_file` は Logic では読むものがありません：Kumi は代わりに、そのリージョンの小節にわたってそのトラックを `hear` または `render` して、リージョンを聴きます。
