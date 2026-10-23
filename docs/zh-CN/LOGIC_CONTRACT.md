# Logic Pro 桥接契约（M2 至 M6）

[English](../en/LOGIC_CONTRACT.md) · 简体中文 · [日本語](../ja/LOGIC_CONTRACT.md)

`logic-kumi` 通过 `kumi-daw/1` 回答什么，以及每个回答来自哪个 macOS 通道。传输协议、握手、发现文件和生命周期属于 daw-bridge；本页只讲 Logic 的数据形状。本页的一切都是在 2026-10-18 的实验（`docs/evidence/logic-lab-2026-10-18.md`）中测得的，各类行（row）还在 M2 期间于 Logic Pro 12.3.1 的 Scratch 工程上测过。确切的 JSON 由主机中模拟的 Logic（`crates/daw-mcp-server/src/simulator/logic.rs`）和桥接从真实 Logic 录下的向量（vectors，`crates/daw-mcp-server/tests/vectors/logic-reads.json`）给出；两者不同之处，向量反映的是 Logic 实际显示的内容，本页会说明原因。录制的记录中不会出现录制时那台 Mac 的主目录：桥接的录制器把 `/Users/<name>` 存为 `/Users/me`，而工程的 id（其路径的哈希）是脱敏后路径的哈希（`logic_kumi::fixtures::redacted`）。

## 通道

| 通道 | 用途 | 规则 |
| --- | --- | --- |
| 辅助功能读取 | 每种行、焦点（focus）和详情（body） | 轮询；从不读取弹出按钮的子元素；优先使用 `AXMainWindow` 和 Window 菜单，而不是 `AXWindows`；当 Logic 自己的某个窗口（Control Surface Setup）是主窗口时，按标题找到工程的窗口；读取需要的面板（Mixer、List Editors）为这次读取而打开，读完后关闭 |
| 辅助功能写入 | 带文本框或滑块的值、按钮、行 | 先设置，再 `AXConfirm`；回答之前先回读 |
| 快捷键命令 | 绑定了按键的动作 | 先激活 Logic；修饰键作为单独的事件发送 |
| 菜单项 | 没有绑定按键的动作 | 先激活 Logic；匹配标题之后按索引按下 |
| Mackie Control | 选择、走带和电平事件；LCD 上的名称；选择轨道；Logic 播放时跟随时间码显示（聆听）；各通道条的独奏按钮（一遍聆听中的独奏） | 只以 Mackie Control（设备 0x14）的身份应答，另外应答版本请求；在 Pan 视图中进行选择，此时 LCD 显示各通道条的名称；时间码的数字是 ASCII（0x31 是 "1"），前两个控制器是分配显示（"PN"） |
| 面板服务 | 打开面板和保存面板 | 向 `com.apple.appkit.xpc.openAndSavePanelService` 发送按键；用 AX 设置前往文件夹（go-to-folder）字段 |
| Spotlight | 最近使用的工程在哪里（`read {kind: "recent"}`） | 按工程的文件名运行 `mdfind`，所有名称合为一次查询；它只能找到 macOS 允许桥接看到的东西 |
| 偏好设置 | 中央 C 所在的八度（工程行的 `middleC`） | `defaults read com.apple.logic10 <key>`，经由偏好设置守护进程，所以刚在 Logic 的 Settings 中改过的设置能立即读到；从不写入 |
| 进程音频分接（process tap） | 聆听 Logic（`hear`、`audition`） | 辅助程序在 Logic 的 pid 上执行 `tap.*`，写入请求所给文件夹下的一个 32 位浮点 WAV 文件；在一轮播放的前后启动和停止，从不让它一直运行；在此期间 `mute` 让 Logic 的声音不从扬声器放出 |

## 身份

- `pid`：Logic 的 pid；`resource`：Logic 的包标识符（`com.apple.logic10`，Creator Studio 版本则为 `com.apple.mobilelogic`）。
- `project.id`：没有每个工程都具备的 UUID，所以 id 是已保存工程包路径的 SHA-256 的 32 位十六进制前缀；未保存的工程带有一个取自窗口标题的 `identity`。
- `status` 是扁平的，与 REAPER 的一样：`{project, busy, permission}`。空闲时 `busy` 为 `false`，否则为 `modal`（主窗口或焦点窗口是对话框，或者有表单（sheet）打开）、`away`（Logic 报告没有窗口，因为它所在的空间（Space）不是当前空间）或 `loading`（工程窗口还没有标题，也还没有文档）；在这些状态下会保留上一个工程。
- `status.permission`：`null`，或者在辅助程序未获信任时为 `"accessibility"`；此时读取回答 `failed`，从不回答 `busy`。身份信息（daw、bridge、dawVersion、pid、resource、project）在欢迎消息（welcome）中，与每个自有桥接一样；发现文件是 `logic-<pid>-<key>.json`，字段相同。

## 行（`read`）

| 类型 | 字段 | 来源 |
| --- | --- | --- |
| `project` | id、identity、path、tempo、timeSignature、keySignature、sampleRate、trackCount、middleC（3 或 4） | 窗口的文档和 control bar（控制条）的 LCD；能读取工程包时，采样率取自 `MetaData.plist`（macOS 不让没有“文件与文件夹”（Files and Folders）权限的进程访问 `~/Music/Logic`：此时为 `null`）；`middleC` 取自 Logic 的偏好设置，即 Logic 的编辑器给音符 60 标注的八度（Settings > View > General 中的 "Display Middle C As"：键 `MiddleC` 为开时是 C4（Roland）；为关或从未写入时是 C3（Yamaha，Logic 的默认值）），至多每五秒重新读取一次 |
| `track` | ref、index、name、kind（audio、instrument、external、aux、output）、mute、solo、armed、monitoring、volumeDb、pan、color（总是 `null`：轨道头不显示颜色）、output、inputs、selected、stack（只出现在 Track Stack 的轨道头上：`true`）、stripUnread（只在 Mixer 没有绘制该轨道的通道条时出现：`true`；见“追赶读取的问题”） | 轨道头（名称、静音、独奏、录音预备、监听、音量、声像、是否选中；堆栈的展开三角形）；类型、输出和输入取自 Mixer 的通道条，需要时读取，保留五秒 |
| `region` | ref、trackRef、name、startBeats、endBeats、kind（audio、midi）、selected、looped | Tracks area（轨道区域）的布局项及其帮助文本（"Region starts at 1 bar  and ends at 5 bars , MIDI region…"） |
| `note` | ref、regionRef、position（bar、beat、division、tick）、pitch、velocity、lengthTicks、channel、selected? | 该片段（region）的 Event List（事件列表），它显示所选片段的事件：未选中的片段会先通过它的项被选中 |
| `plugin` | ref、trackRef、slot、name、bypassed、kind（instrument、effect） | 通道条的插槽，乐器的插槽在最前；通道条自下而上列出插槽，各行则从上往下计数；输出通道条的插件（Stereo Out 的）归在 `trackRef` `<epoch>:output:<n>` 之下 |
| `send` | ref、trackRef、slot、destination、level | 通道条的发送插槽：发送所在的组给出它的总线名称，紧跟其后列出的旋钮给出它的电平（"-6.0"；"-∞" 记为 −96） |
| `param` | ref、pluginRef、index、name、value、text、min、max、normalized | 插件窗口的 Controls View（为读取而通过插槽的打开按钮打开，读完关闭，并恢复原来的视图）：每个参数一行，名称去掉冒号，滑块的值和范围（`normalized` 是值在范围中所占的比例），以及旁边的文字；选项型参数的值是所选选项的位置，菜单的值为 `null`，视图中显示但没有可设置它的控件的值也是如此（Compressor 的 Knee：只有文字，对它的修改以 `bad-params` 拒绝）；需要 `parent`（插件） |
| `smart` | ref、trackRef、index、name、value（0 至 1）、text | 该轨道的 Smart Controls 面板（为读取而打开，读完关闭），读取时选中该轨道，之后恢复原来的选择；需要 `parent`（轨道）；如果轨道的通道条上没有面板能映射的东西（一条只有 Channel EQ 的音频轨道），面板显示标题栏，下面没有控件，读不出任何行；而面板还没有绘制出标题栏时为 `failed` |
| `automation` | ref、trackRef、index、parameter、number、events（positionBeats、value） | 该轨道的 Track Automation Event List（它的快捷键命令，即命令 1032，在出厂键位集中为 Control-Command-E，会为所选轨道把它作为一个独立窗口打开；读完关闭）：其中的 "Fader" 行按参数分组，参数名称取自 Length/Info 单元格，编号取自 Num，每个事件的值（0 至 127）取自 Val；需要 `parent`（轨道） |
| `marker` | ref、position、name、length（以拍计）、text | Marker List |
| `recent` | index（从 1 起，最近的在前）、name、path（不知道时为 `null`）、open | File > Open Recent 中的各项（不打开菜单即可读取：每项都是 `_openRecentDocument:`，标题为工程的文件名）；`open` 只出现在与打开的工程文件名相同的第一项上，其 `path` 取自工程的窗口；其他项的路径则在 Spotlight 除打开的工程外恰好找到一个同名工程时取自 Spotlight（见“最近的工程”） |
| `tempo` | bpm、tempos（position、bpm）、signatures（position、time、key） | Tempo List（`tempos` 是其中的每个事件，第一个是工程开头的速度，然后按位置列出每次速度变化）和 Signature List |

桥接在 Logic 窗口之外读取的东西（偏好设置、Spotlight、磁盘上的文件）都经过 `src/disk.rs` 这一个接缝（seam），所以录制会把它们和辅助程序的回复保存在一起，回放时给出同样的回答：M6 对 Scratch 读取的录制把 `MiddleC` 记为 "0"（自从实验中切换过这项设置后就写为 `false`，那次切换找到了这个键：选择 C4 后它出现并为 `true`，再选回 C3 时它保留下来，值为 `false`）。

