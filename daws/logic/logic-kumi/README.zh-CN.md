# Logic Pro 桥接器：M3 与 Loop Range

- 采样率是可选信息：后台仅限量读取 `Alternatives/000/MetaData.plist` 中的 XML。二进制、无法访问、符号链接或不确定的元数据均保持未知。

- 面向 macOS 上 Logic Pro 12 的实验性原生读取基础。经过身份验证的回环连接保留六个观察方法，新增 `change_preview`、`change_apply`、`change_restore`、`transport_stop`、`transport_play`、`cursor_set`、`undo_step_begin`、`undo_step_end`，以及由值恢复证实的 `undone_in_daw` 事件。原生读取等待时，缓存的状态和 FOCUS 仍能响应。
- 可读取 project、track、region、note、plugin、send、marker、tempo 行，支持 `tree`、`notes`、`timeline`、`routing` 四种正文。M3 支持五种轨道控制、轨道重命名/添加/删除、标记添加/移动/删除、速度、音符添加/更改/删除和插件旁通。写入前检查预览时的值，写入后读取结果。不公开试听或渲染。实验性应用集成可通过 `kumi --daw logic` 和 `/daw logic` 选择；录制数据回放与真实 Logic 验收仍需分别进行。
- `kumi-logic-hands` 是独立的子进程。缺少辅助功能权限时，状态会单独返回 `permission: "accessibility"`，而非将其视为 `busy`。首次连接时，macOS 可能询问是否授予助手权限。无需向 Logic 内安装任何内容，也无需重启 Logic。

在仓库根目录运行以下命令，每次只执行一个构建：

```sh
python3 scripts/build-logic-hands.py
cargo build -p logic-kumi
sh scripts/test-isolated.sh --profile ci -p logic-kumi
```

构建助手需要 macOS 和 Xcode 命令行工具。构建会在 `target/logic-hands/` 下生成名称包含源码摘要的通用助手。隔离的 Rust 测试使用临时助手，不需要 Logic 或辅助功能授权。

先打开 Logic，再直接运行开发版本：

```sh
target/debug/logic-kumi --kumi-home /absolute/path/to/disposable-kumi
cargo run -p logic-kumi --example logic_lab_client -- --kumi-home /absolute/path/to/disposable-kumi
```

- 桥接器会自动查找已构建的助手，也可用 `--helper <path>` 指定。直接运行时，Logic 退出，或桥接器收到 SIGINT、SIGTERM、SIGHUP 后才会结束；标准输入 EOF 不会使其结束。
- 主机会添加 `--hosted`，此时关闭标准输入管道也会使桥接器结束。两种退出路径都会删除发现文件，并停止、回收助手，即使助手请求仍在等待响应。桥接器不会启动或退出 Logic。
- 常规观察不会打开虚拟 MIDI 端口。只有显式添加 `--mcu` 才启用虚拟 Mackie Control 控制台；Logic 可能保存其控制台配置。停止助手会关闭端口，但不会删除已保存的配置。尚未证实能够自动恢复原配置。
- 可选的原生助手检查，不需要 Logic：`python3 -m unittest discover -s daws/logic/logic-kumi/helper/tests -p test_protocol.py`。此检查会编译、运行 macOS 助手，并使用临时虚拟 MIDI 端口。

原生录制回放使用 `d821e669` 中原始的四个 M2 场景和26请求向量。`tests/fixtures/sources.json` 记录来源路径及 SHA-256；测试确认原始字节不变，并比较实际读取结果，不重写期望值。录制回放不等于当前 Logic 或应用的原生验收。读取操作可能通过助手切换 Logic 的面板，但不会选择轨道或片段。读取音符要求目标片段是唯一选中的片段；其他选择状态会被拒绝，不改变制作人的选择。

