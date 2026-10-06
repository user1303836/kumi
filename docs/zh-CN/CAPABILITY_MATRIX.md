# 能力矩阵

[English](../en/CAPABILITY_MATRIX.md) · 简体中文 · [日本語](../ja/CAPABILITY_MATRIX.md)

桥接在 Live 的各个领域覆盖了什么、由哪个通道完成工作、修改如何撤销，以及是否已在真实的 Live 上测试过。工具本身列在[用户指南](USER_GUIDE.md)中；Kumi 在这些工具之上构建的修改工具，见 [Kumi 如何修改你的工程](KUMI_CHANGES.md)。

## 如何阅读本矩阵

**通道**指工作在哪里完成：

| 通道 | 说明 |
| --- | --- |
| Remote Script | `AbletonMcpBridge`，在 Live 内部，通过 Live 的 Python API 工作 |
| 扩展 | Kumi 的 Live 扩展，运行在 Live 的 Extension Host 中（Live 12.4 或更高） |
| Willington | 原生绑定：从带有 Willington 文件的版本起放在 Kumi 的桥接中，在此之前可以自己安装；在 `/willington` 开启之前一直关闭，适用于它覆盖的 Live 构建版本：macOS ARM64 上的 12.4.15b4 和 b5，Windows x64 上的 12.4.15b5（[Willington](WILLINGTON_INTEGRATION.md)） |
| 桥接 | 桥接进程本身，不经过 Live |

只有当所连接的 Live 具备某个工具所需的操作（桥接在连接时得知这一点），并且部署策略允许时，才会提供该工具（见[用户指南](USER_GUIDE.md)）。

**撤销：** *精确* 表示 `live_undo` 会恢复修改前记录的状态，前提是此后没有其他东西改动过它。*kept* 表示桥接无法恢复，但 Live 自己的撤销可以（删除片段、轨道、场景或定位标记，清除一段范围）。*不可撤销* 表示桥接和工具都不保证能恢复（裁剪片段、清除全部包络、删除设备，以及随机化宏等机架操作）。*无* 表示没有可撤销的东西：播放、瞬时动作、读取。详见 [Live 安全](LIVE_SAFETY.md)。