Logic 自己的用词保持不变：调号按 LCD 显示的样子（"C Major"），输出按插槽给出的名称（"Stereo Output"），插件按其插槽给出的名称。引用（ref）是带纪元（epoch）的位置（`<epoch>:<kind>:<n>`），与 Renoise 的一样：轨道按它在 Tracks area 中的位置，片段按它在整个工程中的位置（逐条轨道计），标记按它在列表中的位置，音符归在其片段之下（`1:note:1.3`），插件和发送归在其轨道之下（`1:plugin:1.1`），参数归在其插件之下（`1:param:1.1.7`），Smart Control 和参数的自动化归在其轨道之下（`1:smart:2.1`、`1:automation:1.1`）。Mixer 的输出通道条不属于任何轨道，按 Mixer 中的顺序记为 `<epoch>:output:<n>`（Stereo Out 是 `1:output:1`），它们的插件记为 `<epoch>:plugin:0.<n>.<slot>`，这些插件的参数记为 `<epoch>:param:0.<n>.<slot>.<i>`。桥接在两次轮询之间观察到的每次轨道或片段变化都会推进纪元（某个轨道头出现、消失、被重命名或移动；某个片段被添加、移除或移动）；音符、插件、发送、标记和速度不会推进纪元。来自较早纪元的 ref、parent、around 或 cursor 都会以 `stale` 拒绝，无论它原本用于什么。分页：`limit`（50，最多 200）、`cursor` `<epoch>.<n>`，每个回答中都有 `nextCursor`、`total` 和 `epoch`；只给 `ref` 时读取一行；`parent` 用于过滤；`note` 需要以它的片段作为 `parent`。

### 批量读取（`snapshot {rows}`）

`snapshot {rows: [kinds], limit?}` 在一次请求中读取所列各类型（`project`、`track`、`region`、`plugin`、`send`、`marker`、`tempo` 之中）的每一行，在同一个纪元里对工程只看一次：`{epoch, rows: {<kind>: [rows]}, totals: {<kind>: n}, truncated?: [kinds]}`，每种类型的行与 `read` 为它分页给出的完全一致（相同的行，相同的顺序），在 `limit`（10000，即最大值）处截断，被截断的类型列在 `truncated` 下，再用 `read` 分页读取。没有 `rows` 时，`snapshot` 的回答与以前一样。请求轨道、插件或发送时，它显示一次 Mixer（它们的类型、路由和插槽都来自 Mixer），为标记、速度和拍号显示一次 List Editors（依次切换每个标签页，然后恢复原来的标签页和面板），而分页读取每个列表都要显示一次 List Editors：在 Scratch 上，一次追赶读取（catch-up）所做的六次读取（project、track、region、plugin、marker、tempo；每页 200 行）用了 4.4 至 4.5 s，其中 Mixer 0.7 s，Marker List 1.2 s，Tempo List 和 Signature List 2.4 s，而对同样的行做批量读取用了 2.7 s。两种方式都会打开 Logic 尚未显示的东西：没有 Mixer 就读不到通道条，没有 List Editors 就读不到列表；制作人已经打开的面板保持打开。回答是一个帧（传输协议允许 64 MB）。

### 最近的工程

Logic 把最近使用的工程保存在 macOS 的共享文件列表中，而 macOS 不让其他进程访问这个列表（"Operation not permitted"），Logic 的偏好设置里也没有工程路径（实验用的 Mac 上有 518 个键；`DefaultDir1` 至 `4` 是打开面板和保存面板最后所在的文件夹）。File > Open Recent 会显示这些工程，辅助功能不打开菜单就能读取其中的项，但每一项只带有工程的文件名（`AXTitle`；没有 URL、文档或帮助：在 Logic Pro 12.3.1 上，辅助功能能看到的只有它的标题、标识符和 frame（元素在屏幕上的矩形））。所以打开的工程无法按完整路径匹配：它是同一文件名的第一项（菜单从最新的开始，打开一个工程会把它放到那里），只有这一项是 `open`；另一个同名项（另一个文件夹中同名的工程）不是。`read {kind: "recent"}` 按顺序回答各项，在桥接能知道路径的地方带上 `path`：当前打开的工程（取自其窗口的文档），以及 Spotlight 按该文件名恰好找到一个的工程（不在任何其他包中，不在废纸篓中，也不是打开的那个）；除打开的工程外，两项同名时无法区分，所以两项都没有路径。Spotlight 只对桥接进程可以读取的文件夹给出回答，所以 `~/Music`（Logic 自己的文件夹）下的工程永远找不到，除非它正是打开的那个：在实验用的 Mac 上，读取八个最近的工程时，Scratch 的路径取自它的窗口，七个验收副本的路径取自 Spotlight（它们放在主目录之外的一个共享文件夹中），用时 101 ms（第二次读取 66 ms）。

### 追赶读取的问题（M6）

在 Scratch 的一个副本上测得，这个副本在 Vocal 下方添加了 38 条音频轨道（共 40 条轨道，Keys 及其片段在最后）：

- **视野之外的轨道和片段也能读到**：轨道头的组列出全部 40 个轨道头及其 frame，无论是否在视野内（Tracks area 显示 y 194 至 1117 时，轨道头 1 在 y -1420），内容区列出每个片段（Keys 的片段在第 40 条轨道上，位于视野下方）。**Mixer 只绘制它显示过的内容**：它列出每个通道条，但视野右侧的通道条（第 40 个在 x 1728，即面板的右边缘，然后是 Stereo Out 和 Master）的描述只是数字（"224 40 4"），控件和插槽也都是通用的（"output"、"input"、"audio plug-in"，每个都处于旁通状态），而 Keys 上实际有 Piano、Channel EQ、Compressor 和 ChromaVerb；已向左滚出视野的通道条（Vocal 在 x -885）读取正常。对这样的通道条执行 `AXScrollToVisible` 会得到应答，但什么都不会移动，而且 Mixer 不向辅助功能显示滚动条。所以对于它认出是未绘制的通道条（描述是纯数字，而不是它的名称），桥接不从中读取路由、插件或发送，轨道行会注明 `stripUnread: true`：快照能把未读的通道条和空的通道条区分开。轨道的类型仍然根据其插槽的形态判断（Keys 读作 `instrument`）。
- **读取 40 条轨道**：六次分页读取用了 6.5 s（工程 0.3 s、轨道 1.4 s、片段 0.3 s、插件 0.3 s、标记 1.5 s、速度 2.7 s），批量读取用了 3.3 s。
- **片段的音符不会显示出来**：片段在 Tracks area 中的项，除了帮助文本和大小（"4 bars"）之外，只有名称字段和三个手柄（移动左边界、移动右边界、循环）；没有音符的数量或指纹，这些只有 Event List 能给出（每个片段约 0.7 s，并需选中该片段）。
- **`looped` 只体现在片段的宽度上**：设为循环后，Keys 的帮助文本（"starts at 1 bar and ends at 5 bars"）和大小（"4 bars"）都没变，而它的项的宽度随循环一直延伸到工程末尾（13,421,293 px，未循环时为 192 px）。片段读取只请求代价低的属性，其中没有 frame，所以从 Tracks area 读到的 `looped` 一直是 `false`；所选片段是否循环，由 Region inspector 的 Loop 复选框（`region-loop`）给出。
- **Project Alternatives** 没有切换过：File > Project Alternatives 只有在菜单打开后才会填充它的子菜单（在此之前辅助功能只列出一个无标题的项），也没有快捷键命令能触及它，而桥接从不按下菜单栏中的项（M3 验收时出现过卡死）。窗口的文档是工程包（`…/Scratch M6.logicx/`），各个备选版本（alternatives）是包里的文件夹（`Alternatives/000`，…），所以只要文档仍然是这个包，id（文档路径的哈希）就是这个包的；没有观察过切换时的情况。

## 焦点与详情

- `focus`：`{epoch, view, editor, selection, playhead, transport, changed, ageMs}`，由一个缓存回答，运行循环每 150 ms 用四次小读取（control bar、轨道头、片段、List Editors 的标签页）刷新这个缓存，从不遍历整个窗口。`view` 为 `tracks`；List Editors 显示 Event 标签页时 `editor` 为 `eventList`，Mixer 面板打开时为 `mixer`，编辑器面板显示其中之一时为 `pianoRoll` 或 `stepSequencer`，否则为 `null`。`selection` 带有 `trackRef` 和 `regionRef`（没有时为 `null`），旁边还有所选轨道的 `track: {name, kind}` 和所选片段的 `regionName`，供运行时读取。`transport` 是 `{playing, recording, cycle, tempo, beat, beatsPerBar}`；Logic 无法接受工作时会加上 `busy`，取值与 status 的代码相同。
- `focus_body`：`tree`（通道条：name、kind、volumeDb、pan、mute、solo、armed、inputs、output、带 `pluginRef` 的 inserts、带 `sendRef` 的 sends），`notes`（片段的音符，分页，带 `startBeats`、`endBeats`、`name` 和 `timeSignature`），`timeline`（把轨道作为泳道（lane），在一个以拍为单位的窗口中列出其片段，按泳道分页，带 `beatsPerBar`、`playheadBeats`、`playing` 和到目前为止读到的标记），`routing`（inputs、sends、output）。详情用 `ref` 指定轨道或片段，否则取所选的那个。

## 事件

`project`（路径或 identity 变化），`transport`（control bar 每次变化时的状态，以及立即从 Mackie 的 LED 得到的状态：`{playing, recording, cycle, playhead, tempo, timeSignature}`），`tracks`（纪元推进时发送 `{epoch}`：某个轨道头出现、消失、被重命名或移动，某个片段被添加、移除或移动），`selection`（焦点的选择变化时，发送它及其 `epoch`）。

## M2 维持现状的地方

