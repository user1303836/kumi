# Willington

[English](../en/WILLINGTON_INTEGRATION.md) · 简体中文 · [日本語](../ja/WILLINGTON_INTEGRATION.md)

Willington 是一组原生提供程序（provider），能触及 Live 的 Python API 触及不到的部分：Session 片段的跟随动作（Follow Actions）、机架宏的映射与名称，以及机架链的区域。从带有 Willington 运行时文件的版本起，Kumi 的桥接会把它们放在自己的 Remote Script 里；在此之前，请[自己安装 Willington](#自己安装-willington)。无论哪种方式，在你用 `/willington` 开启之前它们一直是关闭的。每个提供程序都会为所连接的那个确切的 Live 构建版本选择绑定；在其他构建版本上，这些工具只是不会出现，Kumi 的其余部分照常工作。

## 它增加了什么

| Kumi 工具 | 桥接工具 | 编辑类型 | 提供程序 |
| --- | --- | --- | --- |
| `set_clip_follow_actions` | `live_follow_actions_preview/apply` | Session 片段的全部十个跟随动作字段 | WillingtonBindings |
| `edit_rack_mapping` | `live_willington_device_preview/apply` | `macro-name`、`variation-name`、`macro-mapping` | WillingtonDeviceTools |
| `map_modulator` | `live_run_python`（有 `live_willington_device_preview/apply` 时提供） | 把 Live 的 LFO、Shaper、Envelope Follower（各 8 个目标）和 Expression Control（5 个）映射到参数（Willington 的 `map_modulation`） | WillingtonDeviceTools |
| `edit_rack_mapping` | `live_willington_device_preview/apply` | `selector-zone`、`key-zone`、`velocity-zone` | WillingtonRackZones |

桥接只提供那些提供程序已加载且启用了写入的编辑类型：`live_willington_device_preview` 只列出这些 `kind` 值。每次编辑都需要走带处于停止状态。

有些机架方面的改进不需要 Willington：按 Live 自身的宏布局读取机架、按索引调用或删除变体，以及当 Live 的 Browser 没有 Modulators 类别时回退到自带的调制器设备。它们适用于任何桥接。

## 开启与关闭

在 Kumi 中输入 `/willington`。它会在 `Remote Scripts/AbletonMcpBridge` 中、桥接的 `__init__.py` 旁边写入 `willington.json`，开启 DeviceTools 和 RackZones 及其编辑；桥接会在一秒内加载它们，Live 照常运行。有通过的[自检](#跟随动作自检)时，跟随动作也会开启：它的编辑需要自检，而 Live 无法卸载它的绑定。再次输入 `/willington` 会删除这个文件：桥接会卸载 DeviceTools 和 RackZones，并关闭跟随动作写入；跟随动作的绑定会保留到 Live 重启，Kumi 会说明这一点。绑定处于关闭状态时，Kumi 会在启动时告诉你。

Kumi 也会告诉它的模型。绑定关闭时，遇到需要这些编辑的请求，会附上一句：`/willington` 可以开启它们；绑定开启时，Kumi 会自己映射宏，并在加载调制器的同一个计划中用 `map_modulator` 映射 Live 的调制器，而不是请你在 Live 中映射。

覆盖哪些 Live 构建版本，取决于你所用的 Willington 版本；对 Kumi 自带的副本，桥接文件夹里的 `willington/release.json` 写明了这个版本。Willington 经过验证的绑定：macOS ARM64 上 Live 12.4.15b4、b5 和 b6 的跟随动作和 DeviceTools，macOS ARM64 上 b5/b6 的 RackZones，以及 Windows x64 上 Live 12.4.15b5 的全部三者。不支持 Intel macOS。每个提供程序会根据正在运行的 Live 进程的操作系统、架构、版本和可执行文件哈希选择绑定，原生库还会检查正在运行的可执行文件本身（macOS 上是 Mach-O UUID，Windows 上是 CodeView GUID）。

跟随动作编辑还需要所选的库通过一次自检：WillingtonBindings 文件夹中要有 `self-test.json`，其中 `"status": "passed"`，并且 `library_sha256` 等于该库的 SHA-256。没有它，只有跟随动作编辑保持关闭；宏、名称和区域编辑仍然可用。

缺少经过验证的 profile 时，只会跳过那一个组件：同一份配置在 b4 上可以使用跟随动作和 DeviceTools，在 b5 上还可以使用 RackZones。这类因缺少 profile 而产生的带类型的拒绝，会在每个 Live 进程中按组件缓存，每个组件只记录一次日志。

配置格式错误、缺少产物、完整性错误、意外的启动失败，或者已有其他活动的所有者时，原生扩展不可用；普通的桥接保持运行。Live 的日志（Log.txt）会报告原因，并列出实际处于活动状态的提供程序，以及已启用写入的提供程序。跟随动作自检缺失或过时时，只会关闭跟随动作写入。

Remote Script 停止时，会关闭跟随动作写入，并卸载 DeviceTools 和 RackZones。跟随动作的绑定无法卸载：它们在该 Live 进程中保持注册状态，桥接再次启动时，或在 `/willington` 之后重新加载提供程序时，会在写入关闭的状态下重新使用它们。

### willington.json

`/willington` 会写入这个文件；你也可以自己写。它必须是普通文件、仅所有者可访问、最多 4 KiB，并包含以下这些键（`rackZones` 和 `editing` 可选）。`/willington` 写入的内容（有通过的自检时 `followActions` 为 true）：

```json
{"version": 1, "followActions": false, "deviceTools": true, "rackZones": true, "enableWrites": true}
```

| 键 | 含义 |
| --- | --- |
| `version` | 始终为 `1` |
| `followActions` | 加载 WillingtonBindings |
| `deviceTools` | 加载 WillingtonDeviceTools |
| `rackZones` | 可选；加载 WillingtonRackZones |
| `editing` | 可选；加载 WillingtonEditing。仅当通过匹配实际库的自测时，`/willington` 才添加 `"editing": true`。 |
| `enableWrites` | 允许编辑；为 `false` 时加载提供程序，但不提供编辑 |

这个文件变化后，桥接会在一秒内重新读取它。Kumi 更新会保留它，它也不会影响桥接的安装检查。没有它，桥接就是普通的桥接。

## 自己安装 Willington

在版本带上 Willington 之前，或要使用尚未带上的 Willington 构建版本（例如为新的 Live 版本）时，请把 Willington 的多版本包安装到 Live 的 Remote Scripts 文件夹中，与 AbletonMcpBridge 并列：必需的 `WillingtonRuntime`，以及你需要的提供程序 `WillingtonBindings`（跟随动作）、`WillingtonDeviceTools`（宏和变体；它必须提供 `get_macro_mapping` 和 `get_selected_variation_name`）和 `WillingtonRackZones`（区域）。手动复制时，必须保留运行时、`build/<profile-id>/` 目录和清单文件。安装在那里的提供程序优先于 Kumi 自己的副本。在 Live 中关闭任何独立的 Willington 控制界面，然后重启 Live：桥接不会与其他所有者共享这些提供程序。然后用 `/willington` 开启它们。

## 跟随动作自检

切换 Live 构建版本或替换所选的跟随动作库之后，请重新进行自检。这项测试把 WillingtonBindings 作为独立的控制界面运行，所以需要一份[安装在桥接旁边的](#自己安装-willington)副本：Kumi 自己的副本在桥接内部，不会出现在 Live 的列表中。此流程适用于分发的包，不需要源代码检出或 `manage.py`。独立测试会在当前工程中创建一条测试夹具轨道，并执行原生写入和 Live 的撤销，所以请使用一个一次性工程。

1. 在 Live 的控制界面设置中停用 `AbletonMcpBridge` 和独立的 Willington 控制界面，然后退出 Live。原生的跟随动作属性会一直保持注册，直到进程退出。
2. 启动目标 Live 构建版本，在播放停止的状态下打开一个一次性工程，并只选择 `WillingtonBindings` 作为 Willington 控制界面，MIDI 输入/输出设为 None。它安装后的 `status.json` 应显示 `"status": "registered"`。
3. 用下面的命令把测试加入队列，把文件夹参数替换为已安装的 Bindings 目录。如果已有待处理的命令，它会拒绝执行；它还会删除旧的回执，以免与本次运行混淆。

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

4. 等待新的 `self-test.json` 完成，并包含 `"status": "passed"` 和 `library_sha256`。正在运行或失败的报告不会启用写入。如果命令失败，请查看 `command-error.json`。哈希必须与所选的 `build/<profile-id>/libwillington.dylib`（旧版安装则为根目录下的库）一致；`shasum -a 256 '/full/path/to/libwillington.dylib'` 会打印出这个摘要。把回执保留在已安装的 Bindings 文件夹中。如果桥接旁边没有安装任何副本、使用的是 Kumi 自带的副本，请把同一个库的回执放在 `AbletonMcpBridge/willington/WillingtonBindings/` 中：Kumi 更新时会保留它，它也不影响桥接的安装检查。
5. 把独立的 `WillingtonBindings` 控制界面设为 None，退出 Live，并在再次启用 `AbletonMcpBridge` 之前重启 Live。丢弃这个一次性工程。不要在使用 Kumi 的同时选择独立的 Willington 控制界面：两者都会试图拥有原生绑定。在用 `enableWrites: true` 启用跟随动作写入之前，Kumi 会针对它所选的库重新检查回执。

## 跟随动作

`live_follow_actions_preview/apply` 设置一个 Session 片段的全部十个字段：启用、链接、动作 A 和 B、概率 A 和 B、循环次数、时间，以及跳转目标 A 和 B。

- 动作用数字表示：0 无、1 停止、2 再次、3 上一个、4 下一个、5 第一个、6 最后一个、7 任意、8 其他、9 跳转。跳转目标是从 1 开始的场景编号。
- 两个概率之和为 100；给出其中一个，另一个就设为剩余部分。
- 链接时，计时使用循环次数；未链接时，计时使用以拍为单位的 `time`。
- 走带必须处于停止状态，片段也不能在录音。
- 它不会更改场景的跟随动作或 Live 的全局跟随动作开关。要设置启动 Legato，请使用片段设置工具（Kumi 中的 `set_clip`）。

预览会捕获全部十个字段。如果写入中途失败，之前已写入的字段会恢复原值。撤销会恢复捕获的字段，如果片段此后发生了变化，撤销会被拒绝。

## 宏、变体与映射

用 `ref` 指向一个机架，调用 `live_willington_device_preview/apply`：

| 类型 | 参数 | 说明 |
| --- | --- | --- |
| `macro-name` | `macroIndex`（0–15）、`name` | 重命名一个宏 |
| `variation-name` | `name` | 重命名所选变体；必须选中一个已命名的变体 |
| `macro-mapping` | `targetRef`、`mappingIndex`（0–15，或用 `null` 取消映射）、`minimum`、`maximum`、`mappingKind` | 把该机架内的一个参数映射到一个宏 |

映射类型：

- `continuous` 和 `enum`：`minimum` 和 `maximum` 使用参数自身的单位，并处于其范围内；范围可以反转。`enum` 的端点是整数。
- `boolean`：宏阈值为 0 到 127 的整数，`minimum` ≤ `maximum`。

`targetRef` 必须来自最新一次发现，并且位于该机架、它的嵌套设备或它各条链的调音台中。事务会捕获名称，或者捕获映射、参数的值和各宏的值，连同所涉及的身份。Live 会在一个 tick 之后才设置被映射参数的值，所以映射以映射本身和宏的值为栅栏，而不以该参数的值为栅栏。每次写入都会被回读，如果不一致，就精确恢复原样。撤销会恢复捕获的状态，如果机架此后发生了变化，撤销会被拒绝。这是事务自己的撤销，而不是 Live 的撤销。

## 机架链区域

用 `ref` 指向一个机架、`targetRef` 指向它的一条常规链，调用 `live_willington_device_preview/apply`：

| 机架 | 区域 |
| --- | --- |
| Audio Effect Rack | `selector-zone` |
| Instrument Rack、MIDI Effect Rack | `selector-zone`、`key-zone`、`velocity-zone` |

Drum Rack 和返回链会被拒绝。一个区域有四个整数端点：`minimum`、`fadeMinimum`、`fadeMaximum` 和 `maximum`，它们必须保持顺序（`minimum` ≤ `fadeMinimum` ≤ `fadeMaximum` ≤ `maximum`），并处于 0–127 范围内（力度为 1–127）。省略的端点保持当前值，所以移动一个范围时，可能需要同时给出两个淡变端点。

预览会捕获全部四个端点。应用和撤销按身份以机架和链、以及整个区域状态为栅栏；写入后回读与要求不符时，会精确恢复原样。

## 证据

| 提供程序 | 运行 | Live | 桥接 | 覆盖范围 |
| --- | --- | --- | --- | --- |
| 跟随动作 | [kumi-clip-follow-actions-b5.json](../evidence/kumi-clip-follow-actions-b5.json)，2026-09-30 | 12.4.15b5，macOS arm64 | 1.0.53 | 在已保存的测试工程上进行的 Kumi 修改和撤销，走带已停止 |
| 跟随动作、宏、映射 | [willington-kumi-chat.json](../evidence/willington-kumi-chat.json)，2026-09-30 | 12.4.15b4 ARM64 | 1.0.52 | 一次真实的 Kumi 对话：跟随动作、宏重命名、映射，每项都已撤销 |
| 机架区域 | [rack-zones-b5.json](../evidence/rack-zones-b5.json)，2026-10-01；完成验证 2026-10-02 | 12.4.15b5（2026-09-24 构建），arm64 | 1.0.66（Kumi 事务） | 回读、写入、撤销和重做，保存后重新打开，Kumi 的撤销；完成验证增加了信号门控、淡变以及实际的 Max 调用 |

变体重命名以及反转的 continuous 和 enum 映射，是直接通过桥接测试的。跟随动作的调度，以及跟随动作和宏编辑在保存并重新打开工程后是否保留，都没有测试过。

机架区域的完成验证结果和回执摘要见[公开的验证汇总](../evidence/rack-zones-b5.json)：42 项信号门控检查、49 次淡变测量及由此得出的 14 项方向比较，以及 7 次实际的 Max `live.object` 写入/读取/恢复循环。正式采用的 `live-12.4.15b5-arm64` 库与测试过的候选库逐字节相同。测量使用的是归一化的 Live 电平表；不声称精确的线性增益、淡变端点处的静音、按住音符时的编辑、多条链重叠的交叉淡变，也不声称适用于其他构建版本或平台。机架区域在 b4 上仍不受支持。完成验证的原始回执和测试脚本保存在私有的 Willington 仓库中、汇总所记录的那个不可变提交里；这些原始文件不在此处公开。

桥接的自动化测试在没有 Live 的情况下覆盖其余部分：缺失的提供程序、格式错误的配置、过时和冲突的编辑、部分写入、所有权与重连，以及丢失的回复。

## 有意不提供的功能

Willington 有用于以下操作的原生方法，但在能够安全撤销之前，Kumi 不会提供它们：

- **覆盖一个变体**：需要读取并恢复完整的已存储宏值和启用掩码，而不仅仅是变体的名称。
- **直接替换 Drum Sampler 的采样**：需要当前采样的身份和路径，并恢复替换所改变的内容。通过 Browser 加载预设和采样是可以的。
- **作为桥接事务的调制器映射**：原生修改要稍后才会稳定下来，与 Live 的线程不同步。它首先需要稳定状态检查、对源和目标的确切所有权、取消以及恢复。在此之前，`map_modulator` 通过 Kumi 自己的 Python 映射，并留下 HISTORY 记录：只要槽位里仍是它映射的那个参数，Kumi 的撤销就会清空它填上的槽位（`map_modulation(slot, None)`）；替换了另一个映射的映射会被保留（kept），因为 Kumi 无法把旧的映射恢复回去。Live 的撤销对它是否有效尚未验证。

存在原生方法，并不足以构成一个可撤销的操作：不要只添加一个运行时描述符或一个协议条目就提供它。

## 原生编辑及其自测

安装并验证适用于 **Live 12.4.15b5 或 b6 macOS ARM64** 的 `WillingtonEditing` 后，Kumi 提供分组创建、编曲自动化、场景/全局 Follow Actions 和逐音符 MPE。其他构建、Intel macOS 和 Windows 不支持此组件。仅当仅所有者可访问的 `self-test.json` 与已安装库匹配时，`/willington` 才请求编辑；提供器还会核对 Live 实际选择的库。凭据缺失或过期时编辑保持关闭。更新会保留凭据，但库发生变化后需要重新测试。

Live 12.4.15b6 ARM64 需要 Willington 提交 `03b6efccd1c72ef816ae5be6f28ad33115ef3e9a`，或保留该配置的后续捆绑包。仍携带旧包的 Kumi 版本需要更新捆绑包才能支持 b6；请检查 `willington/WillingtonRuntime/matrix.json` 中是否有 `live-12.4.15b6-arm64` 条目，且包含 `status: "validated"` 和所需组件。`willington/release.json` 标明捆绑包的提交。[CI 捆绑包验证记录](../evidence/willington-b6-import.json)涵盖自动加载、禁用写入时的拒绝以及 Kumi 原生编辑测试。本地自测凭据仍须匹配所选库的哈希。

`group_tracks`, `set_scene_follow_actions`, `set_global_follow_actions`, `set_note_expression`, `edit_arrangement_automation` → `live_native_editing_preview/apply`.

准备名为 `Willington Native Editing` 的可丢弃工程并停止播放。前两条轨道不能分组；轨道 1 / 插槽 0 中应有包含音符 ID 1 的 MIDI 片段，并且场景 0 存在。测试会修改并恢复工程。用下面仅所有者可访问的 `willington.json` 加载提供器，然后通过明确授权的 `run_python` 会话在 Live 主线程运行 Python。将路径替换为本地 Kumi 源码路径。脚本验证全部五类编辑，只有所有检查通过后才在加载的包旁写入凭据，并在结束时关闭编辑写入。失败的运行使旧凭据失效。这是明确执行的操作流程，不是启动时的自动行为。

```json
{"version":1,"followActions":false,"deviceTools":false,"editing":true,"enableWrites":false}
```

已安装的桥接包不包含此脚本。请克隆 [Kumi](https://github.com/user1303836/kumi)，检出与已安装 Kumi 版本一致的发布标签，并使用该源码中的 `remote-script/native_editing_self_test.py`。将下面的 `/absolute/kumi` 替换为源码目录的绝对路径。开发期间应使用对应的功能分支，而不是发布标签。

```python
import importlib.util, sys, Live
spec = importlib.util.spec_from_file_location(
    "editing_fixture", "/absolute/kumi/remote-script/native_editing_self_test.py")
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)
provider = Live._kumi_willington_owner
fixture.run(provider, sys.modules[type(provider.mapper).__module__], song)
```

随后用 `/willington` 请求写入，它会再次验证凭据。四类编辑可在当前连接中通过 HISTORY 检查状态后恢复；分组没有 HISTORY 逆操作，并会使移动位置的引用失效。分组后请重新发现轨道。新 MPE 通道及待编辑的既有通道都限制为 4096 个事件。编曲插入仅支持线性控制柄。底层限制见[原生 API 指南](WILLINGTON_NATIVE_EDITING.md)。
