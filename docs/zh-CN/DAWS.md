# Kumi 在 Live、REAPER 和 Renoise 中能做什么

[English](../en/DAWS.md) · 简体中文 · [日本語](../ja/DAWS.md)

Kumi 能在 Ableton Live、REAPER 和 Renoise 中工作，用的是各个 DAW 自己的说法。本页说明在哪个 DAW 中能做什么。连接方法见[指南](KUMI_GUIDE.md#连接-reaper-或-renoise)，每个修改工具见 [Kumi 如何修改你的工程](KUMI_CHANGES.md)，Live 桥接的细节见[能力矩阵](CAPABILITY_MATRIX.md)。

## 一览

✓ 表示能做；“仅 Live”“仅 REAPER”“仅 Renoise”表示只有那个 DAW 才有；“尚未”说明还缺什么；“不需要”表示该 DAW 用别的方式做到；“不能”表示 DAW 的 API 不允许；“改为”说明在那里由什么来做。

| 能力 | Live | REAPER | Renoise |
| --- | --- | --- | --- |
| **修改工程** | | | |
| 带 Kumi 自己撤销的修改，记入 HISTORY | ✓ | ✓ | ✓ |
| 不调用模型就能完成的即时短语 | ✓ | ✓ | ✓ |
| 在 DAW 内运行代码 | ✓ Python | ✓ Lua（ReaScript）、REAPER 的 API 和动作（action） | ✓ Lua |
| 一次停止一切（`/stop`） | ✓ | ✓ | ✓ |
| 把变奏做成带编号的版本（take），可保留、修改或丢弃 | 尚未 | ✓ 条目 | ✓ |
| 采样映射：按名称、音高和响度把声音排到键盘上，并整理乐器 | 尚未 | 尚未：只有鼓组（`drum_kit`） | ✓ |
| 在真正的击打处切开录音，并排好切片来演奏 | 尚未 | 尚未 | ✓ |
| 交付文件：每个素材一个区域，按项目的命名规则，先试运行 | 尚未 | ✓ | 尚未 |
| 找出轨道为何响两遍或完全不响，并以一步撤销建立侧链、并行总线和分轨 | 尚未 | ✓ | 尚未 |
| 把重复的杂事保存为脚本：先作为一步撤销运行一次，你同意后再保存为 DAW 自己的动作 | 尚未 | ✓ | ✓ |
| 知道 DAW 是否正在录音 | ✓ | ✓ | ✓ |
| 在 DAW 的状态栏显示一行 | ✓ | 不能：REAPER 的 API 没有状态栏（改用通知） | ✓ |
| 打开保存的工程 | ✓ | 尚未：需要你自己打开（`kumi -r` 和设置中的重启都不会打开） | 尚未：需要你自己打开（`kumi -r` 和设置中的重启都不会打开） |
| 告诉你 Kumi 关闭期间的变化 | ✓ | ✓ | ✓ |
| 对话跟随保存的工程 | ✓ | ✓ | ✓ |
| **聆听** | | | |
| 聆听轨道或混音 | ✓ | ✓ | ✓ |
| 离线渲染后聆听，再大声播放给你听 | 改为：直接听 Live 播放的声音 | ✓ | ✓ |
| 对照参考曲试听候选，以及在保持打开的装置上运行 `/goal` | ✓ | ✓ | ✓ |
| 判定自己的修改、`/loop`、匹配参考曲 | ✓ | ✓ | ✓ |
| 片段、对象（item）或采样背后的音频文件 | ✓ | ✓ | ✓ |
| 把轨道分离成分轨（stem） | ✓ Live 自己的分离器 | 尚未：没有 Kumi 能用的分离器 | 尚未：没有 Kumi 能用的分离器 |
| 把音频转换为 MIDI | ✓ 鼓、旋律与和声 | 尚未：只有文件中有音高的声部（Basic Pitch） | 尚未：只有文件中有音高的声部（Basic Pitch） |
| **FOCUS** | | | |
| FOCUS 和节拍灯跟随 DAW 的变化 | ✓ | ✓ | ✓ |
| 以树状显示轨道的设备 | ✓ | ✓ FX 链和容器 | ✓ Track DSPs、Doofer 和 Splitter |
| 音符编辑器中的音符 | ✓ | ✓ MIDI 编辑器 | 改为：Pattern Editor 和 Phrase Editor 的行 |
| Pattern Editor 和 Phrase Editor 的行 | 仅 Renoise | 仅 Renoise | ✓ |
| 时间线一览 | ✓ 编曲视图 | ✓ 编排视图 | 改为：Pattern Matrix |
| Pattern Matrix | 仅 Renoise | 仅 Renoise | ✓ |
| 启动器网格 | ✓ 会话视图 | 仅 Live | 仅 Live |
| 以琴键图显示乐器的键区 | 尚未：不绘制 Sampler 的区域 | 仅 Renoise | ✓ |
| 轨道的路由：发送、接收和硬件输出 | 尚未 | ✓ | 尚未 |
| 混音器：电平、声像、静音和独奏 | 尚未 | ✓ 带峰值表 | ✓ 没有电平表（Renoise 的 API 不提供） |
| 指向 DAW 中的对象（**Ask Kumi about this**） | ✓ | 尚未：REAPER 中还没有对应的动作 | 尚未：Renoise 中还没有对应的菜单项 |
| **素材库与教程** | | | |
| 观看视频教程 | ✓ | ✓ | ✓ |
| 查找并载入 DAW 的预设 | ✓ | ✓ | ✓ |
| 列出你自己的工程（`my_sets`） | ✓ | 尚未：不读取 `.rpp` 文件 | 尚未：不读取 `.xrns` 文件 |
| 搜索 DAW 的手册 | ✓ | 尚未：不搜索 REAPER 的手册 | 尚未：不搜索 Renoise 的手册 |
| 按 ID 载入浏览器项目 | ✓ | 仅 Live | 仅 Live |
| 要先配置才显示的插件参数 | ✓ | 不需要：所有参数本来就在 | 不需要：所有参数本来就在 |
| 在脚本够不到的地方操作 DAW 窗口的 Kumi 的“手” | ✓ | 尚未 | 尚未 |
| **Live 自己的** | | | |
| Max for Live 设备、Kumi Ears | ✓ | 仅 Live | 仅 Live |
| Live 的 Extensions SDK、Willington | ✓ | 仅 Live | 仅 Live |

## Live

- Kumi 的桥接是一个 Remote Script，旁边还有 Kumi 的 Live 扩展和 Willington（[能力矩阵](CAPABILITY_MATRIX.md)）。
- 整个计划在 Live 中只算一次撤销（Cmd-Z，Windows 上为 Ctrl-Z）。
- 脚本做不到的事，用 Live 自己的菜单完成：冻结、并轨（bounce）、分轨、音频转 MIDI 等（[Live 自己的命令](KUMI_GUIDE.md#live-自己的命令)）。

## REAPER

通过 Kumi 的扩展（`reaper_kumi`）工作。需要 macOS、Windows 或 Linux 上的 REAPER 7 或更高版本（[运行环境](SUPPORT_MATRIX.md#reaper-与-renoise)）；已在 macOS 上的 7.82 验证，尚未在 Windows 或 Linux 上试用。

### 你的设置

Kumi 每次查看时都会读取你对 REAPER 的设置，并按它工作，而不是按 REAPER 的出厂默认：

- 新对象的淡入淡出（或没有）、新对象是否循环源、新轨道的音量、工程的时基；
- 装了 SWS、ReaPack、ReaImGui 和 js_ReaScriptAPI 中的哪些（只在装了时才使用）；
- 正在使用的渲染设置（来源、范围、格式、采样率、文件名模式），修改之前会先告诉你。

Kumi 导入的文件，只在你导入的媒体会循环时才循环其源，而且和你自己导入的一样不加淡入淡出。

### Kumi 能修改什么

每项修改都是 REAPER 撤销中的一步，整个计划也是一步。全部工具见 [KUMI_CHANGES](KUMI_CHANGES.md#kumi-能在-reaper-中修改什么)。

- **轨道**（`track_add`、`track_move`、`track_delete` 及其他 `track_…`）：添加（也可作为文件夹轨道）、移动、删除、改名、着色、显示或隐藏、展开或折叠文件夹、锁定；轨道编组、轨道模板（`track_template_insert`）、固定轨道层与拼接（`lane_plays`、`lane_name`、`lane_comp`）、剃刀区域（`razor_set`）。
- **混音器**（`track_volume`、`track_pan` 等）：音量、声像及声像模式、宽度、静音、独奏、极性、声道数、送往上级文件夹或主控的发送、FX 链的开关；录音预备、录音输入、监听。
- **路由：** 发送与接收（`send_…`、`receive_add`）、3/4 声道上的侧链、硬件输出（`hw_out_…`）、送往硬件的 MIDI 输出。
- **FX**（`fx_…`）：按 REAPER 的 FX 浏览器中显示的名称添加，删除、移动、复制、旁通、离线；湿声、引脚、预设、插件的完整状态、FX 链文件；容器与并联 FX；参数（一次最多 256 个）、ReaEQ 的频段；参数调制（LFO、音频控制信号、链接、学习）。
- **包络与自动化**（`envelope_…`、`automation_item_…`、`automation_override`）：显示与启用、包络点、自动化对象（automation item）、自动化模式。
- **对象（item）与 take**（`item_…`、`items_glue`、`items_render`、`take_…`、`stretch_markers_…`）：添加、导入文件、移动、分割、修剪、淡入淡出、编组、删除、复制；只在你要求时才粘合（glue）或渲染；take、伸缩标记、take 标记。
- **MIDI**（`notes_…`、`midi_events_…`）：音符的添加、修改、删除、量化、人性化；CC、弯音、音色切换、压力、文本、SysEx。
- **标记、区域与循环**（`marker_…`、`region_add`、`regions_add`、`region_render`、`ruler_lane`、`time_selection`、`loop_range`）：标尺层、区域渲染矩阵；一步加入多个区域。
- **速度与工程**（`tempo_set`、`tempo_marker_set`、`metronome_switch`、`project_…`）：速度标记、拍号与渐变、节拍器、网格、时基、备注、标题、作者。
- **鼓组与目标**（`drum_kit`、`scratch_keep`）。
- **脚本**（`run_lua`、`api_call`、`run_action`）。

它还能播放、停止、移动编辑光标，以及保存工程（或 REAPER 的下一个版本）、轨道模板和 FX 链。

### 如何聆听

- 离线渲染这一段（比播放更快），听过之后再大声播放给你听，最长一分钟。单独一条轨道用的是它的推子前分轨。
- REAPER 播放时，它可以边播放边听主输出（默认 8 秒）。渲染会停止 REAPER 的播放。
- 要判定的修改用最后一个 ReaLimit 保持电平，没有时用 Kumi 加上的 JS: Volume Adjustment。目标在轨道的临时副本（“Kumi · try …”）上试候选，最好的留在“Kumi · Goal best”上。

### 工作流程

- **交付**（`render_files`）：为游戏或客户交付文件，每个声音一个。Kumi 把条目归成素材（叠在一起的几层是一个脚步声），按工程中区域遵循的命名规则（或按你说的）为新的素材命名，加上在下一个声音之前停止的尾音，并一步为每个素材建立区域（每个区域渲染该素材自己的轨道）。然后它把 REAPER 将要写出的文件与这些名称核对（不渲染任何东西），再只渲染这些文件：只渲染这次交付的区域，各自的轨道经由主输出混合；除非你同意，不使用渲染对话框的标准化或第二格式，也绝不覆盖已有的文件；事后恢复渲染设置。
- **路由**（`routing`）：轨道为何太响、无声或相互抵消。Kumi 读取每条轨道到达主输出的所有路径（文件夹、发送、硬件输出），并用名称指出可能的错误：经由只做求和的总线被听到两次的轨道、环路、哪里都听不到的轨道、落在轨道没有的通道上的侧链、极性反相的重复。它把侧链（通过 ReaComp 让底鼓压低贝斯）、并行总线或分轨作为 REAPER 撤销的一步建立，并重新读取路由来检查。
- **音效变奏**（`variations`）：为一个条目做出带编号的版本 V1、V2……，每个都是放在它所在轨道上、源条目之后的副本，音高、速率、起点、长度、淡入淡出和音量各不相同（MIDI 条目则改变音符、力度和时值）。Kumi 会检查每个版本既不同于源和其他版本，又仍是同一个声音（保留起音和你指定的音符），渲染出来听，并按你说的保留、修改或丢弃；用 `render_files` 可以各自交付一个文件。
- **杂事脚本**（`scripts`）：你反复做的杂事，按你的工程用 Lua 写好，并作为 REAPER 撤销的一步运行一次。你同意后，Kumi 会保存它以便按名称再次运行，并把它做成动作列表（Scripts/Kumi）中的动作，可以设快捷键或工具栏按钮。它不会保存点名本工程 ID 或 Kumi 的 ref 的脚本，并会说明哪些地方按位置取用。
- 用声音文件制作**鼓组**：每个打击垫一个 ReaSamplOmatic5000，做成打击垫轨道的文件夹或一个容器，踩镲互相切断。
- **来自教程和参考曲的声音：** 先 `watch_video`，再用 ReaEQ、ReaComp、ReaSynth 或你的插件做出来，靠耳朵判定。
- **预设：** 在 REAPER 的资源文件夹中查找并载入 FX 链、轨道模板和工程模板、JSFX 及 FX 预设。

### 限制

- **只有 REAPER 的撤销**能撤回 `run_lua`、`api_call` 和 `run_action`。`run_action` 绝不运行名称以“…”结尾的动作（对话框），也不运行退出、打开/关闭/保存工程、渲染，或在 REAPER 中撤销和重做的动作。
- **只有 Kumi 的撤销**能撤回 REAPER 的撤销跳过的内容：循环播放（repeat，`loop_repeat`）、节拍器开关（`metronome_switch`）、ReaEQ 频段的类型或开关（`fx_eq_band`）、自动化覆盖（`automation_override`）、工程的备注、标题、作者、网格和时基（`project_notes`、`project_info`、`project_grid`、`project_timebase`），以及什么都没开启的参数调制。
- 用 Kumi 的撤销撤回参数调制时，会关掉它的开关，但 REAPER 一旦创建就会保留的设置仍然留着；HISTORY 的那一行会说明。
- 所有轨道一次被全部替换的工程会被当作另一个工程：Kumi 的引用失效，它会重新读取。
- `track_midi_out` 需要在 REAPER 的偏好设置中有 MIDI 输出设备。
- REAPER 被对话框或渲染占用时，修改会立即被拒绝。

## Renoise

通过 Kumi 的工具（`com.kumi.Bridge.xrnx`）工作。需要 macOS、Windows 或 Linux 上的 Renoise 3.5 或更高版本（[运行环境](SUPPORT_MATRIX.md#reaper-与-renoise)）；已在 macOS 上的 3.5.4 验证，尚未在 Windows 或 Linux 上试用。

### 你的设置

Kumi 每次查看时都会读取这首歌曲的设置并照着做：BPM（2 倍或 4 倍的惯例保持不变）、LPB、TPL、Edit Step、大多数模式的长度、编组及其颜色，以及已安装的工具（包括 Paketti）。

### Kumi 能修改什么

每项修改都是 Renoise 撤销中单独的一步。全部工具见 [KUMI_CHANGES](KUMI_CHANGES.md#kumi-能在-renoise-中修改什么)。

- **歌曲**（`song_bpm`、`song_lpb`、`song_tpl`、`song_groove` 及其他 `song_…`、`metronome_switch`、`edit_settings`）：BPM、LPB、TPL、律动（groove）、节拍器、编辑设置（Follow Player、Edit Step、八度、录音量化）、名称、艺术家、注释、余量（headroom）。
- **轨道**（`track_…`）：音序器轨道、发送轨道和编组轨道的添加、移动、编组、删除、改名、着色、折叠；音符列和效果列；音量、声像、宽度、静音、独奏、输出路由和延迟。
- **模式序列与矩阵**（`slot_…`、`slots_drop`、`loop_range`、`loop_pattern`）：槽位的插入、删除（多个可在一步内完成）、移动、克隆、独立（unique）、改播另一个模式或它的变奏；段落（section）、矩阵中的静音和颜色；循环。
- **模式与行**（`notes_write`、`notes_clear`、`notes_transpose`、`notes_humanize`、`lines_copy`、`pattern_…`、`automation_write`）：用 Renoise 的记法写音符、乐器、音量、声像、延迟和效果列；模式的名称、长度和别名（alias）；自动化。
- **Track DSPs**（`device_…`、`doofer_…`、`splitter_set`、`notepad_write`、`meta_destination`）：原生和插件设备、参数、预设、设备链文件（.xrnt）；Doofer 及其宏、Splitter、Notepad、元设备指向哪里。
- **乐器与采样**（`instrument_…`、`sample_…`、`keyzone_…`）：添加、复制、移动、载入（.xrni、.sfz、采样）、用采样文件制作；宏；采样设置、切片、键区（keyzone）、鼓组布局。
- **调制与采样 FX**（`modset_…`、`moddevice_…`、`chain_…`、`chaindevice_…`）。
- **乐句**（`phrase_…`、`phrases_drop`）：添加（连同其行）、删除（多个可在一步内完成）、映射到琴键、播放方式和行；编写并渲染乐句脚本；.xrnz 文件。
- **插件与 MIDI**（`plugin_…`、`midi_set`）：插件乐器及其参数和音色（program）；MIDI 乐器的输出。
- **目标与脚本**（`scratch_keep`、`run_lua`）。

它还能播放、停止、把一个音符大声播放出来、在 Renoise 中显示某个对象，以及保存歌曲、乐器和设备预设。

### 如何聆听

- 离线渲染这一段，听过之后再大声播放给你听，最长一分钟。单独一条轨道用的是 Renoise 的轨道独奏。
- Renoise 的脚本听不到它正在播放的声音，而且渲染会停止播放：播放时，Kumi 只渲染你指定的那一段。
- 要判定的修改用最后一个 Maximizer 保持电平，没有时用 Kumi 加上的 Gainer。
- Renoise 的演示版不能渲染，所以 Kumi 在那里无法聆听；`kumi doctor` 会说明。

### 工作流程

- **变奏**（`variations`）：把一个模式轨道的行（比如你用 `0S`、`0R`、`0B` 命令写的切片编排）、一个乐句，或某个乐器的新乐句（“给我做 9 个乐句”）做成带编号的版本 V1、V2……。你指定的行被当作这段素材的核心保留下来，其余部分（切片、滚奏、反向、鬼音、重音、音区）被改变；Kumi 会检查每个版本既不同于原素材和其他版本，又保留原来的想法、调性和网格，并先把各个版本渲染出来听一遍（Renoise 播放时不渲染）。每个版本是一个乐句，或在序列末尾的“Variations of …”段落里有自己的槽位，和模式的其余轨道一起播放。说“保留 1、3、5，其余的改一改”，就只改那些，每个朝不同方向，其余的在一步之内删掉；你编辑过的版本不会被动。保留下来的版本会改名为“V3 · … · kept”，Kumi 重启后也记得这些版本。
- **乐句：** 用 Renoise 的记法写，或编写乐句脚本再渲染成行；把模式的行变成可以从任意键弹奏的乐句（`variations` 的 extract），而由于乐句的 LPB 是固定的，更快的副本用 `0Zxx` 切换。
- **采样映射**（`map_samples`）：把一个文件夹里的声音，或某个乐器自己的采样，一步排到键盘上。Kumi 读取每个文件名里的音名（“C3”“A#2”“C-4”“060”）、力度（“v3”“vel64”“ff”）和轮循（“rr2”），听每个采样的音高和响度，并判断这些名称把中央 C 写成 C3、C4 还是 C5。它规划根音和键区、按响度排序的力度分层、轮流发声的轮循，以及每种演奏法各一个乐器；它会说明自己的决定，只询问它判断不了的事。整理会找出没有任何音符演奏的乐器、副本，以及两个乐器共有的采样，并且只改动你同意的部分。
- **切片**（`chop`）：在真正的击打处切开一段 break。每一下都精确到帧，并判断是底鼓、军鼓、镲、鬼音还是滚奏，再根据长度推算 break 自己的速度。Kumi 一步切好（速度一致时落在歌曲的网格上），并按录音原样排好切片，在两行之间的击打用延迟列表示。它会渲染结果，告诉你击打落得多准、与录音相消到多深。更稀的密度（去掉鬼音和滚奏；只有底鼓和军鼓）会作为变奏的版本出现，可保留或丢弃。
- **杂事脚本**（`scripts`）：你反复做的杂事，按你的歌曲用 Lua 写好，并作为 Renoise 撤销的一步运行一次。你同意后，Kumi 会保存它以便按名称再次运行，并把它做成 Tools > Kumi > 它的名称 菜单项，可以在 Renoise 的 Keys 偏好设置里设快捷键。它不会保存点名 Kumi 的 ref 的脚本，并会说明哪些地方按位置取用。
- **用采样文件制作乐器：** 带静音组的鼓组，或带力度分层的键盘乐器，都放在一个乐器里。
- 用设备制作 **Doofer**，并给宏命名。
- **来自教程和参考曲的声音：** 先 `watch_video`，再用原生设备或你的插件做出来，靠耳朵判定。

### 限制

- **只有 Renoise 的撤销**能撤回 `run_lua`。
- **只有 Kumi 的撤销**能撤回 Renoise 的撤销跳过的内容：静音和独奏（`track_mute`、`track_solo`）、循环和模式循环（`loop_range`、`loop_pattern`）、节拍器开关（`metronome_switch`）、编辑设置（`edit_settings`）和插件的自动休眠（`plugin_suspend`）。
- 行的编辑（`notes_write`、`lines_copy`、`phrase_write` 等）经过 Renoise 自己的撤销和重做之后，Kumi 对它们的撤销会被拒绝：它分不清 Renoise 的重做和你自己写下的相同的行。Renoise 的撤销仍然可以撤回。
- 在长时间的会话中，载入和卸下 AU 乐器之后，Renoise 的撤销历史有时会变空（原因不明）。
- FOCUS 的 Mixer 没有电平表：Renoise 的 API 不提供。
- 渲染内容很密的歌曲时，可能会短暂地以忙碌为由被拒绝；请再试一次。