- 从外部选择轨道只能通过 Mackie Control 的选择（select）按钮：轨道头的 `AXSelected` 不可设置，它们的 "Has Focus" 按钮不接受按下，Tracks area 也不接受来自辅助功能的键盘焦点。控制界面必须处于 Pan 视图，select 才表示选择该轨道；Logic 第一次添加该控制界面时会打开它的 Control Surface Setup 窗口，在该窗口是主窗口期间，桥接按标题读取工程的窗口。
- Logic 从按钮打开的弹出菜单（发送插槽、插入插槽、输出插槽）在显示期间会挂住辅助功能调用，并且不显示任何菜单元素。M4 的第二批工作发现，这个菜单照样接受发送给 Logic 的按键（见“Logic 迫使做法与 REAPER 不同之处”）。
- 列表的文字单元格（标记的名称）不是文本框：标记无法从外部重命名；Event List 处于片段层级时，其中片段的名称是文本框。
- 拖动条（scrubber）和滑块每次设置（set）移动 1，每次递增（increment）移动 10（速度、播放头所在的小节、列表的位置分段），M3 的修改必须按这一点来计算。

## 辅助程序协议（`kumi-logic-hands`）

桥接把辅助程序作为子进程运行，通过它的 stdin 和 stdout 以 JSON 行通信：每行一个请求，每行一个回复，事件穿插其间。请求是 `{"id": n, "op": "…", …}`；回复是 `{"id": n, "ok": true, "ms": n, …}` 或 `{"id": n, "ok": false, "error": "<code>", …}`；事件是 `{"event": "…", …}`，不带 id。一次只处理一个请求：辅助程序在 2 s 内回答，或者回答 `timeout`（工作已经开始）或 `busy`（上一个请求仍在运行）；它只向 stderr 写日志，除非被要求，否则从不激活任何应用。辅助程序的 README（`crates/logic-kumi/helper/README.md`）是确切的参考；本表只是摘要。

| 操作 | 参数 | 回复 |
| --- | --- | --- |
| `trusted` | `prompt`（bool，默认 false） | `trusted` |
| `apps` | `bundles`（各 id） | `apps: [{bundle, pid, name, active, hidden}]`，之后为这些 bundle 发送 `app` 事件 |
| `processes` | `contains`（可执行文件路径的子串） | `pids`（升序） |
| `activate` | `pid` | `active` |
| `windows` | `pid`、`menu`（Window 菜单的标题，默认 "Window"） | `windows: [{title, role, subrole, main, focused, document, minimized, fullscreen, modal}]`、`listed`（Window 菜单最后一条分隔线之后的标题；可能包含不是窗口的项） |
| `tree` | `pid`、`root`、`path`、`depth`（1）、`children`（100）、`attrs`（除 `set` 和 `act` 以外的全部）、`popups`（false） | `node`、`truncated` |
| `get` | `pid`、`root`、`path`、`attrs`、`popups` | `attrs: {"AXName": value}`、`truncated` |
| `set` | `pid`、`root`、`path`、`attr`、`value`、`settleMs`（50）、`popups` | `result`、`before`、`after`；AX 失败时为 `ok: false`，`error` 为 AXError 的名称，并保留这三个字段 |
| `action` | `pid`、`root`、`path`、`name`、`settleMs`、`popups` | 同 `set` |
| `menu` | `pid`、`titles`（从菜单栏往下） | `result`、`title`、`active`；`no-menu-item` 会给出第一个找不到的标题 |
| `keys` | `pid`、`key`、`mods`（`cmd`、`shift`、`opt`、`ctrl`） | `posted`、`active` |
| `type` | `pid`、`text`（最多 200 个字符） | `posted`、`active` |
| `click` | `pid`、`x`、`y`、`mods`、`count`（1） | `posted`、`active` |
| `midi.open` | `name`、`uid`（int，或哈希成 int 的字符串） | `opened`、`source`、`destination`；已打开时为 `midi-open` |
| `midi.send` | `hex` | `sent` |
| `midi.close` | | `closed`（幂等） |
| `tap.start` | `pid`、`path`（新 WAV 文件的完整路径）、`mute`（false）、`maxSeconds`（600，最多 3600） | `sampleRate`、`channels`、`hostTime`；已有分接在运行时为 `busy`，`denied`（没有系统录音权限），`unsupported`（macOS 14.2 之前），带 `step` 和 `detail` 的 `failed`（进程还没有产生音频时为 `step: "process"`） |
| `tap.status` | | `running`、`frames`、`seconds`、`truncated` |
| `tap.stop` | | `file`、`frames`、`seconds`、`sampleRate`、`channels`、`peak`、`rms`（线性）、`anchors`（`[frame, hostTime]`）、`timebase`（`{numer, denom}`）、`dropped`、`truncated`；没有分接在运行时为 `failed` |
| `quit` | | 先回答 `ok`，然后退出 |

错误代码：`malformed`、`unknown-op`、`bad-request`、`untrusted`、`no-process`、`no-app`、`timeout`、`busy`、`popup`、`unknown-attr`、`no-menubar`、`no-menu-item`、`disabled`、`unknown-key`、`unknown-mod`、`too-long`、`bad-hex`、`midi-closed`、`midi-open`、`denied`、`unsupported`、`failed`，以及读取、写入或动作失败时的 AXError 名称。

`root` 为 `"app"`、`"main"`（`AXMainWindow`）、`"focused"`（`AXFocusedWindow`）、`"menubar"` 或 `{"title": "substring"}`；`path` 是从该根开始、用点连接的子元素索引。`depth` 和 `children` 限定 `tree` 的范围；`attrs` 指定需要的属性（`"all"` 表示全部，可按节点字段或 AX 名称指定）；`popups` 为 false 时从不读取弹出按钮的子元素，经过弹出按钮的路径会以 `popup` 拒绝。

节点是 `{i, role, subrole, title, desc, value, valueDesc, help, id, enabled, selected, pos: {x, y}, size: {w, h}, set: [attributes], act: [actions], c: [nodes], more: n}`；空的字段会省略；`more` 统计超出 `children` 的子元素个数，在深度限制处则统计全部子元素。值是 JSON（数字、字符串、布尔值），点、尺寸、矩形和范围是对象，其他元素是 `{"ref": "role title"}`。

事件：来自 MIDI 目的端的字节为 `{"event": "midi", "hex": "…", "ms": n, "hostTime": n, "timeStamp": n}`（长的 SysEx 可能分几次到达；`hostTime` 是辅助程序收到它们的时间，`timeStamp` 是数据包自己的主机时间戳，发送方没有给出时为 0）；上一次 `apps` 请求中列出的应用为 `{"event": "app", "state": "launched" | "terminated" | "activated" | "deactivated", "pid": n, "bundle": "…"}`。

进程分接（`tap.*`）把一个进程的输出缩混为立体声，录进辅助程序自己写入的 WAV 文件：格式 3（IEEE 浮点），32 位，采样率为分接的私有聚合设备的采样率（以默认输出设备为时钟；以其他采样率播放的进程会被重采样到这个采样率），文件头中的大小每秒修补一次，结束时再修补一次。读取分接期间，`mute` 让该进程自己的输出不从扬声器放出。`anchors` 把文件中的某一帧与 Core Audio 给从该帧开始的缓冲区的主机时间（`mach_absolute_time`，即 CoreMIDI 给数据包打时间戳所用的时钟）配成一对：第一个缓冲区、至少每四分之一秒一个、每次丢失之后一个，以及最后一个。主机时间 `t` 按位于它之前或与它同时的锚点 `(f, h)`，落在帧 `f + (t - h) × numer / denom / 10⁹ × sampleRate` 上。`dropped` 统计丢失的帧数；达到 `maxSeconds` 的捕获会停止写入，并注明 `truncated`。EOF、`quit` 和 SIGTERM 会让正在运行的分接把文件写完；分接及其聚合设备是辅助程序私有的，随辅助程序一起结束。

## 修改（`change_preview`、`change_apply`、`change_restore`）

M3 让 Kumi 能修改工程并撤销，M4 加入了下表中从 `key-command` 起往下的各个类型，以及 `project_save` 和 `key_command`（与 REAPER 的 `run_action` 形状相同）。传输协议与 REAPER 扩展的相同：主机的日志（journal，`crates/daw-mcp-server/src/changes.rs`）预览一项修改、记下它、以一个具名步骤应用它（`desc`，`Kumi: … (k<nonce>)`），再以另一个步骤撤销它；桥接回答 `change_preview`、`change_apply` 和 `change_restore` 时，使用与 REAPER 相同的字段。每个类型都通过一个 macOS 通道写入，并回读该值：只有 Logic 显示了新值时才是 `applied`，否则为 `uncertain`。预览不写入任何东西，并以此刻读到的引用纪元和目标值设下栅栏（fence）；应用在写入之前重新检查栅栏，不符时以 `changed` 拒绝；恢复在值可以设置时把值放回去，否则改用 Logic 自己的撤销。有对话框打开时为 `busy`，缺少辅助功能授权时为 `failed`，并以文字说明（从不为 busy）。测量的是 Logic Pro 12.3.1 中的 Scratch 工程；主机中模拟的 Logic（`simulator/logic.rs`）给出同样形状的回答。

### 各个类型、它们的通道与回读

