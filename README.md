# PCR532 Studio · Rust

用于自有或获授权测试卡的 macOS 桌面工具。应用界面、PN532 串口通信、卡片数据处理、NDEF 和打包工具均用 Rust 实现；不调用 Python、Qt、libnfc 或旧的 C 恢复程序。

**当前版本：0.2.0-alpha.1。Rust 基础迁移已可运行，尚未完成旧版所有功能的迁移，更不是 Windows 原版的完整复刻。** 特别是 nested、hardnested、DarkSide 的纯 Rust 恢复算法尚未实现，不能把字典认证当作未知密钥恢复。完整差异见 [迁移状态](docs/FEATURES.txt)。

## 已验证

2026-09-06，在开发 Mac 和已连接的 PCR532 上，Rust 直连成功识别 PN532 1.6 与 Classic 1K 兼容测试卡，读取 **56/64 块**，与旧版一致；第 13、14 扇区仍为未知。未对现有测试卡执行写入、格式化或 UID 修改。

自动测试覆盖帧分片、校验和、卡片响应边界、4K 扇区布局、值块/访问位冗余、备份冲突和 NDEF/APDU 边界。写卡、NTAG 实卡和 Type 4 模拟仍需要对应测试卡/手机验证。

## 使用

从预览 Release 下载 Apple Silicon 的 `.zip`，解压打开 `PCR532 Studio Rust.app`。这是本地 ad-hoc 签名构建，未经过 Apple 公证。程序无需安装 Python、Homebrew 或 Rust。

1. 选择 `/dev/cu.usbserial-*`、115200 波特率，点击「连接 / 识卡」。
2. 在「密钥与恢复」编辑字典，在「IC 数据」选择容量并读取。
3. 保存 JSON 保留未知块和隐藏密钥状态；只有完整数据和已知密钥才能导出完整二进制备份。
4. 写入时换上目标测试卡并重新识别；确认窗口绑定目标 UID、端口、块列表及数据快照。写后逐块回读；中途停止可能已完成部分写入。

数据保存在 `~/Library/Application Support/PCR532 Studio/`，不上传到服务器。可读取旧 Python 版生成的 JSON。原卡备份、密钥、日志、原厂安装包和反编译资料不进入 Git 历史。

## 功能

- Rust egui 桌面界面，串口枚举、后台线程和取消控制。
- PN532 UART ACK、普通/扩展帧、校验、超时与 ISO14443A 识卡。
- Classic Mini/1K/2K/4K 数据布局；字典认证、部分读取、选块写入、尾块检查与回读验证。
- JSON / MFD / BIN / DUMP / MCT / EML，HEX 编辑、备份比较与冲突检测合并。
- 密钥字典、访问位检查、值块生成/解码、本地归档。
- NTAG 页面读取、PWD 认证、213/215/216 用户区写入。完整读取结果和待写数据分开。
- 文本、网址、电话、名片、Wi-Fi、蓝牙、Android 应用 NDEF；纯 Rust Type 4 只读模拟。
- PCR532 HID/ID 厂商扩展只读指令；低频响应需实卡验证。

未完成：未知密钥恢复、Gen1a UID 写入迁移，以及 Windows 专有写卡、特殊卡、侦测卡、变色龙、手机/手环流程。原厂云服务需要独立服务端支持。

## 构建和检查

安装 [Rust](https://rustup.rs/) 和 Xcode Command Line Tools：

```sh
cargo run --bin pcr532-studio
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo run --bin xtask
```

`xtask` 本身也是 Rust，调用 Cargo 编译与 macOS 系统签名/归档工具，输出 `dist/PCR532 Studio Rust.app` 和带版本号的 ZIP。

命令行只读诊断：

```sh
cargo run --bin pcr532-studio -- scan /dev/cu.usbserial-110
cargo run --bin pcr532-studio -- read /dev/cu.usbserial-110 /tmp/card.json FFFFFFFFFFFF
```

界面渲染自检（不读写卡，生成七页截图后自动退出）：

```sh
PCR532_SMOKE_DIR=/tmp/pcr532-ui cargo run --bin pcr532-studio -- gui
```

## 项目结构

| 文件 | 用途 |
|---|---|
| `src/gui.rs` | egui 界面和确认流程 |
| `src/tasks.rs` | 后台任务、取消、归档 |
| `src/pn532.rs` | Rust PN532 UART 驱动和卡片命令 |
| `src/document.rs` | 备份、扇区映射、访问位、值块 |
| `src/ndef.rs` | NDEF 编码和有边界检查的 Type 4 APDU 状态机 |
| `src/vendor.rs` | 厂商只读扩展协议 |
| `src/bin/xtask.rs` | Rust Mac 打包工具 |

## 版本管理

- `main` 保存可构建的集成版本；新增工作使用 `feat/*`、`fix/*` 分支。
- 按 SemVer 打标签；迁移完成前使用 `-alpha.N` / `-beta.N` 预览标记。
- Cargo.lock 纳入 Git；CI 检查格式、Clippy、测试和 Mac 构建。
- 版本差异记录在 [CHANGELOG.md](CHANGELOG.md)，未完成事项在 GitHub Issues 跟踪。

独立实现代码使用 MIT 许可；依赖许可见 [THIRD_PARTY.md](THIRD_PARTY.md)。Rust crate 使用 macOS 系统 API/框架，不意味着系统底层代码也是 Rust。协议参考 [NXP PN532 手册](https://www.nxp.com/docs/en/user-guide/141520.pdf)。

实机只读取消自检（默认 CI 跳过，需自有/授权卡）：

```sh
PCR532_TEST_PORT=/dev/cu.usbserial-110 cargo test hardware_cancel_releases_device -- --ignored
```

目前已通过 16 项自动测试、七页界面渲染自检和上述实机取消/重连测试。后续恢复算法与专有功能分别在 [Issue #1](https://github.com/Jacky1n7/pcr532-studio/issues/1)、[Issue #2](https://github.com/Jacky1n7/pcr532-studio/issues/2) 跟踪。
