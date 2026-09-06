use anyhow::{Context, Result};
use pcr532_studio::{document, pn532};
use std::sync::{Arc, atomic::AtomicBool};
fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let op = args.get(1).map(String::as_str).unwrap_or("gui");
    match op {
        "--version" | "-V" => println!("PCR532 Studio {}", env!("CARGO_PKG_VERSION")),
        "gui" => pcr532_studio::gui::run()?,
        "scan" | "read" | "diagnose" | "nested" | "fudan-read" | "fudan-recover" | "verify" => {
            let port = args.get(2).context("请指定串口")?;
            let mut reader = pn532::Reader::open(port, 115200, Arc::new(AtomicBool::new(false)))?;
            println!("{}", reader.firmware()?);
            if op == "verify" {
                let path = args.get(3).context("请指定待验证 JSON 备份")?;
                let expected = document::Document::load(std::path::Path::new(path))?;
                let keys: Vec<_> = expected
                    .keys
                    .values()
                    .flat_map(|k| k.values().cloned())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect();
                let uid = expected.card.get("uid").context("备份没有 UID")?;
                reader.select(Some(uid))?;
                let actual =
                    reader.read_classic(&keys, expected.blocks.len(), |m| println!("{m}"))?;
                anyhow::ensure!(actual.card.get("uid") == Some(uid), "验证过程中卡片已更换");
                anyhow::ensure!(
                    actual.blocks == expected.blocks && actual.known() == actual.blocks.len(),
                    "普通认证读取与备份不一致"
                );
                println!(
                    "普通 A/B 密钥重读验证通过：{}/{} 块一致",
                    actual.known(),
                    actual.blocks.len()
                );
            } else if op == "fudan-read" || op == "fudan-recover" {
                let path = args.get(3).context("请指定 JSON 输出文件")?;
                let doc = if op == "fudan-read" {
                    pcr532_studio::recovery::fudan_read(&mut reader, |m| println!("{m}"))?
                } else {
                    pcr532_studio::recovery::fudan_recover(&mut reader, |m| println!("{m}"))?
                };
                doc.save(std::path::Path::new(path))?;
                println!("读取 {}/{} 块", doc.known(), doc.blocks.len());
            } else if op == "nested" {
                let target = args.get(3).context("请指定目标块")?.parse()?;
                let source = args.get(4).context("请指定已知密钥块")?.parse()?;
                let key = args.get(5).context("请指定已知 A 密钥")?;
                let kind = args.get(6).map(String::as_str).unwrap_or("A");
                let result =
                    pcr532_studio::recovery::nested(&mut reader, source, key, target, kind, |m| {
                        println!("{m}")
                    })?;
                println!(
                    "{}",
                    result.unwrap_or_else(|| "此次有界尝试未恢复密钥".into())
                );
            } else if op == "diagnose" {
                let block = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(0);
                let report = pcr532_studio::recovery::diagnose(&mut reader, block, "A", 8, |m| {
                    eprintln!("{m}")
                })?;
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else if op == "scan" {
                let card = reader.select(None)?;
                eprintln!("卡型：{}", pn532::classify(&card));
                println!("{}", serde_json::to_string_pretty(&card)?);
            } else {
                let output = args.get(3).context("请指定输出文件")?;
                let keys = document::parse_keys(
                    args.get(4).map(String::as_str).unwrap_or("FFFFFFFFFFFF"),
                )?;
                let blocks = pn532::capacity(&reader.select(None)?)?;
                let doc = reader.read_classic(&keys, blocks, |m| println!("{m}"))?;
                doc.save(std::path::Path::new(output))?;
                println!("读取 {}/{} 块", doc.known(), doc.blocks.len());
            }
        }
        _ => println!(
            "PCR532 Studio Rust · scan/read/diagnose/nested/fudan-read/fudan-recover/verify"
        ),
    }
    Ok(())
}