| 协议类型 | 工具 | 通道（写入） | 回读来源 | Logic 撤销步骤 |
| --- | --- | --- | --- | --- |
| `track-volume` | `track_volume` | 轨道头的 Volume 滑块，逐步调整到 dB 文字相符为止 | 滑块的 `AXValueDescription` | 无 |
| `track-pan` | `track_pan` | 轨道头的声像滑块（0–127，64 为居中） | 滑块的值 | 无 |
| `track-mute` | `track_mute` | 在轨道头 Mute 复选框的 frame 处点击 | 该复选框 | 无 |
| `track-solo` | `track_solo` | 在轨道头的 Solo 复选框处点击 | 该复选框 | 无 |
| `track-arm` | `track_arm` | 在轨道头的 Record Enable 复选框处点击 | 该复选框 | 无 |
| `track-rename` | `track_rename` | Inspector 的 "Track:" 字段，`AXConfirm` | 轨道头的标签 | "Renaming" |
| `track-add` | `track_add` | Track 菜单（New Audio/Software Instrument/External MIDI Track） | 新的轨道头 | "Create Track" |
| `track-delete` | `track_delete` | Track > Delete Track，在其表单中回答 Delete | 各轨道头 | "Delete Tracks" |
| `marker-add` | `marker_add` | 把播放头移到那里，Marker List 的 Create new Marker | 列表中的行 | "Create Marker" |
| `marker-move` | `marker_move` | 该行的 Position 拖动条，一次一个分段 | 列表 | "Value Change" |
| `marker-delete` | `marker_delete` | 该行的 Delete 动作 | 列表 | "Delete" |
| `tempo-set` | `tempo_set` | control bar 的 Tempo 滑块（整数 BPM） | LCD 上的速度 | 无 |
| `notes-add` | `notes_add` | Event List 的 Create new Event，再逐步调整其单元格 | 列表中的行 | "Create Event"/"Value Change" |
| `notes-change` | `notes_change` | 音符所在行的单元格（音高、力度、长度、通道、位置） | 列表 | "Value Change" |
| `notes-delete` | `notes_delete` | 该行的 Delete 动作 | 列表 | "Delete" |
| `fx-bypass` | `fx_bypass` | Mixer 通道条插槽的旁通复选框 | 该插槽 | 无 |
| `key-command` | `key_command` | 快捷键命令的组合键，按名称或 id 从 `com.apple.logic10.plist` 查得（否则用出厂的 U.S. 键位集），在 Logic 位于前台时发送 | Edit 菜单的 Undo 标题，看它是否变化 | 该命令自己的步骤，或无 |
| `region-move` | `region_move` | Event List 的片段层级（Leave Folder），片段的 Position 拖动条 | Tracks area 中的项 | "Value Change" |
| `region-resize` | `region_resize` | 播放头放在新的边缘处，Set Region Start/End to Playhead（Cmd-[ / Cmd-]） | Tracks area | "Set Start to Playhead Position"、"Set End to Playhead Position"（每个边缘一个） |
| `region-split` | `region_split` | 点击片段，播放头放在分割处，Split at Playhead（Cmd-T） | Tracks area：右边部分为 `created` | "Split Regions" |
| `region-loop` | `region_loop` | Region inspector 的 Loop 复选框 | 该复选框（Tracks area 从不显示 `looped`） | "Loop Parameter Change" |
| `region-rename` | `region_rename` | Region inspector 的名称字段，`AXConfirm` | Tracks area | "Renaming" |
| `region-delete` | `region_delete` | 点击片段，按 Delete 键 | Tracks area | "Delete" |
| `loop-range` | `loop_range` | LCD 设为 Custom，它的两个定位器组的滑块，Cycle 按钮 | 定位器和该按钮 | 无 |
| `notes-quantize` | `notes_quantize` | Event List：Edit > Select > All，标题栏的 Quantize 弹出菜单 | 各行，落在网格上 | "Quantize" |
| `midi-import` | `midi_import` | `/tmp/kumi-logic` 下的 format 0 文件，File > Import > MIDI File…，面板的前往（go-to） | 新的片段和轨道 | "Import" |
| `tempo-marker-set` | `tempo_marker_set` | Tempo List：在播放头处 Create new Tempo Event，Tempo 单元格的各分段 | 列表 | "Insert Tempo" + "Value Change" |
| `tempo-marker-delete` | `tempo_marker_delete` | 该行的 Delete 动作 | 列表 | "Delete" |
| `signature-set` | `signature_set` | Signature List：Create new Time/Key Signature Change，分子滑块，分母弹出菜单 | 列表 | "Value Change" |
| `track-stack` | `track_stack` | 点击各轨道头（其余的按住 Shift 点击），Create Summing/Folder Stack（Cmd-Shift-G/H） | 各轨道头：堆栈的展开三角形 | "Create Track Stack" |
| `track-stack-flatten` | `track_stack_flatten` | 点击堆栈的轨道头，Flatten Stack（Cmd-Shift-U） | 各轨道头 | "Flatten Stack" |
| `fx-add` | `fx_add` | Mixer：按下通道条最后一个效果器下方的空插槽（或它要插到其前面的那个效果器顶部的插入条），在菜单的搜索中逐键输入，Down，Return；关闭 Logic 打开的插件窗口 | 通道条的插槽：所要求的名称出现在它的位置上 | "Insert Plug-in in Channel Strip"（空插槽）、"Change Plug-in in Channel Strip"（插入条） |
| `fx-delete` | `fx_delete` | Mixer：按下插件的列表按钮，No Plug-in（Down、Down、Return） | 通道条的插槽 | "Remove Plug-in in Channel Strip" |
| `fx-param` | `fx_param` | 插件窗口（通过其插槽的打开按钮），View > Controls，设置参数的滑块（精确值），按下它的选项或切换它的开关；之后恢复视图并关闭窗口 | Controls View 的值和文字 | 无（记在插件窗口自己的撤销中） |
| `fx-params` | `fx_params` | 同 `fx-param`，在一个窗口中设置多个 | Controls View | 无 |
| `fx-preset` | `fx_preset` | 处于编辑器视图的插件窗口（放大到标题栏显示出这些按钮为止）：按下一项或上一项设置按钮 `step` 次，或者一直按下一项，直到设置菜单显示 `preset` | 设置弹出菜单的名称 | 无（在插件窗口自己的撤销中，恢复时按下它） |
| `patch-load` | `patch_load` | 选中轨道，Library（Y）：把它的搜索框设为音色（patch）的名称，选中结果所在的行；之后清空搜索，关闭 Library | Inspector 通道条的设置按钮（音色的名称）及其插件 | "Load Patch" |
| `send-add` | `send_add` | Mixer：按下通道条的空发送插槽，Down、Down（Bus）、Right，Down 到该总线，Return；然后像 `send-volume` 那样逐步调整电平 | 通道条的发送：名为 "Bus n" 的组（以及 Logic 创建的辅助轨（aux），在 `created.aux` 下） | "Change Send in Channel Strip" |
| `send-remove` | `send_remove` | Mixer：按下发送的列表按钮，No Send（第六项） | 通道条的发送 | "Change Send in Channel Strip" |
| `send-volume` | `send_volume` | Mixer：发送的旋钮，粗调用递增，细调用设置（0 dB 附近为 0.1 dB） | 旋钮的文字 | 无 |
| `smart-control-set` | `smart_control_set` | 选中轨道，Smart Controls 面板（B）：控件的滑块，十分之一用递增，百分之一用设置 | 滑块的值和文字 | 无 |
| `automation-add` | `automation_add` | 选中轨道，它的 Track Automation Event List（命令 1032，Control-Command-E）：把事件类型设为 Fader，播放头放在每个事件的拍上，Create new Event，逐步调整新行的 Num，直到它的 Length/Info 单元格显示该参数（或调到所要求的编号），再逐步调整它的 Val；之后恢复类型、播放头和选择，关闭列表 | 列表中的行 | "Create Event"/"Value Change" |
| `automation-change` | `automation_change` | 列表：删除该事件所在的行，再按要求的位置和值重新创建 | 列表 | "Delete"，然后同 `automation-add` |
| `automation-delete` | `automation_delete` | 列表：该行的 Delete 动作 | 列表 | "Delete" |
| `metronome-set` | `metronome_set` | control bar 的 Metronome Click 复选框，只在它与要求的状态不同时按下 | 该复选框 | 无 |
| `sample-load` | `sample_load` | 通过面板服务，用 File > Import > Audio File… 导入到播放头处的一条音频轨道上，然后 Track > Convert Regions to New Sampler Track…（其表单中：zones 选 Regions，选 Sampler，填名称，按 OK） | 新的轨道头及其在 Inspector 中的通道条（乐器插槽的插件） | "Insert"，然后 "Convert Regions to New Sampler Track" |

轨道头的控件（音量、声像、静音、独奏、录音预备）和 Mixer 插槽的旁通，不需要选中轨道、也不需要 Logic 在前台就能写入；静音、独奏和录音预备复选框不接受 `AXPress` 或设置，只接受在其 frame 处的点击，这个点击在先把 Logic 带到前面之后，于系统的事件分接点（event tap）发送。名称字段、Track 菜单和各列表的按下动作只有在 Logic 是前台应用时才起作用，所以这些类型会激活 Logic，如果另一个应用一直占着前台，就以 `busy` 拒绝。读取列表时会打开它的面板和标签页，之后恢复原样；标记在播放头处创建，播放头会被移过去再移回来。

`transport_play`、`transport_stop` 和 `cursor_set`（即工具 `play`、`stop`、`set_cursor`）通过 control bar 的 Play 和 Stop 按钮以及 LCD 的小节和拍滑块完成；它们都不是 Logic 撤销中的步骤，`cursor_set` 接受 `positionBeats` 或 `position`（按工程的恒定速度计算的秒数）。无论在哪里放置播放头（`cursor_set`，以及创建标记、分割、调整大小的边缘、速度或拍号事件、导入或自动化事件的位置），它都会被精确放在所要求的拍上：LCD 的小节和拍滑块会保留播放头原有的 division 和 tick，而一次停止会让它们停在任意值上（有一次停止之后，Logic 从 2.1.3.162 开始播放，要求在第 3 小节进行的分割落在了它之后），所以如果自桥接上次放置播放头以来 Logic 播放过或停止过，或者 Mackie Control 的时间码显示的 division 或 tick 超过 1，就先用 Stop 按钮把播放头移到工程开头（Logic 停在别处时，这个按钮显示为 "Go to Beginning"；在开头时显示为 "Stop"，播放头已经在拍上），然后再逐步调整小节和拍。`undo_step_begin`/`undo_step_end` 只做计数（Logic 没有撤销块）：`undo_step_begin` 回答 `{stepId: "sN"}`，`undo_step_end` 回答 `{desc, entry: false, changes: n}`，计划中的每项修改仍是 Logic 撤销中各自独立的一步。

