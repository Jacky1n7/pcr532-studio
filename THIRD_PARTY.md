# Dependencies

## Recovery algorithm attribution

The combined application from 0.2.0-alpha.3 is distributed under **GPL-3.0-or-later** (see LICENSE). Original independent MIT code retains its notice in LICENSE-MIT. Previous release tags retain their original licensing.

`src/crypto1.rs` adapts `crypto1.c`, `crapto1.c`, and `crapto1.h` from [nfc-tools/mfoc-hardnested](https://github.com/nfc-tools/mfoc-hardnested/tree/a6007437405a0f18642a4bbca2eeba67c623d736/src), Copyright (C) 2008–2014 bla <blapost@gmail.com>, GPL-2.0-or-later. `src/recovery.rs` adapts the software authentication and nested recovery sequence from that project's `mfoc.c` (Mifare Classic Offline Cracker; Nethemba; libnfc porting by Michal Boska and Romuald Conty), GPL-2.0-or-later. These upstream works permit redistribution under GPL version 2 or any later version; the combined application uses GPL version 3 or later. They come WITHOUT ANY WARRANTY, including MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.

The Fudan static-encrypted-nonce workflow in `src/recovery.rs` follows the published protocol and candidate filtering approach in [RfidResearchGroup/proxmark3](https://github.com/RfidResearchGroup/proxmark3/tree/fc355df0507778e48d9beb17fa10377a6474fc38), including `armsrc/mifarecmd.c` and `tools/mfc/card_only/staticnested_2x1nt_rf08s_1key.c`, Copyright (C) Proxmark3 contributors, GPL-3.0-or-later. The associated research is Philippe Teuwen, “MIFARE Classic: exposing the static encrypted nonce variant,” IACR ePrint 2024/1275.

Changes: Rust state representation, owned vectors instead of pointer tables, bounded/cancellable search, broader timing calibration, multiple-capture filtering, cross-sector candidate intersection, authenticated-key validation, and PN532 serial integration. No upstream C binary is linked or invoked. The corresponding Rust source is provided in this repository and release tags.

Application and packaging code in `src/` is Rust. The application does not load Python, Qt, libnfc, MFCUK, mfoc-hardnested, vendor DLLs or EXEs. Rust crates still link to macOS system frameworks and system libraries; “Rust” does not mean replacing the operating system.

The exact dependency graph is recorded in Cargo.lock. Direct dependencies:

| Crate | Purpose | License |
|---|---|---|
| eframe / egui | Rust desktop interface | MIT OR Apache-2.0 |
| serialport | System serial API bindings | MPL-2.0 |
| rfd | Native file and confirmation dialogs | MIT |
| serde / serde_json | Documents | MIT OR Apache-2.0 |
| anyhow | Error propagation | MIT OR Apache-2.0 |
| hex | Hex encoding | MIT OR Apache-2.0 |
| png | Opt-in app renderer smoke-test images | MIT OR Apache-2.0 |

No proprietary vendor source or binaries are distributed. The PN532 implementation follows [NXP UM0701-02](https://www.nxp.com/docs/en/user-guide/141520.pdf). NDEF Wi-Fi and Bluetooth layouts were cross-checked against [ndeflib documentation](https://ndeflib.readthedocs.io/en/latest/), without a Python runtime dependency.

`cargo about generate about.hbs > dist/THIRD_PARTY.html` generates full transitive license notices using the checked-in configuration. Include the generated notices with distributable builds.
