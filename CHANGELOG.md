# Changelog

## 0.2.0-alpha.1 — 2026-09-06

首次 Rust 预览版：替换 Python/Qt 界面与 C/libnfc 读写桥，使用 Rust 直接管理串口、卡片数据、NDEF、后台任务和 Mac 打包。兼容旧版 JSON 文档。实机读取结果与旧版一致，为 56/64 块。

已知差异：尚未迁移旧版 C 后端提供的 nested/hardnested/DarkSide、Gen1a UID 写入。Windows 专有功能仍有缺口，见迁移状态与 Issues。

此前 Python 0.1 原型仅保留在本机，不作为 Rust 仓库或 Release 的组成部分。