`bounce {epoch, startBeats, endBeats, path, format?}` 和 `export_tracks {epoch, path}` 是动作，与 `project_save` 一样是桥接的方法（不是修改，没有行，也没有撤销步骤）。`bounce` 通过 File > Bounce > Project or Section… 进行：在 Logic 的 Bounce dialog 中只勾选 Uncompressed（选中它所在的行，这样 File Type 和 Bit Depth 弹出菜单就是它自己的），选择格式的文件类型和位深度（默认 `wav24`，另有 `wav16`、`wav32f`、`aiff24`），Normalize 设为 Off，不勾选 Add to Project 和 Add to Apple Music Library，逐步调整 Start 和 End 位置的各分段，按 OK；然后通过服务操作保存面板（用前往表单指定文件夹，再填写名称），等待文件出现在磁盘上并读取它的文件头（`{epoch, bounced, format, bytes, seconds, rate, channels, bits, startBeats, endBeats}`；如果按工程的速度，文件的长度不等于范围的长度，则为 `uncertain`）。`path` 是一个尚不存在的 `.wav`（或 `.aif`）的完整路径，所在文件夹必须已存在。对话框原先勾选着的 Add to Project 会把文件放进工程的音频中，这是 Logic 撤销中的一步（"Bounce Project"），压在 Kumi 之前所做的一切之上：之后，通过 Logic 自己的撤销撤回 Kumi 的某一步（一次分割、一个音色）就被拒绝了，Edit 菜单显示的是 "Undo Bounce Project"。不勾选它时，并轨不会改变工程中的任何东西，Logic 的撤销也保持原样（先分割，再并轨，然后通过 Logic 的撤销撤回分割：已录制）。对话框会为下一次并轨记住这些选择，就像它记住格式一样。并轨包含 Logic 播放的一切，在 control bar 的 Metronome Click 打开时也包括节拍器的咔嗒声（Scratch 的第 6 和第 7 小节没有片段在播放，打开咔嗒声时并轨的峰值为 0.42）：`bounce` 让咔嗒声保持制作人设置的状态，而 `render`（M5，见下文“聆听”）会在它的并轨中关掉咔嗒声。`export_tracks` 通过 File > Export > All Tracks as Audio Files… 进行，这是一个接受文件夹的打开面板（它的选项保持制作人的设置）：用前往表单进入文件夹（文件夹必须已存在），按下 Export，回答列出 Logic 写入那里的文件（`{epoch, folder, files: [{name, bytes, seconds}]}`）。桥接在磁盘上查看的东西都经过同一个接缝（`src/disk.rs`），所以录制的场景会保存它们，回放时也从中回答。

`project_save {epoch, path?, version?}`（工具 `save_project`）与 REAPER 的形状相同：不给 `path` 时，用 Logic 的 Save 快捷键命令（Cmd-S）就地保存，如果窗口的关闭按钮显示没有需要保存的内容，就立即以 `dirty: false` 回答；给出 `path`（一个尚不存在的 `.logicx` 的完整路径）时，通过 macOS 的面板服务执行 File > Save As…（前往文件夹表单、名称字段、Save），此后工程就是那个文件。从窗口的文档及其已编辑标记回读；从不覆盖 Kumi 能看到的文件，从不以工程自己的文件作为路径（`bad-params`），从不存入不存在的文件夹（面板的 "where" 弹出菜单必须显示这个文件夹，否则为 `bad-params` 并取消面板；Logic 自己会为面板上显示过的名称留下一个空的 `<name>.logicx/Alternatives` 包）。`version` 属于 REAPER，会被拒绝。保存不是 Logic 撤销中的一步。回答为 `{saved, dirty}`。

### 栅栏与结果

- 轨道头属性的栅栏是此刻的值；重命名的是名称；删除轨道的是 `{name, index, regions}`；添加轨道的是 `{tracks, names}`（数量和每个名称）；标记的是它的完整状态或数量；速度的是 BPM；音符的是片段的音符数量和它们的值的哈希。片段相关类型的栅栏是片段的 `{startBeats, endBeats, name, track, looped}` 和片段数量；`loop-range` 的是循环区域（cycle）的 `{on, startBeats, endBeats}`；`notes-quantize` 的是音符的数量和哈希；`midi-import` 和 `track-stack` 的是 `{tracks, names, regions}`；速度事件或拍号的是它所在的行（`exists`、`bpm` 或 `timeSignature`）和列表的数量；`key-command` 的是命令的 id 和名称；`metronome-set` 的是咔嗒声的 `{on}`。
- `key-command` 的回答与 REAPER 的 `run_action` 一样（工具是 `key_command`）：`applied: {command, name, changed}`；如果 Undo 标题没有变化（Logic 没有为它生成步骤），则带上 `display.unchanged` 和 `undo.entry: false`；如果变化了，这一步以 Logic 自己的名称保留，它的恢复为 `daw-only`（在 Logic 中按 Cmd-Z）。Save、Save As、Undo、Redo、Quit、工程命令，以及名称以 "…" 结尾的每个命令（对话框）都会被拒绝。
- `region-split` 回答 `applied: {left, right}` 和 `created: {ref}`（右边部分，属于新纪元）；`midi-import` 回答 `applied: {notes, startBeats, endBeats, track, trackName, file}` 和 `created: {ref, track, trackAdded}`（Logic 把 MIDI 文件导入到所指定轨道下方的一条新轨道上，从不导入到所指定的轨道里），如果 Event List 列出的行少于文件中的音符，还会带上 `rows` 和一条说明（它只列出屏幕上的行；片段的长度表明其余的音符也已导入）；`track-stack` 回答 `applied: {index, name, kind, tracks}`（各子轨道的引用）和 `created: {ref, stack}`；`tempo-marker-set` 和 `signature-set` 在创建了该行时回答 `made: true`。
- `fx-add` 回答 `applied: {index, name, slot, plugins}`（添加之后通道条上的效果器，从上往下）和 `created: {ref, kind: "plugin"}`；它的栅栏是通道条的 `{plugins, instrument}`。`fx-delete` 回答 `applied: {deleted, plugins}` 和 `material: {name, index, strip, step}`；它拒绝删除乐器。`fx-param` 和 `fx-params` 以 `{plugin, params: [{index, name, value, text}]}` 为栅栏，也以此回答；`fx-preset` 回答 `{plugin, preset, steps}`，并带有 `material: {steps}`。`send-add` 回答 `applied: {destination, slot, level}` 和 `created: {ref, kind: "send", bus, aux?}`；`send-remove` 回答 `material: {destination, level, slot, step}`；`send-volume` 回答 `applied: {level}`。`patch-load` 以 `{patch, plugins}`（通道条的设置名称和插件）为栅栏，也以此回答。`smart-control-set` 以 `{name, value, text}` 为栅栏，也以此回答。各自动化类型接受 `{ref: track, parameter: <name as the list's Length/Info cell writes it, "Volume"> | <its number>}`，再加上 `events: [{positionBeats, value}]`（`automation_add`；整拍，值为 0 至 127）、`changes: [{index, positionBeats?, value?}]`（`automation_change`）或 `indexes: [...]`（`automation_delete`），事件按它在该参数各事件中的位置指定，从 0 开始，顺序与读取列出的相同；它们以轨道的全部自动化 `{events: [{positionBeats, value, number, parameter}]}` 为栅栏，回答 `applied: {made, removed, events}`。
- `sample-load`（`sample_load {kind: "sampler", file, ref?, positionBeats?, name?}`）接受 Kumi 能看到的一个音频文件（wav、aif、aiff、aifc、caf、mp3、m4a）的完整路径、让它导入的音频轨道（`ref`；否则为所选轨道（如果它是音频轨道），再否则为第一条音频轨道；其他类型的轨道为 `bad-params`）、位置（`positionBeats`，整拍；否则为播放头）以及乐器的名称（否则为去掉扩展名的文件名）。它以 `{tracks, names, regions}` 为栅栏，回答 `applied: {file, kind, track, name, instrument, startBeats, endBeats, sourceTrack, answered}`（回读到的新轨道的位置、名称和乐器，它的触发片段的边界，音频轨道的位置）和 `created: {ref, kind: "track", region, source}`（新轨道、它的触发片段和被静音的音频片段，属于新纪元）。`kind: "quick-sampler"` 会被拒绝：Logic 只能把音频片段转换为 Sampler、Alchemy 或 Drum Machine Designer 轨道（表单中的三种乐器），而 Quick Sampler 要靠把采样拖到它的窗口上来载入。
- `metronome-set`（`metronome_set {on}`）以 `{on}` 的形式预览咔嗒声、为它设栅栏并回答，目标为 `{ref: null, kind: "project"}`；咔嗒声已经是所要求的状态时为 `display.unchanged`（没有步骤，没有行），control bar 上没有显示它时（已用 Customize Control Bar and Display 去掉）为 `failed`。它的恢复按值把它按回去。
- `applied` 带有写入之后 Logic 显示的值。Logic 没有显示回来的写入为 `uncertain`（"Logic Pro shows … after Kumi asked for …"），并且不给这一步命名。应用的 `undo` 是 `{desc, entry}`：Logic 的历史记录生成了一步时，带有 `title`（应用时 Edit 菜单的 Undo 标题）和 `entry: true`，没有生成时为 `entry: false`。`track-add` 回答新的纪元和 `created: {ref: "<new epoch>:track:<n>"}`；`track-delete` 回答的 `material` 中含有 `{name, kind, index, volumeDb, pan, mute, solo, arm, step}`（`step` 是应用的 `desc`）。
- 能设置值的地方（每个轨道头属性、速度、标记、音符、旁通）按值恢复，这是 Logic 撤销中新的一步，回答 `undo: {desc, entry: false}`。删除的轨道无法按值重新创建，所以它的恢复使用 Logic 自己的撤销（通过 Edit 菜单执行 Cmd-Z），并且只在这次删除仍是 Logic 最上面的 Undo 步骤时才发送（监视通过修改的类型与目标以及步骤的 `desc` 来跟踪它）；它回答 `restored`、`undo: {desc, entry: false, how: "undo"}` 和一条说明（指出是 Logic 自己的撤销完成的），否则回答 `failed`（"Logic Pro's undo has other steps on top of this one …; Cmd-Z in Logic Pro can, step by step"）。`marker-delete` 的恢复在它的步骤位于最上面时先用 Logic 自己的撤销，否则按值重新创建标记（由 Logic 自己编号）。M4 的各类型中，移动、调整大小、片段循环、重命名、循环区域、量化、速度事件和拍号按值恢复；分割、删除片段、导入、拼合的堆栈和删除的速度事件只能在位于最上面时通过 Logic 自己的撤销恢复（`undo: {how: "undo"}`）；Kumi 创建的堆栈在其步骤不在最上面时按值再次拼合。第二批的类型中，添加的插件按值再去掉，删除的插件在位于最上面时通过 Logic 自己的撤销恢复（连同其设置），否则按名称恢复（使用 Logic 的默认设置；乐器不会被删除）；添加的发送在位于最上面时通过 Logic 自己的撤销撤回（Logic 创建的辅助轨也随之去掉），否则按值撤回（No Send，辅助轨留在 Mixer 中），移除的发送同理；音色只能在位于最上面时通过 Logic 自己的撤销恢复；参数、发送电平和 Smart Controls 按值恢复；预设通过插件窗口自己的 Undo 恢复，Kumi 调了多少步就按多少次；自动化事件按值恢复（删除 Kumi 创建的事件，重新创建它删除的事件）。
- 恢复所用的引用来自修改自己的纪元，并被带到轨道或片段现在所在的位置：桥接监视每次纪元推进（某个轨道头被添加、删除、重命名或移动；即 M2 观察者的规则），并为每次推进记下每个位置去了哪里，所以在添加或删除轨道之后撤销推子或重命名，**不会**回答 `stale`。预览或应用仍会以 `stale` 拒绝来自较早纪元的引用。

