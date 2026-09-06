//! Build and package the macOS app using Rust and system signing/archive utilities.
use anyhow::{Result, ensure};
use std::{path::Path, process::Command};
fn checked(mut command: Command) -> Result<()> {
    let status = command.status()?;
    ensure!(status.success(), "Command failed: {command:?}");
    Ok(())
}
fn main() -> Result<()> {
    ensure!(
        cfg!(target_os = "macos"),
        "macOS packaging must run on macOS"
    );
    std::env::set_current_dir(env!("CARGO_MANIFEST_DIR"))?;
    let mut build = Command::new("cargo");
    build.args(["build", "--release", "--locked", "--bin", "pcr532-studio"]);
    checked(build)?;
    let version = env!("CARGO_PKG_VERSION");
    let app_path = Path::new("dist")
        .join(version)
        .join("PCR532 Studio Rust.app");
    let app = app_path.as_path();
    let macos = app.join("Contents/MacOS");
    let resources = app.join("Contents/Resources");
    std::fs::create_dir_all(&macos)?;
    std::fs::create_dir_all(&resources)?;
    std::fs::copy("target/release/pcr532-studio", macos.join("pcr532-studio"))?;
    let short = version.split('-').next().unwrap_or(version);
    let build_version = version
        .replace("-alpha.", "a")
        .replace("-beta.", "b")
        .replace("-rc.", "fc");
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>PCR532 Studio Rust</string>
<key>CFBundleDisplayName</key><string>PCR532 Studio Rust</string>
<key>CFBundleIdentifier</key><string>local.pcr532.studio.rust</string>
<key>CFBundleExecutable</key><string>pcr532-studio</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>{short}</string>
<key>CFBundleVersion</key><string>{build_version}</string>
<key>LSMinimumSystemVersion</key><string>12.0</string>
<key>NSHighResolutionCapable</key><true/>
<key>NSPrincipalClass</key><string>NSApplication</string>
</dict></plist>"#
    );
    std::fs::write(app.join("Contents/Info.plist"), plist)?;
    for file in [
        "README.md",
        "LICENSE",
        "LICENSE-MIT",
        "THIRD_PARTY.md",
        "THIRD_PARTY.html",
    ] {
        std::fs::copy(file, resources.join(file))?;
    }
    let mut sign = Command::new("codesign");
    sign.args(["--force", "--deep", "--sign", "-"]).arg(app);
    checked(sign)?;
    let mut verify = Command::new("codesign");
    verify.args(["--verify", "--deep", "--strict"]).arg(app);
    checked(verify)?;
    let zip = format!(
        "dist/pcr532-studio-{version}-macos-{}.zip",
        std::env::consts::ARCH
    );
    let mut archive = Command::new("ditto");
    archive
        .args(["-c", "-k", "--sequesterRsrc", "--keepParent"])
        .arg(app)
        .arg(&zip);
    checked(archive)?;
    println!("Built {} and {zip}", app.display());
    Ok(())
}
