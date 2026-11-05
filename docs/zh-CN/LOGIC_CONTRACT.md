# Logic Pro 桥接协议（M2、M3）

M2 的六个读取方法为 `ping`、`status`、`focus`、`read`、`snapshot`、`focus_body`。M3 添加 `change_preview`、`change_apply`、`change_restore`、`transport_play`、`transport_stop`、`cursor_set`、`undo_step_begin`、`undo_step_end`。

支持16种修改：轨道音量、声像、静音、独奏、待命、名称、添加、删除，标记添加、移动、删除，速度，音符添加、修改、删除，插件旁通，以及播放、停止、播放头定位。Logic 没有整批撤销块，音量、声像、静音、独奏、待命、速度和旁通不进入 Logic 自身撤销历史。Kumi 撤销以新的写入恢复值；若之后有人改动同一值，会拒绝恢复。已删除轨道和标记需要手动使用 Logic 自身撤销。聆听、目标、脚本、通用 UI 操作和离线变化追踪仍未启用。

音符读取要求只选中目标区域，不改变选择。未授权时 `status.permission` 为 `accessibility`，正常为 `null`；`busy` 为 `false` 或 `modal`、`away`、`loading`。采样率只从普通 XML 文件 `Alternatives/000/MetaData.plist` 异步读取，上限 64 KiB；符号链接、二进制格式或不确定值返回 `null`。正常启动不创建 MIDI 端口；显式使用 `--mcu` 可能在 Logic 中留下控制表面配置。

完整表和 JSON 字段见[英文协议](../en/LOGIC_CONTRACT.md)。必须先打开临时工程，并使用 `--changes --scratch-confirmed`，才能启用 `accept_logic` 的自动修改检查；撤销失败可能留下修改。历史 M2、M3 验收记录不代表此次移植的新实机结果。

Kumi 不自动恢复已删除轨道和标记。撤销标题不能唯一识别 Kumi 的步骤，需要手动使用 Logic 自身撤销。已确认删除在 HISTORY 中保留手动撤销说明，`/undo` 不发送 RPC；未确认的删除仍保持不确定。不会仅凭标题推断标记、音符或结构修改已由原生撤销恢复。