### 撤销栈与 `undone_in_daw`

Logic 不会把它自己的撤销告诉任何扩展，它的 Undo History 无法从外部读取，而 Edit 菜单的 Undo 和 Redo 标题只有在 Logic 验证过该菜单之后才会给出最上面的步骤的名称，Logic 自己的快捷键命令（包括制作人的 Cmd-Z）并不会做这种验证。所以桥接维护一个 Kumi 步骤的栈，顺序与 Logic 的历史记录相同，每一步都带有应用时 Logic 给它的标题、主机发送的 `desc`，以及（对于焦点刻（focus tick）本来就会读取的内容）一个探针：某个轨道头的静音、独奏、录音预备、音量、声像或名称，各轨道的名称，或速度。每个刻（焦点刻；读取菜单栏时不激活 Logic），它从两个方面查看：

- 探针：值回到 Kumi 修改之前的样子，就是这项修改被手动撤销了（`how: "undo"`），回到 Kumi 设置的值则是重做；无论标题怎么说，这些都会立即触发；
- 标题，按其当前的样子读取（~0.6 ms）：Redo 标题变成某一步的名称，表示这一步及其上面的每一步都被撤销了，Undo 标题重新变成它，表示重做。如果某一步的探针仍显示它处于已应用状态，单凭标题不会把它当作已撤销，因为 Logic 的标题不带 nonce，而制作人自己的步骤可能同名。刻本身从不验证标题：只有在 Logic 位于前台时、Kumi 自己写入之后，才通过一个 Logic 并不占用的按键等效键（key equivalent；它不会打开任何东西）刷新标题；Logic 在后台时，标题按未经验证的状态读取，此时应用会用该类型自己的 Logic 名称来命名它的步骤，并且不带 `title`。

桥接从不按下菜单栏中的项。它过去会在 Logic 位于后台时，通过按下再取消该项来验证 Edit 菜单（这是看不见的，因为菜单栏不在屏幕上）；在 M3 验收时，有一次按下让该项停留在 `selected` 状态，却没有可以取消的菜单，之后 Logic 不再接受任何点击、按键、选择或激活，也不回应任何 Apple Event，直到一次真实的点击为止，所以此后的每项修改都失败了。读取不会卡死任何东西，而探针能立即识别大多数撤销，所以宁可接受这点损失，也不冒这个风险：Logic 在后台时，制作人对标记或音符步骤按下的 Cmd-Z，要等到 Logic 下一次验证它的 Edit 菜单时（Kumi 下一次在前台写入，或制作人打开该菜单）才会被识别；第一次以未经验证的方式读取标题时，日志会说明一次。保留下来的是补救措施：在一次写入因无法到前台而以 `busy` 放弃之前，如果发现某个菜单栏项停留在 `selected` 状态，就在它的菜单上执行取消来关闭它（最多两次），并再请求一次让 Logic 到前台；仍然保持打开的菜单会记入日志，这项修改为 `busy`。通过 Logic 自己的撤销进行的恢复会先把 Logic 带到前台，然后才检查经过验证的标题。

桥接发送 `undone_in_daw {descs, how, epoch}`，其中带有标题或探针使之变动的那些修改的 `desc`，这样即使 Logic 的标题不带 nonce，主机的 `by_descs` 匹配也能认出 Kumi 的修改。对 Logic 而言 `plan_steps` 为 false：一个计划是 Logic 撤销中的若干步，按它们的标题命名。

### Logic 迫使做法与 REAPER 不同之处

