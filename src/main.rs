use anyhow::{Context, Result};
use pcr532_studio::{document, pn532};
use std::sync::{Arc, atomic::AtomicBool};
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let op = args.get(1).map(String::as_str).unwrap_or("gui");
    match op {
        "--version" | "-V" => println!("PCR532 Studio {}", env!("CARGO_PKG_VERSION")),
        "gui" => pcr532_studio::gui::run()?,
        "scan" | "read" => {
            let port = args.get(2).context("请指定串口")?;
            let mut reader = pn532::Reader::open(port, 115200, Arc::new(AtomicBool::new(false)))?;
            println!("{}", reader.firmware()?);
            if op == "scan" {
                println!("{}", serde_json::to_string_pretty(&reader.select(None)?)?);
            } else {
                let output = args.get(3).context("请指定输出文件")?;
                let keys = document::parse_keys(
                    args.get(4).map(String::as_str).unwrap_or("FFFFFFFFFFFF"),
                )?;
                let doc = reader.read_classic(&keys, 64, |m| println!("{m}"))?;
                doc.save(std::path::Path::new(output))?;
                println!("读取 {}/{} 块", doc.known(), doc.blocks.len());
            }
        }
        _ => println!("PCR532 Studio Rust · scan PORT | read PORT OUTPUT.json [KEY]"),
    }
    Ok(())
}
