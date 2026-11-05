# Kumi 的 Logic Pro 适配器：M2

- 原始 Logic M2 读取器的应用端适配器。通过 `logic_read` 读取工程、轨道、片段、音符、插件、发送、标记和速度；FOCUS 提供 Channel Strip、Piano Roll、Tracks、Routing 四种视图。
- 需要 macOS 上的 Logic Pro 12。桥接程序连接已打开的 Logic，不向 Logic 安装组件，也不要求重启 Logic。程序、已构建的辅助程序和连接信息放在 `KUMI_HOME/bridges/logic/bridge/logic-kumi`。
- 请在系统设置 → 隐私与安全性 → 辅助功能中允许运行 Kumi 的应用。设置流程轮询已认证桥接程序的权限状态；对话框或其他桌面的状态与权限不足分别显示。
- 使用已连接 Logic 的 Apple 包标识打开已保存的工程。可读取工程、所选项目和录音状态。当前阶段不提供音乐编辑、撤销、听音、渲染、目标、脚本或变更追踪。资料库来源与学习属于 M6。
- 发布文件包括 `logic-kumi`、`kumi-logic-hands-*` 和 `version`。源码检出需预先构建原生桥接程序及辅助程序。`KUMI_LOGIC_BRIDGE_SOURCE` 可指定现有发布格式文件夹；设置流程不构建原生代码。
- 隔离测试使用本地夹具与工程打开操作的替身，不启动真实 DAW，也不要求辅助功能授权。原生录制回放与实际 Logic／应用验收属于独立检查。