- 对于调音台类的操作（音量、声像、静音、独奏、录音预备、速度、旁通），Logic **不保留撤销步骤**：这些回答 `undo.entry: false`，并按值撤销，从不通过 Logic 自己的撤销。**节拍器**（`metronome-set`）也没有撤销步骤：Logic 的历史记录为空时，把 Metronome Click 按关再按开之后（每次按下都用刷新键做了验证），它的 Edit 菜单仍然显示灰色的 "Undo"。咔嗒声是 Logic 随工程保存的一项设置，所以切换它会像循环区域一样改变工程：关闭工程时 Logic 会询问是否保存（就像一遍聆听之后那样，聆听会把它关掉再打开）。
- 速度通过滑块只能取**整数 BPM**；小数的 `bpm` 会被四舍五入，并附一条说明。
- **标记无法从外部命名**（它的名称单元格不是文本框）：`marker-add` 拒绝 `name`，标记由 Logic 自己编号。`marker-rename` 没有实现。
- `fx-move`、`send-mode`、`track-move`、`track-color` 和 `marker-rename` **没有实现**：Logic 只能通过拖动来移动插件；发送的模式是发送菜单中的一个勾选标记，辅助功能读不到，所以永远无法回读；轨道头无法拖动（没有快捷键命令能重排轨道，轨道头除了它的菜单之外不提供任何动作）；对辅助功能来说，Color 调色板的色块只是一些位置，而轨道头又不显示颜色。每一个都会以 `bad-params` 拒绝，并说明原因。
- **Mixer 的插槽菜单是 Logic 自己的**（插入插槽的、插入条的、发送插槽的、插槽列表按钮的）：辅助功能不为它们显示任何元素，菜单显示期间按下动作会被挂住，所以辅助程序在它 1.5 s 的消息超时之后回答 `cannotComplete`（在较慢的 Mac 上为 `timeout`），而菜单仍然显示着（窗口服务器把它显示在弹出菜单层级）。此后发送给 Logic 的按键都会进入这个菜单：插件菜单的搜索接受逐键输入的名称（按键码；以字符形式发送的文字也能到达，但桥接按按键输入），Down 高亮第一个结果，Return 选取它，格式由 Logic 为该通道条选择；已占用插槽的菜单中，No Plug-in 是第二项；发送菜单用方向键逐项走（Down 两次到 Bus，Right 进入，再 Down 到该总线），已占用的发送的菜单中，No Send 是第六项，排在它的各路由模式之后，而这些模式的勾选标记辅助功能读不到。Escape 可以关闭其中任何一个菜单。Logic 采用的插件或发送会按名称回读，选错的（另一个搜索结果）会再去掉。
- Logic **插入插件时会打开插件的窗口**（一个以通道条命名的对话框）；桥接会再把它关掉，并等它消失，因为它打开期间，Edit 菜单反映的是这个窗口（"Undo"、"Redo"），而不是工程。
- **插件的参数在其窗口的 Controls View 中访问**（视图菜单是标准菜单，通过弹出按钮读取），它的滑块接受精确值；Logic 自己的插件在编辑器视图中也公开滑块，但 Controls View 列出每个插件的滑块。以 `value` 要求的数字（视图显示的数：对 "-3.0 dB" 是 -3）通过对滑块的原始值做二分查找来找到，每次设置后读取它的文字：一直进行到某个值读出的数字与之完全相同为止（Stereo Out 的 Gain 在原始值 930 处读作 -3.0 dB，而过去读到第一个相差不到四分之一 dB 的值 -3.1 dB 时查找就结束了），如果没有任何值能做到，就设置在该数字自身精度内最接近的值，并由 `display.note` 说明滑块在该处的步长。在那里调整的参数和加载的预设是插件窗口自己的撤销（它的 Undo 和 Redo 按钮）中的步骤，从不属于工程的撤销，所以由 `outside_undo` 记录，恢复按值进行（预设则通过窗口的 Undo）。设置（预设）菜单也是 Logic 自己的：预设通过下一项和上一项按钮切换。**窄的标题栏会隐藏放不下的东西**：Controls View 的标题栏没有 Undo 和 Redo，小的编辑器（75% 的 Gain）甚至没有设置按钮，所以 `fx-preset` 在编辑器中进行，每次放大一级（视图菜单中的尺寸），直到标题栏显示出按钮，之后恢复原来的尺寸。"Default Preset" 不在任何设置按钮的路径上（从它按下一项会到第一个预设，从那里按上一项则停在原地）：只有窗口的 Undo 能把它找回来。
- **有自己编辑器的插件会把它的控件和标题栏的控件放在一起**：Compressor 的窗口（Logic Pro 12.3.1）在顶层列出 44 个元素，它的旋钮、电路类型按钮和字段混在标题栏的元素之间，最后是写着插件名称和通道条名称的文字；桥接每次读取窗口顶层的 40 个元素，如果被截断，就再完整读取一次，这样才能找到这个窗口。它的标题栏先列出 Side Chain 弹出菜单（"Internal"），再列出设置弹出菜单（"Custom"），所以设置是标题栏的最后一个弹出菜单；它的编辑器除了视图菜单的 50% 至 200% 之外还保留一个自己的尺寸（71%），放大一级就是菜单中比它大的下一个尺寸。它的 Controls View 列出 25 个参数：Threshold（"-17.5 dB"）、Ratio（"3.9:1"）、Attack、Release、Make Up（"0.0 dB"）、Knee（"0.7"）、Peak/RMS、Auto Gain、Distortion、Circuit Type、Side Chain Detection、Limiter Threshold、Limiter On、Auto Release、侧链滤波器的 State、Mode、Frequency、Q 和 Gain、Mix、Gain Reduction、Input Gain、Output Gain、Display Select 和 View Select（Scratch 的 Keys）。Knee、Peak/RMS、滤波器的 Gain 和 Gain Reduction 显示时没有控件（文字所在的组里有一个被禁用、没有范围的滑块）：读取时带有它们的文字和 `null` 值，不会被设置。
- 在这台 Mac 上，**Logic 新建的发送从 −∞ 开始**：给出 `volumeDb` 时 `send-add` 会设置它，没给出时会加以说明。新总线的辅助轨是 Logic 自己创建的，列在 `created.aux` 下。
- **音色会替换整个通道条**（乐器、效果器、发送）：它通过 Library 的搜索加载，搜索结果的行可以通过辅助功能选中（而 Library 浏览器中的项只接受点击），并且只有 Logic 自己的撤销能撤回它。
- **Smart Controls 和发送电平按步移动**：一次设置让 Smart Control 朝所要求的值移动百分之一、让发送旋钮移动一个细步，一次递增则移动十分之一或一个粗步。发送的细步在 -6 dB 以上是 0.1 dB，以下是整 1 dB；它的递增在 0 dB 附近是 1 dB，更低处是 10 dB（所以会越过目标电平再回来），因此电平能达到半个细步以内（要求 −6.5 得到 −6.1，−20 得到 −20.1），并按 Logic 显示的样子回答。多个发送的旋钮列在所有发送组之后，按它们的顺序排列（单个发送的旋钮紧跟在它的组之后），并按顺序配对。
- **Smart Controls 按面板中显示的位置排列**（先按行，再从左往右，依据它们的 frame）：辅助功能按自己的顺序列出面板中的滑块，而这个顺序每次显示面板时都会变。
- **自动化通过 Track Automation Event List 读写**，这是一个独立的窗口（每按一次它的快捷键命令就会再打开一个，所以桥接会关闭它自己打开的那个），其中的行是 Logic 的 "Fader" 事件，按 Logic 自己的参数编号编号（Num：Volume 是 7），参数名称在 Length/Info 单元格中，值为 0 至 127（没有 dB 文字）。事件在播放头处创建，而 LCD 把播放头放在拍上，所以事件只能取整拍；按名称要求的参数，通过把新行的 Num 从 0 往上逐步调整，直到单元格显示该名称来找到（最多到 127）。改变事件的位置或值，就是把该事件删掉再重新创建。这些都不会推进纪元；每个事件都是 Logic 撤销中的一步（"Create Event"、"Value Change"、"Delete"）。
- **并轨**通过 Logic 自己的 Bounce dialog 进行：它的弹出菜单是标准菜单，通过弹出按钮读取，但在另一个菜单刚关闭后打开的菜单可能还没有显示出来，所以会再找一次并再按一次。**导出在写入期间会占住 Logic 的主线程**：辅助程序的读取会超时，桥接在等待文件期间把这当作忙碌（busy）。导出的选项（文件名、格式）由制作人决定：在这台 Mac 上，命名模式按编号命名文件（"01.wav"），没有声音的轨道不写出文件。
- **Library 的搜索结果**位于其面板往下六层，结果的名称再往下三层，比辅助程序一次读取所能到达的深度（八层）更深：先找到表格，再读取它。
- **循环区域**（`loop-range`）通过 LCD 的 Custom 显示模式写入，其中的定位器滑块以 Logic 的 tick 为单位；它不是 Logic 撤销中的一步，关闭的循环区域会保留它的定位器，恢复时也会把定位器放回去。
- Logic 的**量化是事件的一个参数**，而不是移动：`notes-quantize` 为片段的每个事件设置 Event List 的 Quantize 弹出菜单，之后各行读到的（以及 Logic 播放的）音符都在网格上，而它们录制时的位置仍保留在底下，恢复时把弹出菜单设回去，它们就又精确地读作录制时的样子。网格是弹出菜单中的某个音符时值或三连音；没有强度（只有 100）。
- 如果被编辑的事件在选中的事件之中，Event List 会把**值的修改应用到每个选中的事件**：与其他行一起被选中的音符行，在写入之前会被单独选中（`notes-change`、`notes-delete`；量化的选择会在之后恢复）。
- 片段的**边缘按整拍移动**（`region-resize`：用 LCD 的小节和拍放置播放头，然后执行 Logic 的 Set Region Start/End to Playhead）；分割和删除需要点击片段（快捷键命令作用于键盘焦点，而设置 `AXSelected` 不会给出键盘焦点）。
- **采样通过转换变成 Sampler 轨道**：Logic 从音频片段把采样载入它的 Sampler（Track > Convert Regions to New Sampler Track…），所以 `sample-load` 先把文件导入到一条音频轨道上，再转换 Logic 选中的片段；Logic 在那条音频轨道下方创建以乐器命名的软件乐器轨道，带有一个 zone（采样区域）和一个包含其触发音符的 MIDI 片段，并把音频片段静音后留在原处。无论工程是否已保存，Logic 都会**立即把文件复制到工程包中**（导入的复制到 `Media/Audio Files/<file>`，zone 的复制到 `Media/Samples/<instrument>/<file>`），它的撤销会留下这些副本：这个类型是在 Scratch 的一个副本上测量的，从未在 Scratch 上测量，否则 Scratch 的工程包会留下它们。恢复是：在转换位于最上面时，用 Logic 自己的撤销撤回转换，再撤回紧挨在它下面的导入（"Insert"），这会把新轨道和音频片段都去掉；两者都会回读（Sampler 轨道的轨道头已消失；导入所创建的音频片段，按其轨道和边界核对），没有生效的步骤为 `uncertain`，并指明是哪一步（"its “Undo Insert” didn't land: the audio region of “kumi-tone” is still on “Vocal”"）。
- **MIDI 导入会创建一条轨道**：File > Import > MIDI File… 会把文件放到所指定轨道（"Studio Grand"）下方的一条新轨道上，所以 `created.trackAdded` 为 `true`，导入的撤销会把这条轨道连同片段一起去掉。
- 从外部创建的**速度事件**是 Logic 撤销中的两步（"Insert Tempo"、"Value Change"）；Tempo List 和 Signature List 每一步都会重建单元格，所以桥接按单元格的路径回读每一步。拍号行没有 Delete 动作：Kumi 创建的行要先选中，再通过列表的 Edit > Delete 删除。两个列表的位置都只接受整拍。
- **Track Stack** 由 Logic 自己的快捷键命令（Create Summing/Folder Stack）在选中的轨道头上创建，从不通过对话框；由 Logic 给它命名（Sum 1、Sub 1），给出 `name` 时会在创建后重命名它，这会在创建步骤上面再放一个 "Renaming"。新的文件夹堆栈（folder stack）显示为合上的，要打开（Open Track Stack）才能读取；拼合一个总和堆栈（summing stack），以及撤销它的创建，都会让其子轨道的输出留在该堆栈的总线上（这是 Logic 自己的做法）。哪些轨道在一个堆栈之下是读不到的，所以拼合后的堆栈只能通过 Logic 的撤销恢复。
- **快捷键命令**只在 Logic 位于前台时运行；`key_command` 接受名称或 id，从制作人的键位集（`com.apple.logic10.plist`，即其选定的预置）中解析，以出厂的 U.S. 键位集作为后备，并拒绝没有绑定按键的命令。
- `track-add` 把轨道放在**所选轨道的下方**（Logic 的 New Tracks 行为），所以 `index` 从 1 开始计数（0，即所有轨道之上，无法到达）；可以给出类型（`audio`、`instrument`、`external`）。

## 聆听（`hear`、`audition`、`render`）

M5 让 Kumi 能听到 Logic。这三个是桥接的方法，和修改一样在运行循环上排队。它们都不是修改、不是行，也不是 Logic 撤销中的一步，并且每一个都让 Logic 保持它原来的样子：停止状态，播放头在原来的小节和拍上（落在拍上），每条轨道的独奏、节拍器和循环区域都和原来一样。无论发生什么，包括某一遍失败，这些都会被恢复，无法恢复的列在 `notPutBack` 下。Logic 正在播放或录音时为 `busy`（"Kumi listens only while it is stopped"），有对话框打开时为 `busy`，缺少辅助功能授权时为 `failed` 并以文字说明，与所有方法一样。音频从不在传输协议上传送：每个回答都给出请求的 `folder`（一个已存在的文件夹的完整路径）中的一个文件（Kumi 不创建文件夹，也从不覆盖文件：`kumi-hear-<n>.wav`、`kumi-audition-<n>-<k>.wav`、`kumi-render-<n>.wav`，编号接在已有文件之后）。Kumi 从不听节拍器：如果 control bar 的 Metronome Click 是打开的，就在这一遍期间关闭它，回读，之后再打开（`metronome: "off for the pass"`；原本就关闭时为 `"off"`）。节拍器的状态是 Logic 随工程保存的设置，所以把它关掉再打开（每一轮播放都会这样做：这一轮的咔嗒声会把它打开）会让工程被标记为已编辑，尽管其中什么都没有变：制作人下次关闭工程时会被询问是否保存（移动播放头则让工程保持原样；在 Scratch 的一个副本上测得）。

