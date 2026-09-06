use crate::{
    document::{Card, Document},
    ndef,
    pn532::Reader,
};
use anyhow::{Result, ensure};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool, mpsc::Sender},
    time::Duration,
};

#[derive(Clone)]
pub enum Operation {
    FudanRead,
    FudanRecover,
    Diagnose {
        block: usize,
        kind: String,
    },
    Nested {
        source: usize,
        known: String,
        target: usize,
        kind: String,
        keys: Vec<String>,
    },
    VendorRead {
        hid: bool,
    },
    Scan,
    Read {
        keys: Vec<String>,
        blocks: usize,
    },
    Write {
        document: Document,
        indices: Vec<usize>,
        uid: String,
        keys: Vec<String>,
        trailers: bool,
    },
    NtagRead {
        pages: usize,
        password: String,
    },
    NtagWrite {
        start: usize,
        data: Vec<u8>,
        uid: String,
        password: String,
    },
    Emulate {
        message: Vec<u8>,
    },
}
#[derive(Clone)]
pub struct Job {
    pub port: String,
    pub baud: u32,
    pub operation: Operation,
}
pub enum Event {
    Log(String),
    Card(Card),
    Document(Document),
    Ntag(Card, Vec<u8>),
    Done(Result<()>),
}
pub fn data_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    let suffix = "Library/Application Support/PCR532 Studio";
    #[cfg(not(target_os = "macos"))]
    let suffix = ".local/share/pcr532-studio";
    PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into())).join(suffix)
}
pub fn archive(doc: &Document) -> Result<PathBuf> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("rust-{}.json", crate::document::unique_id()?));
    doc.save(&path)?;
    Ok(path)
}
pub fn run(job: Job, cancel: Arc<AtomicBool>, tx: &Sender<Event>) -> Result<()> {
    if let Operation::VendorRead { hid } = job.operation {
        let result = crate::vendor::read(&job.port, job.baud, hid, cancel)?;
        let _ = tx.send(Event::Log(result));
        return Ok(());
    }
    let mut reader = Reader::open(&job.port, job.baud, cancel)?;
    let mut progress = |s| {
        let _ = tx.send(Event::Log(s));
    };
    match job.operation {
        Operation::FudanRead => {
            let doc = crate::recovery::fudan_read(&mut reader, &mut progress)?;
            let path = archive(&doc)?;
            progress(format!(
                "读取 {}/{} 块，已归档 {}。普通 A/B 密钥仍需单独认证或恢复。",
                doc.known(),
                doc.blocks.len(),
                path.display()
            ));
            let _ = tx.send(Event::Document(doc));
        }
        Operation::FudanRecover => {
            let doc = crate::recovery::fudan_recover(&mut reader, &mut progress)?;
            let count: usize = doc.keys.values().map(|k| k.len()).sum();
            let path = archive(&doc)?;
            progress(format!(
                "读取 {}/{} 块；已验证 {count}/32 个普通密钥。归档 {}",
                doc.known(),
                doc.blocks.len(),
                path.display()
            ));
            let _ = tx.send(Event::Document(doc));
        }
        Operation::Diagnose { block, kind } => {
            let report = crate::recovery::diagnose(&mut reader, block, &kind, 8, &mut progress)?;
            progress(format!(
                "随机数 {} 种；弱 PRNG 样本 {}/{}。{}",
                report.distinct,
                report.weak_prng_samples,
                report.nonces.len(),
                report.conclusion
            ));
            let path = data_dir()
                .join("diagnostics")
                .join(format!("nonce-{}.json", crate::document::unique_id()?));
            std::fs::create_dir_all(data_dir().join("diagnostics"))?;
            crate::document::atomic_write(&path, &serde_json::to_vec_pretty(&report)?)?;
            progress(format!("本地诊断报告：{}", path.display()));
            let _ = tx.send(Event::Card(report.card));
        }
        Operation::Nested {
            source,
            known,
            target,
            kind,
            mut keys,
        } => {
            let recovered =
                crate::recovery::nested(&mut reader, source, &known, target, &kind, &mut progress)?;
            if let Some(key) = recovered {
                keys.push(key);
                keys.push(known);
                keys.sort();
                keys.dedup();
                let blocks = crate::pn532::capacity(&reader.select(None)?)?;
                let doc = reader.read_classic(&keys, blocks, &mut progress)?;
                let path = archive(&doc)?;
                progress(format!(
                    "恢复后读取 {}/{} 块，归档 {}",
                    doc.known(),
                    doc.blocks.len(),
                    path.display()
                ));
                let _ = tx.send(Event::Document(doc));
            } else {
                anyhow::bail!(
                    "本次普通 nested 未恢复密钥；不能据此断言卡片不可恢复。hardnested / DarkSide 尚未接入。"
                );
            }
        }
        Operation::VendorRead { .. } => unreachable!(),
        Operation::Scan => {
            progress(reader.firmware()?);
            let _ = tx.send(Event::Card(reader.select(None)?));
        }
        Operation::Read { keys, blocks } => {
            let doc = reader.read_classic(&keys, blocks, &mut progress)?;
            let path = archive(&doc)?;
            progress(format!(
                "读取 {}/{} 块，已归档 {}",
                doc.known(),
                doc.blocks.len(),
                path.display()
            ));
            let _ = tx.send(Event::Document(doc));
        }
        Operation::Write {
            document,
            indices,
            uid,
            keys,
            trailers,
        } => {
            let n =
                reader.write_classic(&document, &indices, &uid, &keys, trailers, &mut progress)?;
            progress(format!("已写入并验证 {n} 块"));
        }
        Operation::NtagRead { pages, password } => {
            let (card, data) = reader.ntag_read(pages, &password)?;
            let _ = tx.send(Event::Ntag(card, data));
        }
        Operation::NtagWrite {
            start,
            data,
            uid,
            password,
        } => {
            let n = reader.ntag_write(start, &data, &uid, &password, &mut progress)?;
            progress(format!("已写入并验证 {n} 页"));
        }
        Operation::Emulate { message } => {
            let mut tag = ndef::Type4::new(message)?;
            // Passive ISO14443-4 PICC: Mode + MIFARE[6] + FeliCa[18] + NFCID3t[10] + GtLen + TkLen.
            let mut params = vec![5, 4, 0, 0x12, 0x34, 0x56, 0x20];
            params.extend([0u8; 18]);
            params.extend([0u8; 10]);
            params.extend([0, 0]);
            progress("等待手机读取 NDEF Type 4，点停止退出".into());
            reader.command(0x8c, &params, Duration::from_secs(3600))?;
            loop {
                reader.check_cancel()?;
                let incoming = reader.command(0x86, &[], Duration::from_secs(3600))?;
                ensure!(incoming.first() == Some(&0), "目标模式连接结束");
                let response = tag.respond(&incoming[1..]);
                let result = reader.command(0x8e, &response, Duration::from_secs(3))?;
                ensure!(result.first() == Some(&0), "发送 NDEF 响应失败");
            }
        }
    }
    Ok(())
}
