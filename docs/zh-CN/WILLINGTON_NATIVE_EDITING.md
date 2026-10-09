# Willington 原生编辑

[English](../en/WILLINGTON_NATIVE_EDITING.md) · 简体中文 · [日本語](../ja/WILLINGTON_NATIVE_EDITING.md)

Willington 的仓库是私有的；本指南中的上游链接需要访问权限。

本指南说明原生 Python API。Kumi 的受保护工具和凭据验证式 `/willington` 设置见 [Willington 集成](WILLINGTON_INTEGRATION.md#原生编辑及其自测)。原生编辑要求安装已验证的 **Live 12.4.15b5 或 b6 macOS ARM64** 组件；不支持其他构建或 Windows。

b6 需要 Willington `03b6efccd1c72ef816ae5be6f28ad33115ef3e9a`，或保留 b6 配置的后续捆绑包。旧版 Kumi 捆绑包需要更新。参见 [b6 上游验证](https://github.com/xonedsp/willington/blob/03b6efccd1c72ef816ae5be6f28ad33115ef3e9a/evidence/live-12.4.15b6-arm64/README.md)和 [Kumi CI 捆绑包验证](../evidence/willington-b6-import.json)。

在可丢弃的测试工程中开发时，请遵循上游的[构建与安装说明](https://github.com/xonedsp/willington/blob/main/integrations/WillingtonEditing/README.md)。安装提升后的包后，`api.install()` 会自动选择并验证与精确构建匹配的库。写入默认关闭，必须明确启用；播放必须停止。安装后，如果 full 策略允许 Python，就可以通过 `run_python`（在主机接口中为 `live_run_python`）调用这些方法。如果方法不存在或精确构建检查拒绝加载，则表示不可用，无论 Kumi 的 Willington 开关处于什么状态。

Python 调用**不会**创建 Kumi `HISTORY` 记录。原生方法各有自己的 Live 撤销边界：一个脚本内的多次调用可能产生多个撤销步骤。全局跟随动作开关不进入 Live 撤销记录。不要盲目调用 `song.undo()` 来补偿失败的脚本。

`run_python` 提供 `song`；传入当前对象的 `ref` 时，还会提供 `obj`。重连后应重新发现引用，不要沿用另一个工程中的对象索引。

## 创建分组轨道

`song.group_tracks(*tracks)` 返回新组，不依赖 UI 选中状态。应先解析 1–128 条不重复、连续、按 Song 顺序排列的顶层音频/MIDI 轨道。现有组、嵌套组、主轨/返回轨以及其他 Song 的轨道会被拒绝。

`song.ungroup_track(group)` 接受包含音频/MIDI 成员、没有设备和嵌套组的顶层组。它可用于立即恢复测试修改，但不是通用的历史逆操作：之后对路由、自动化、设备或成员关系的修改不能被丢弃。专用历史功能需要完整捕获这些状态，并在撤销前确认它们没有改变。

## 场景与全局跟随动作

`scene.get_follow_actions()` 返回 JSON；`scene.set_follow_action(field, value)` 设置 `enabled`、`action_a`、`action_b`、`chance_a`、`chance_b`、`jump_a`、`jump_b`、`time`、`linked` 或 `loop_count`。动作范围为 0–9，概率范围为 0–100，且相互联动、总和为 100。跳转目标为从 1 开始的场景编号（0 表示未设置），最大 8388608。时间以四分音符拍数计，最小 0.25；循环次数为 1–1073741823。关联计时使用最长的片段及循环次数。应保留完整状态并在写入后重新读取。

`song.get_follow_actions_enabled()` 读取全局开关；`song.set_follow_actions_enabled(True)` 或 `False` 修改它。保留原来的布尔值并明确恢复。此组件不提供场景/全局观察者 API。测试通过 UI 启动验证了调度，但该实验中的 `Scene.fire()` 未能重现 UI 调度行为。

## 逐音符 MPE

使用重新读取 MIDI 片段后得到的稳定音符 ID。`clip.get_note_expression(note_id, dimension)` 返回 JSON；dimension 为 `pitch`、`slide` 或 `pressure`。`clip.replace_note_expression(note_id, dimension, state_json)` 接受包含布尔值 `exists` 和 `events` 列表的状态。

事件为 `[time, value, x1, y1, x2, y2]`：时间是从音符起点算起的拍数，音高使用音分（±4800），slide/pressure 使用 MIDI 单位（0–127）。限制为 65536 个事件，时间 0–1576800，同一时刻最多两个事件，曲线系数 0–1。不存在的曲线通道与明确存在但为空的通道不同。恢复时应保留 `exists` 和所有曲线系数，并确认仍是同一片段和音符。

## Arrangement 自动化

解析所属轨道和连续值参数；量化参数以及属于其他轨道的参数会被拒绝。

- `track.get_arrangement_automation(parameter, start, end)` — 区间 JSON。
- `track.insert_arrangement_event(parameter, event_json)` — 插入六个数值组成的事件，时间是 Song 绝对拍数（0–1576800），数值使用参数公开单位。
- `track.delete_arrangement_events(parameter, start, end)` — 删除区间。
- `track.get_arrangement_snapshot(parameter)` — 完整的不透明快照 JSON。
- `track.restore_arrangement_snapshot(parameter, snapshot_json)` — 明确恢复。

修改前应保留完整快照。区间读取不包含隐藏的初始事件和区间外事件，不能作为完整撤销数据。快照保留原生原始值、曲线、同一时刻的事件以及包络不存在的状态。Live 可能在插入时规范化曲线控制柄，包括重置最后一个点的控制柄；应检查读取结果，不要假定提交的控制柄会原样保留。

快照带签名，并绑定原来的参数和适配器实例。重新安装适配器或重启 Live 会使其失效。它不是可持久保存的 Kumi 历史：不要修改内容，也不要在签名验证拒绝后改用 Live 撤销。存在尚未完成的自动化变换时会拒绝操作。UI 选择和事件对象身份不属于快照保证范围。

原始所有者被删除或从适配器的 128 个所有者 FIFO 缓存中被淘汰时，快照也会失效。指针复用不会保留所有权。

## 验证与发布边界

上游[测试工程验证记录](https://github.com/xonedsp/willington/blob/main/evidence/native-editing/b5/README.md)涵盖读取确认、原生撤销/重做、保存和重新打开、Max 调用、MPE 播放和音符通知、Arrangement 播放、分组路由以及 UI 场景调度。待完成变换的拒绝通过原生标志控制器测试，而不是实际的 UI 拖动。这是上游证据，不是 Kumi 工具的验收结果。

Kumi 仅通过 [Willington 更新流程](DEVELOPER_GUIDE.md#willington-的文件)导入 Willington 的文件。