| 方法 | 参数 | 回答 |
| --- | --- | --- |
| `hear` | `fromBeats`、`beats`（至少 0.25，按工程速度最多 600 s）、`folder`、`mute?`（false）、`solo?`（这一遍中单独独奏的各轨道引用；不给时：整个混音，不独奏任何轨道）、`epoch?` | `{file, seconds, sampleRate, channels, lead: 0, peak, rms, aligned: {by, latencyMs, errorMs}, fromBeats, beats, mute, solo, metronome, epoch, note?, notPutBack?}` |
| `audition` | `candidates`（1 至 16 个，每个是 `{ref}`，即单独独奏的一条轨道，或 `{mix: true}`，即整个混音）、`fromBeats`、`beats`、`folder`、`mute?`、`epoch?` | `{candidates: [{ref` 或 `mix: true, name, file, seconds, sampleRate, channels, lead: 0, peak, rms, note?}], aligned, fromBeats, beats, mute, metronome, epoch, notPutBack?}`：各候选按顺序排列，在一次捕获中各听一遍 |
| `render` | `ref?`（并轨时单独独奏的一条轨道；不给时：整个混音）、`fromBeats`、`beats`、`folder`、`epoch?` | `{file, seconds, sampleRate, channels, bits: 32, lead: 0, fromBeats, beats, metronome, epoch, ref?, note?, notPutBack?}` |

`peak` 和 `rms` 是对文件每个采样计算的线性值（1.0 为满刻度）；`seconds`、`sampleRate` 和 `channels` 指文件的。每一遍都以 32 位浮点、默认输出设备的采样率、立体声捕获（实验用的 Mac 上为 44.1 kHz），而渲染是 Logic 以工程采样率进行的 32 位浮点并轨。`lead` 为 0：文件从段落（part）的第一拍开始。被独奏但已被制作人静音的轨道保持无声，它那一条结果（take）的 `note` 会说明这一点；在段落结束前就停下的播放（制作人按了 Stop）会保留已播放的部分，`note` 会说明 Logic 停在哪里。引用必须属于当前纪元（否则为 `stale`），请求如果给出 `epoch`，它必须是工程的纪元。

### 一轮播放

1. 按 Logic 当前的样子读取：走带（播放时为 `busy`）、每个轨道头的独奏和静音、Metronome Click、循环区域、播放头、各片段（歌曲在哪里结束）。
2. 关闭循环区域，关闭节拍器，在 Logic 的 pid 上启动辅助程序的分接，写入 `folder` 下的 `kumi-tap-<n>.wav`（给出 `mute` 时对制作人静音），时长最多为各遍的长度再加一段余量。
3. 每一遍：恰好独奏这一遍所要求的轨道，如果 Mackie Control 的通道条与轨道一一对应（LCD 按顺序显示前八条的名称，以 Logic 最后写出的名称为准），就通过它的独奏按钮，否则通过每个轨道头的 Solo 复选框（在 Logic 位于前台时点击），并从轨道头回读；把播放头精确放在段落的第一个整拍上（从某拍中间开始的段落会从前一拍开始播放，再在它的起点处剪切）；Play；跟随播放头，直到它超过段落结尾 150 ms，并且看到显示更新至少八次；Stop。
4. 咔嗒声：把播放头放在歌曲结尾之后一个小节、什么都不播放的地方，打开节拍器，Play 一拍多一点（两次咔嗒声），Stop。
5. 停止分接，恢复 Logic（停止状态、独奏、节拍器、播放头回到它的小节和拍，然后是循环区域，Stop 按钮本会跳到它的左定位器），把每一遍的段落从捕获中剪切成单独的文件，并删除捕获。

### 跟随播放头，以及段落如何剪切

播放头通过 Mackie Control 的时间码显示来跟随。Logic 播放时，大约每 33 ms 发送一次发生变化的数字，每个数字一个 CoreMIDI 数据包，最高位在前；辅助程序给每个数据包打上它到达时的主机时间（Logic 自己的数据包所带的时间戳为 0）。一串位次递减的数字是一次更新，以它的第一个数据包计时；当控制界面的 PLAY 和 BEATS LED 亮着时，每次更新都是一个位置（bar、beat、division、tick）。一次播放的各个位置落在一条主机时间对 tick 的直线上（每个四分音符 960 tick；division 的 tick 数取能使它们成为一条直线的那个除数），误差在 0.4 ms 以内，偶尔会有一次更新晚几毫秒，拟合时会把它排除。如果 Play 之后 1.5 s 内控制界面什么都没发送（没有控制界面，或者它的显示设为 SMPTE 时间），就改为轮询 control bar 的小节和拍（`by: "transport"`），每次拍的变化以它前后两次轮询的中点计时，精度为两次轮询间隔的一半。

分接用同一个时钟给它的帧打时间戳（它的锚点），所以一拍的主机时间对应捕获中的一帧。时间码无法说明的，是分接听到 Logic 显示的内容要晚多久：在实验用的 Mac 上，这个延迟每次分接都不同，40 次分接中在 9 至 21.5 ms 之间（在同一次分接内，各次播放之间相差约一毫秒），没有哪个固定数值能把它覆盖在 10 ms 以内。所以每一轮都用第 4 步的咔嗒声来测量它：咔嗒声在它们的拍之后、相隔这段延迟出现，而那里没有其他声音，捕获中的每个段落都按这个延迟剪切（`by: "timecode+click"`、`latencyMs`）。听不到咔嗒声时（节拍器被设为无声），取 15 ms，`errorMs` 为 10（`by: "timecode"`）。`errorMs` 是剪切的不确定度：两次咔嗒声的一致程度、直线的离散程度，以及同一次分接内一次播放的变化量（1.5 ms）。

在 Scratch 上以 160 BPM 测得，每个文件都在删除前由录制器读取：在 `listen-align` 中对第 1 小节的六次 hear 和 `listen-hear` 中的两次里，Piano 片段的四个音符在文件中都比拍所指的位置晚 0.4 至 1.8 ms 开始（共 32 个音符；咔嗒声测得的延迟为 20.1 至 21.2 ms，`errorMs` 为 1.7 至 1.8）。在 Logic 自己对该段落的离线并轨中（`listen-render`，只有 Keys），同样的音符比它们的拍晚 0.2 至 0.9 ms 开始（钢琴的起音），所以听到的段落，其剪切位置与并轨放置拍的位置相差约一毫秒以内；在试听的各条结果中，音符从比拍早 0.5 ms 到晚 0.7 ms 之间开始。在采用咔嗒声之前，实验按时间码和一个固定的延迟剪切：分接的延迟在不同分接之间于 9 至 21.5 ms 之间变动，所以固定值最多会偏差 11 ms；而用同一次分接中另一次播放的咔嗒声来测量时，音符都落在 2 ms 以内。

### 渲染

`render` 就是 M4b 的 `bounce`（见上文）作用于段落：32 位浮点 WAV，Normalize 关闭，Add to Project 关闭，节拍器关闭（否则并轨会包含咔嗒声），以及请求所要求的独奏。一条轨道自身的音频，就是单独独奏这条轨道，此时它会经过 Stereo Out 的插件（它的 `note` 会说明这一点）。独奏和节拍器会被恢复。

### 权限

在实验用的 Mac 上（macOS 26.6.1），分接不需要任何提示：辅助程序在桥接所运行的终端下运行，使用该终端的辅助功能授权，而系统录音从未被请求过。在 macOS 拒绝分接的地方，辅助程序的 `tap.start` 回答 `denied`，`hear` 和 `audition` 回答 `failed`，并说明要为运行 Kumi 的应用允许系统录音（系统设置 › 隐私与安全性 › Screen & System Audio Recording）；在 macOS 14.2 之前，`failed` 会说明聆听需要 14.2。

### 需要多长时间，以及主机的调用必须留出多少时间

在 Scratch 上测得（160 BPM，段落长 1.5 s）：一次 `hear` 用了 4.3 至 4.9 s，所以一轮的开销是 2.8 至 3.4 s（其中约一秒是咔嗒声）；三个候选的 `audition` 用了 9.7 s，第一遍之后的每一遍增加它的段落长度再加约 1.2 s；只有 Keys 的 `render` 用了 8.2 s，混音四个小节（6 s 音频）的那次用了 6.4 s，大部分时间花在 Bounce dialog 和保存面板上。

桥接等待一个聆听请求的时长，是它的段落播放时长再加一段余量，按焦点最近读到的工程速度计算（没有时按 60 BPM）：`hear` 为段落的秒数加 28 s，`audition` 为每个候选的段落各加 8 s，再加 20 s，`render` 为段落加 45 s（`listen::bound_seconds`），无论截止时间怎么规定，最多两小时；给出自己截止时间的请求会一直等到那时。主机的调用也必须留出段落的长度和一段余量：1.5 s 的段落大约 5 s 得到回答，所以主机 30 s 的修改超时可以覆盖长达约 20 s 的段落，更长的段落（或包含多个候选的试听）需要按这个上限来留时间。

### 片段的音频文件

Logic 只显示音频片段所用文件的名称，从不显示它的路径。把一段纯音（tone）导入 Scratch 的一个副本（放到 Vocal 上）后，片段的项给出片段的名称（"import-tone"），它的帮助文本说明片段从哪里开始、到哪里结束，以及它是音频片段；Region inspector 给出片段的名称；Browsers 面板的 Project 标签页按名称列出文件（"import-tone.wav"），其片段列在它下面，所用的文本框都不带路径、URL 或文档。所以片段行没有 `file` 字段，运行时的 `audio_file` 对 Logic 无从读取：Kumi 改为在片段的小节范围内对它所在的轨道执行 `hear` 或 `render` 来听这个片段。
