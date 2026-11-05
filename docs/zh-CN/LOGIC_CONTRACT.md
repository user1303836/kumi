# Logic Pro 桥接协议（M2、M3 与 Loop Range、区域值预览）

M2 的六个读取方法为 `ping`、`status`、`focus`、`read`、`snapshot`、`focus_body`。M3 添加 `change_preview`、`change_apply`、`change_restore`、`transport_play`、`transport_stop`、`cursor_set`、`undo_step_begin`、`undo_step_end`。

支持16种修改：轨道音量、声像、静音、独奏、待命、名称、添加、删除，标记添加、移动、删除，速度，音符添加、修改、删除，插件旁通，以及播放、停止、播放头定位。Logic 没有整批撤销块，音量、声像、静音、独奏、待命、速度和旁通不进入 Logic 自身撤销历史。Kumi 撤销以新的写入恢复值；若之后有人改动同一值，会拒绝恢复。新增和已删除轨道、已删除标记，以及原始身份不完整的标记修改，需要在 Logic 撤销历史中手动撤销。聆听、目标、脚本、通用 UI 操作和离线变化追踪仍未启用。

音符读取要求只选中目标区域，不改变选择。未授权时 `status.permission` 为 `accessibility`，正常为 `null`；`busy` 为 `false` 或 `modal`、`away`、`loading`。采样率只从普通 XML 文件 `Alternatives/000/MetaData.plist` 异步读取，上限 64 KiB；符号链接、二进制格式或不确定值返回 `null`。正常启动不创建 MIDI 端口；显式使用 `--mcu` 可能在 Logic 中留下控制表面配置。

完整表和 JSON 字段见[英文协议](../en/LOGIC_CONTRACT.md)。必须先打开临时工程，并使用 `--changes --scratch-confirmed`，才能启用 `accept_logic` 的自动修改检查；新增的测试轨道会保留，供操作者在 Logic 撤销历史中手动撤销；其他恢复失败也可能留下修改。历史 M2、M3 验收记录不代表此次移植的新实机结果。

Kumi 不会自动移除新增轨道，也不会自动恢复已删除轨道或标记。Logic 的界面无法完整显示自动化、关闭的插件状态或隐藏元数据，撤销标题也无法唯一识别某一步。请在 Logic 自身的撤销历史中查看相应步骤并手动撤销。添加或移动标记时，若原始 Text 列等必要字段未显示，也需要手动撤销；之后显示字段无法恢复其原值。只有完整捕获原始行与列表身份时才允许按值恢复，discard 不能绕过未知内容。已确认的手动修改在 HISTORY 中保留为 Kept，默认 /undo 会说明限制且不发送 RPC，不会跳到更早修改。不确定的写入仍保持不确定。

M4 的首个预览只在 M3 的16种修改之外添加 loop_range。startBeats 和 endBeats 使用四分音符单位，必须精确对齐每四分音符960刻度后才能启用循环，不会自动取整；必须事先显示 Custom LCD 与完整 Signature List，使用单一 N/4 拍号（N 为1至16）及 /16 细分，并停止播放与录音；0,0 会关闭循环并保留定位点。写入前必须完整读取当前单一拍号与时间基准，围栏同时绑定循环开关、两个定位点和拍号身份。未知或多个拍号、制作者更改循环值、时间基准变化都会拒绝写入或恢复，discard 不能绕过检查。循环不进入 Logic 自身撤销历史；Kumi 仅按值恢复完整确认的状态，缺少原始证据时在 HISTORY 中保留手动操作说明，不会跳到更早修改。其他 M4 修改及工程保存仍未启用。这只移植历史提交 6f8f6e9bbc3666159ce68be1ce7466aa30e82670 的 Loop Range，不代表新的实机验收。

另外只添加 Region Rename（region_rename {ref,name}）和 Region Loop（region_loop {ref,loop}），共19种修改。必须事先只选中目标区域并显示 Inspector 与 Mixer。Kumi 完整检查工程、轨道、区域和 Inspector 事实后，只写入名称或 Loop 复选框；不会改变选择、发送按键、操作 Logic 原生撤销或切换面板。未知、重复、替换或已被制作者更改的事实都会拒绝写入或恢复，discard 不能绕过检查。重命名后须重新读取引用；普通区域读取的 looped 不能证明 Inspector Loop 的值。完整原始证据只保留在主机日志中，公开响应仅含名称、Loop 值和严格验证的恢复能力。证据缺失的已确认修改保留为 Kept 并提供手动操作说明，默认撤销不会跳到更早修改；不确定写入不会重发。界面观测完全相同的替换仍是身份限制。这不代表其他 M4 修改、工程保存或新的实机验收。
