# Logic Pro 桥接器：M2

- 采样率是可选信息：后台仅限量读取 `Alternatives/000/MetaData.plist` 中的 XML。二进制、无法访问、符号链接或不确定的元数据均保持未知。

- 面向 macOS 上 Logic Pro 12 的实验性原生读取基础。经过身份验证的本地回环连接仅公开六个方法：`ping`、`status`、`focus`、`read`、`snapshot`、`focus_body`，以及项目、走带、轨道和选择事件。原生读取等待时，缓存的状态和 FOCUS 仍能响应。
- 可读取 project、track、region、note、plugin、send、marker、tempo 行，支持 `tree`、`notes`、`timeline`、`routing` 四种正文。不公开乐曲编辑、试听或渲染。应用中尚未注册 Logic 提供程序或 DAW 选择项；主机功能开放以及原生与应用验收仍是独立工作。
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
