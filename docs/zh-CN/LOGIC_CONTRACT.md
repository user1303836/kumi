# Logic Pro 桥接协议（M2）

在 macOS 上使用 Logic Pro 12 或更新版本，通过 `kumi --daw logic` 或 `/daw logic` 显式选择。实验性集成不会自动选中。`kumi --daw logic bridge` 将桥接安装到 Kumi 自己的目录，保持已打开的 Logic 运行，无须重启。设置提示时，在系统设置中授予助手辅助功能权限；仅桥接连接成功还不能完成这一步。

Kumi 可读取工程、轨道、区域、所选区域的音符、插件、发送、标记和速度。FOCUS 显示通道条、音符、时间线和路由。它可报告录音状态、记住已保存工程的路径，并打开之前观察过的已保存工程。修改、撤销、聆听、目标、脚本、离线变化追踪和素材库发现尚未启用。读取音符要求只选中目标区域；Kumi 不会改变选择。正常启动不创建 MIDI 端口。

详见[协议与限制](LOGIC_CONTRACT.md)。录制数据的回放测试不能代替当前 Logic 版本的实机验收。开发者可在临时工程上显式运行 `accept_logic` 示例，生成新的验收报告。

六个线协议方法为 `ping`、`status`、`focus`、`read`、`snapshot`、`focus_body`。未授权时 `status.permission` 为 `accessibility`，正常为 `null`；`busy` 为 `false` 或 `modal`、`away`、`loading`。采样率只从普通 XML 文件 `Alternatives/000/MetaData.plist` 异步读取，上限 64 KiB；符号链接、二进制格式或不确定值返回 `null`。显式使用 `--mcu` 可能在 Logic 中留下控制表面配置。完整 JSON 结构和助手操作见[英文协议](../en/LOGIC_CONTRACT.md)。2026-10-18 的实验记录属于历史证据，不代表此次移植的新实机测量。
