# PCR532 Studio · Rust

用于自有或获授权测试卡的 macOS 桌面工具。应用界面、PN532 串口通信、卡片数据处理、NDEF 和打包工具均用 Rust 实现；不调用 Python、Qt、libnfc 或旧的 C 恢复程序。

**当前版本：0.2.0-alpha.3。Rust 基础迁移已可运行，尚未完成旧版所有功能的迁移，更不是 Windows 原版的完整复刻。** 新增纯 Rust 随机数诊断、普通 nested，以及 Fudan 固定加密随机数卡的本地完整读取和密钥恢复；hardnested、DarkSide 尚未实现。完整差异见 [迁移状态](docs/FEATURES.txt)。

## 已验证

2026-09-06，在开发 Mac 和已连接的 PCR532 上，Rust 直连成功识别 PN532 1.6 与 Classic 1K 兼容测试卡。字典读取为 56/64 块；Fudan 本地恢复找回剩余两个扇区的 A/B 密钥，随后使用普通认证重新读取，得到 **64/64 块一致**。每个诊断读取块均通过 CRC 和奇偶校验。未对测试卡执行写入、格式化或 UID 修改。

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
- 纯 Rust Crypto1、随机数诊断、弱 PRNG 普通 nested，以及 Fudan 固定加密随机数卡的完整读取和跨扇区复用密钥恢复。
- NTAG 页面读取、PWD 认证、213/215/216 用户区写入。完整读取结果和待写数据分开。
- 文本、网址、电话、名片、Wi-Fi、蓝牙、Android 应用 NDEF；纯 Rust Type 4 只读模拟。
- PCR532 HID/ID 厂商扩展只读指令；低频响应需实卡验证。

未完成：hardnested / DarkSide 恢复、Gen1a UID 写入迁移，以及 Windows 专有写卡、特殊卡、侦测卡、变色龙、手机/手环流程。本阶段只迁移无需注册、充值的本地功能；原厂账号、充值、购买数据、云端求助和付费恢复暂不接入。

## 构建和检查

安装 [Rust](https://rustup.rs/) 和 Xcode Command Line Tools：

```sh
cargo run --bin pcr532-studio
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo run --bin xtask
```

`xtask` 本身也是 Rust，调用 Cargo 编译与 macOS 系统签名/归档工具，输出 `dist/<版本>/PCR532 Studio Rust.app` 和带版本号的 ZIP。

命令行只读诊断：

```sh
cargo run --bin pcr532-studio -- scan /dev/cu.usbserial-110
cargo run --bin pcr532-studio -- read /dev/cu.usbserial-110 /tmp/card.json FFFFFFFFFFFF
cargo run --release --bin pcr532-studio -- fudan-read /dev/cu.usbserial-110 /tmp/card.json
cargo run --release --bin pcr532-studio -- fudan-recover /dev/cu.usbserial-110 /tmp/card-with-keys.json
cargo run --release --bin pcr532-studio -- verify /dev/cu.usbserial-110 /tmp/card-with-keys.json
```

界面渲染自检（不读写卡，生成七页截图后自动退出）：

```sh
PCR532_SMOKE_DIR=/tmp/pcr532-ui cargo run --bin pcr532-studio -- gui
```

## 本地恢复（实验）

「密钥与恢复」页面提供三条本地路径：字典读取、普通 nested、Fudan 固定加密随机数恢复。普通 nested 需要一个已知 **A** 密钥；Fudan 恢复会先用公开的诊断认证只读采集，再利用跨扇区密钥复用缩小候选。任何恢复出的普通密钥必须通过实卡 A/B 认证才会写入备份。

所有恢复任务支持停止；普通 nested 搜索阶段上限 240 秒、最多验证 64 个候选。Fudan 卡即使能完整读取数据，也不一定能恢复每个普通密钥；界面会分别报告完整区块数和已验证密钥数。失败会显示实际原因，不自动转入云服务。

```sh
cargo run --release --bin pcr532-studio -- diagnose /dev/cu.usbserial-110 4
cargo run --release --bin pcr532-studio -- nested /dev/cu.usbserial-110 4 0 FFFFFFFFFFFF A
```

上述例子中的块号和密钥仅为测试输入，请按自己的卡片调整。诊断报告仅保存在本机 `diagnostics/` 子目录；不会上传随机数、密钥或卡片数据。随机数符合弱 PRNG 不等于必然恢复成功。

## 项目结构

| 文件 | 用途 |
|---|---|
| `src/gui.rs` | egui 界面和确认流程 |
| `src/tasks.rs` | 后台任务、取消、归档 |
| `src/pn532.rs` | Rust PN532 UART 驱动和卡片命令 |
| `src/crypto1.rs` | Crypto1 运算、状态回退与候选恢复 |
| `src/recovery.rs` | 随机数诊断、nested 与 Fudan 本地恢复 |
| `src/document.rs` | 备份、扇区映射、访问位、值块 |
| `src/ndef.rs` | NDEF 编码和有边界检查的 Type 4 APDU 状态机 |
| `src/vendor.rs` | 厂商只读扩展协议 |
| `src/bin/xtask.rs` | Rust Mac 打包工具 |

## 版本管理

- `main` 保存可构建的集成版本；新增工作使用 `feat/*`、`fix/*` 分支。
- 按 SemVer 打标签；迁移完成前使用 `-alpha.N` / `-beta.N` 预览标记。
- Cargo.lock 纳入 Git；CI 检查格式、Clippy、测试和 Mac 构建。
- 版本差异记录在 [CHANGELOG.md](CHANGELOG.md)，未完成事项在 GitHub Issues 跟踪。

从 alpha.3 起，包含 GPL 恢复算法移植的完整应用按 GPL-3.0-or-later 发布；原有独立代码的 MIT 声明保留在 LICENSE-MIT。署名和依赖许可见 [THIRD_PARTY.md](THIRD_PARTY.md)。Rust crate 使用 macOS 系统 API/框架，不意味着系统底层代码也是 Rust。协议参考 [NXP PN532 手册](https://www.nxp.com/docs/en/user-guide/141520.pdf)。

实机只读取消自检（默认 CI 跳过，需自有/授权卡）：

```sh
PCR532_TEST_PORT=/dev/cu.usbserial-110 cargo test hardware_cancel_releases_device -- --ignored
```

目前已通过 21 项自动测试、七页界面渲染自检和上述实机取消/重连测试。后续恢复算法与专有功能分别在 [Issue #1](https://github.com/Jacky1n7/pcr532-studio/issues/1)、[Issue #2](https://github.com/Jacky1n7/pcr532-studio/issues/2) 跟踪。