**真实 Live** 一栏说明该领域最近一次在真实 Live 上运行是在哪里：*验收* 指 Kumi 的验收运行（`accept_live`），在 Live 12.4.15b5 上使用桥接 1.0.63；*早期运行* 指在 Live 12.4.15 beta 上使用桥接 1.0.0 至 1.0.65 的其他运行；*7 月运行* 指桥接最初的几次运行，在 Live 12.4.5b8 上使用桥接 0.1.0。*尚未* 表示只有测试，这些测试在 macOS、Linux 和 Windows 上针对假 Live 对象运行。这些记录的索引见[实现状态](IMPLEMENTATION_STATUS.md#证据)。

## 各领域的覆盖情况

| 领域 | 覆盖内容 | 通道 | 撤销 | 真实 Live |
| --- | --- | --- | --- | --- |
| 轨道与场景 | 创建 MIDI 轨道、音频轨道、返回轨道和场景；复制轨道和场景；重命名；颜色；折叠和视图设置；删除轨道、场景和返回轨道 | Remote Script | 精确；删除轨道或场景：kept；删除返回轨道：不可撤销 | 验收；早期运行 |
| Session 片段与音符 | 带音符的 MIDI 片段（力度、概率、力度偏差、释放力度、静音）；音符编辑、量化、复制；带种子的 MIDI 变换与生成器；片段循环、启动模式和量化、连奏（legato）、颜色、静音；裁剪、复制循环、拖动播放位置（scrub）；删除片段 | Remote Script | 精确；删除：kept；裁剪：不可撤销 | 验收 |
| 音频片段与文件 | 增益、音高、变速（warp）模式、变速标记、淡入淡出、RAM 模式；把音频文件放进 Session 槽、编曲视图或某条 take lane；替换 Simpler 的采样；把采样放到 Drum Rack 打击垫上 | Remote Script；不经 Browser 把采样放到打击垫上时使用扩展 | 精确 | 早期运行；变速标记尚未 |
| 编曲视图 | 创建、复制和移动片段；定位标记及跳转到定位标记；take lane（读取、重命名、把音频放进 lane）和 comp（读取）；带音符的 MIDI 片段；清除一段范围 | Remote Script；MIDI 片段和清除范围使用扩展 | 精确；清除范围：kept | 验收 |
| 自动化 | Session 片段包络（创建、插入点、删除一段范围、全部清除）；某个参数在某一时刻的自动化值；读取编曲视图中的自动化 | Remote Script | 精确；清除全部包络：不可撤销 | 早期运行 |
| 调音台与路由 | 音量、声像、发送、静音、独奏、预听（cue）、交叉渐变器、分离立体声（split stereo）；机架链调音台；轨道输入输出路由、录音准备（arm）和监听；设备输入与侧链 | Remote Script | 精确 | 验收 |
| 设备 | 从 Browser 加载；参数、开关、移动、复制、删除；机架、链、鼓垫、宏和变体；保存、调用和渐变（morph）设备状态；Drift、Drum Cell、EQ Eight、Hybrid Reverb、Meld、Looper、Simpler、Wavetable、Roar、Shifter、Spectral Resonator 和 CC Control 的设置；插件参数、预设和编辑器窗口 | Remote Script；复制时使用扩展或 Remote Script | 精确；删除设备和机架操作：不可撤销 | 验收；早期运行（Browser 中的每个设备） |
| 宏映射、Follow Actions | 宏和变体的名称、把参数映射到宏、机架链区域（zone）、Session 片段的 Follow Actions | Willington | 精确 | Willington 记录 |
| Browser 与音色库 | 搜索、根目录、查看、预览；Live 的库数据库（标签、类型、插件列表；需主动开启，只读） | Remote Script；数据库由桥接读取 | 无 | 早期运行 |
| 走带与歌曲 | 播放、停止、继续、位置、循环、节拍器、穿插录音（punch）、敲击速度（tap tempo）、微调（nudge）；速度、拍号、摇摆（swing）、启动和录音量化；音阶与调律；律动池；Link 设置；Live 自己的撤销和重做；把几项修改合成 Live 的一个撤销步骤 | Remote Script | 设置：精确；动作：无 | 验收 |
| 播放与录音 | 启动片段和场景、按住启动按钮、受保护的场景试听、紧急停止；Session 和编曲视图录音；捕获 MIDI 和场景 | Remote Script | 无（只是播放）；捕获的片段：精确 | 验收；早期运行 |
| 离线渲染 | 音频轨道自身的片段（在经过其设备之前），比实时快许多倍 | 扩展 | 无 | 验收 |
| 视图与选择 | 选中的轨道、场景、片段、设备、参数和链；Session 或编曲视图、缩放、细节视图；Live 的对话框；状态栏消息 | Remote Script | 在 Live 允许恢复之处：精确；对话框和消息：无 | 早期运行 |
| 音频分析 | 响度（BS.1770、EBU R128）、真峰值、频谱与动态、参考对比、结合工程中设备的诊断（[音频智能](AUDIO_INTELLIGENCE.md)） | 桥接 | 无 | 7 月运行；FFmpeg 对照基准 |
| 音频捕获 | 通过 Session Resampling 捕获某条轨道的输出，需经同意，带看门狗和清理 | Remote Script 和桥接 | 事后清理 | 7 月运行 |
| 项目与文件 | 项目信息和经过验证的备份副本；工程快照与差异对比；不经 Live 读取、检查（lint）和比较已保存的 `.als` 文件；把文件导入项目；保存在工程中的 Kumi 笔记 | Remote Script；`.als` 文件由桥接处理；导入使用扩展 | 数据笔记：精确；其余：无 | 7 月运行（信息、备份） |
| 事件 | Live 中正在发生的变化（走带、选择、名称、调音台、参数、结构、右键点击），或通过观察和轮询获得 | Remote Script；右键点击使用扩展 | 无 | 早期运行；验收（右键点击） |
| 实时控制 | 发往已待命（armed）参数的 UDP JSON、OSC 和 XY 数据包（以及带 Max 标签的数据包）（[实时控制](REALTIME_CONTROL.md)） | Remote Script | 写入经过检查；解除待命即停止 | 7 月运行 |
| Live 内的 Python | `live_run_python`，用于其他工具都不覆盖的操作；仅在 `full` 策略中可用 | Remote Script | Live 撤销中的一步；没有 `live_undo` | 尚未 |

## 保留的操作

操作注册表中有一些桥接从不执行的操作。调用它们会被拒绝并给出原因，也没有任何工具提供它们：

- `arrangement.automation.create`、`.delete`、`.point.insert`、`.point.delete`：编辑编曲视图中的自动化；
- `audio.comp.read`：按 Live 的 comp 编辑器显示的样子读取 comp 区域；
- `project.new`、`.open`、`.save`、`.save-as`、`.collect`、`.export`、`.bounce`：在能力资源中作为限制报告；
- `session.discover`：一个别名，由 `discover` 提供。

`browser.preview.start` 和 `browser.preview.stop` 在[能力清单](../evidence/capability-manifest.json)中也被标为保留，但只要 Live 的 Browser 能够预览，Remote Script 就会执行它们，桥接也提供 `live_browser_preview`。创建 take lane 以及在 take lane 中创建 MIDI 片段已在注册表和 Remote Script 中，但没有工具提供它们。

## Live 的 API 不提供的功能

对 Live 12.4.15b5 的 Python API 所做的普查（[LOM 审计](../evidence/lom-audit.md)）把每个类和成员都与 Remote Script 进行了对照。剩下的要么是 Live 的 Python API 和 Extensions SDK 都无法触及的，要么不在范围之内：

| 不提供 | 替代方案 |
| --- | --- |
| 保存、打开或导出工程；Collect All and Save | 已保存工程的经验证备份；把单个文件导入项目。请在 Live 中保存。 |
| 导出混音或分轨（stems）；冻结和合并（flatten） | 对音频轨道自身片段的离线渲染；通过 Resampling 录音 |
| 创建编组轨道 | — |
| 编辑编曲视图中的自动化 | 读取它；Session 片段中的包络 |
| 把宏或调制器映射到参数 | Wavetable 和 Drift 的调制矩阵；通过 Willington 进行宏映射 |
| Follow Actions | 通过 Willington |
| comp 编辑，删除或试听 take lane | 读取 lane 和 comp、重命名 lane、把音频放进 lane |
| 逐音符 MPE（压力、滑音、逐音符调音） | 概率、力度偏差、释放力度、静音 |
| 插件自身的窗口或隐藏状态 | 它的参数、预设，以及打开或关闭它的窗口 |
| 轨道播放时的音频 | 通过 Resampling 捕获；离线渲染 |
| Browser 相似度搜索、Packs、Cloud | Live 的库数据库：标签、类型、插件列表 |
| 偏好设置、音频与 MIDI 设置、授权；分轨分离；视频轨道 | — |
| 重新打开工程后仍保持不变的对象标识 | 引用只在一次连接内有效；探查时会重新读取 |

有意不纳入范围的：Push 及其他硬件控制界面（桥接是一个 Control Surface，但不读取原始 MIDI）、通用的 OSC、网络、串口或传感器连接（实时控制仅限回环）、外部 Link 对等端和 Link Audio，以及桥接内部的 Max for Live 设备。Kumi 自己就能制作 Max for Live 设备；见 [Kumi 指南](KUMI_GUIDE.md#制作-max-for-live-设备)。