- 值恢复通过重新写入完成。新增轨道需要在 Logic 中手动撤销：轨道头和 Mixer 快照无法排除远离播放头的自动化、关闭窗口的插件参数或隐藏元数据。已删除的轨道和标记也需手动撤销；Edit 菜单标题不能证明某一步属于 Kumi，原标记名称也无法设置。仅靠标题识别标记、音符或结构撤销不受支持；属性、轨道名和速度需以观察到的值恢复为证。
- 新增或移动的标记仅在完整捕获所需历史身份、且标记与整个列表仍匹配时才能自动恢复。未显示的 Text 为未知，不能当作空文本。历史身份不完整时，即使设置 `discard: true` 或当前位置看似已恢复，也必须手动撤销；之后显示 Text 无法补回过去的值。Text 完整可见的派生录制保留正向值恢复验证。
- 新增的20份历史录制逐字节复制自 `8d609d6a`，来源和哈希见 `tests/fixtures/change-sources.json`。14份重放全部请求，并明确处理身份/指导信息扩展及不安全结构归因的移除；6份停止在明确拒绝的恢复边界，保留依赖该操作的后续请求。未消费请求数为：`change-track-add` 0、`change-track-delete` 4、`change-marker-add` 1、`change-marker-move` 0、`change-marker-delete` 3、`change-carry` 4。拒绝的恢复及未消费请求不算成功重放覆盖，也不代表当前真实 Logic 验收。
- 音符编辑拒绝其他片段的选择，并在 Event List 开启期间核对目标仍为唯一选择。观察不会写入选择状态。引用对应不明确时拒绝恢复；缺失或空白的屏幕坐标不会触发点击。

- 插件旁通的校验包含所属轨道、槽位编号、完整有序插件链、各名称与乐器/效果器类型，以及可获得的稳定 AX role/subrole/id。应用和恢复前会重新读取；缺失、重名或不完整时拒绝，即使值已恢复或设置了 `discard: true`。身份中不包含旁通值和易变 UI 状态。原始录制保持不变，重放仅从录制的 Mixer 信息明确推导新增的身份字段。
- Logic 槽位不提供持久插件实例身份或组件类。以完全相同的可观察身份删除后重建插件的 ABA 变化，以及最后读取与 AX 操作之间的变化，无法由这些检查区分。这仍是真实环境验收的限制；本次没有执行真实 Logic 修改测试。Undo 记账只观察标题，不发送合成快捷键。


- 此处从 M4 新增的控制器仅有 `loop-range`。要求事先显示 Custom LCD、两个完整且可确认写入能力的音乐定位器、`/16` 细分，以及已选中的完整 Signature List；拍号须为 `1.1.1.1` 处唯一已知的 N/4（N 为1至16），并停止播放和录音。拍号或细分未知、多重或不支持时，在写入项目前拒绝。Kumi 不会为满足此限制而修改拍号事件。
- 输入为从音乐小节1起算的 `startBeats`、`endBeats`，必须准确落在1/960拍刻度上。`0, 0` 仅关闭 Cycle，保留两个定位器。日志保存原始位置、重新读取的已保存项目身份、完整受支持拍号映射及细分；应用和值恢复均核对这些事实，已恢复判断和 discard 也不能跳过。历史日志信息不完整时要求手动恢复。
- 写入前检查全部控件可写性和64步上限，每次发送前后重新核对完整状态。回复丢失、不合法，或项目、时间基准、无关定位器状态变化时，以不确定结果立即停止；不重试、不回滚、不发送显示清理操作。控制器不打开面板、不更改 LCD 模式、不发送按键、不使用 MIDI，也不调用原生 Undo。最后读取至发送之间的竞争，以及可观察身份完全相同的项目重新打开，仍是原生接口限制。
- `tests/fixtures/cycle-source.json` 记录 `6f8f6e9b` 的 Loop Range 原始场景来源，原有7个步骤的字节保持不变。只读来源前缀表明 LCD 不是 Custom，因此首个预览拒绝；依赖它的后续6个步骤明确保留、未消费。原录制也缺少完整 Signature List。另用明确已知状态的合成测试验证控制器，不将这些测试或拒绝前缀称为当前真实 Logic 验收或全部7步回放。
