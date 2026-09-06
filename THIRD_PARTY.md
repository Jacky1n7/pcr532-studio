# Dependencies

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
