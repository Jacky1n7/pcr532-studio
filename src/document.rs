use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Card {
    pub uid: String,
    pub atqa: String,
    pub sak: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Document {
    pub format: String,
    pub version: u32,
    pub blocks: Vec<Option<String>>,
    #[serde(default)]
    pub keys: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    pub card: BTreeMap<String, String>,
    #[serde(default)]
    pub notes: BTreeMap<String, serde_json::Value>,
}
pub fn sectors(blocks: usize) -> Result<usize> {
    match blocks {
        20 => Ok(5),
        64 => Ok(16),
        128 => Ok(32),
        256 => Ok(40),
        _ => bail!("容量须为 20/64/128/256 块"),
    }
}
pub fn sector_blocks(s: usize) -> Result<std::ops::Range<usize>> {
    ensure!(s < 40, "扇区超出范围");
    Ok(if s < 32 {
        s * 4..s * 4 + 4
    } else {
        128 + (s - 32) * 16..128 + (s - 31) * 16
    })
}
pub fn sector_of(b: usize) -> Result<usize> {
    ensure!(b < 256, "块号超出范围");
    Ok(if b < 128 { b / 4 } else { 32 + (b - 128) / 16 })
}
pub fn is_trailer(b: usize) -> bool {
    sector_of(b)
        .and_then(sector_blocks)
        .is_ok_and(|r| r.end - 1 == b)
}
pub fn bytes(text: &str, len: Option<usize>) -> Result<Vec<u8>> {
    let data = hex::decode(
        text.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>(),
    )
    .context("无效的十六进制数据")?;
    if let Some(n) = len {
        ensure!(data.len() == n, "需要 {n} 字节");
    }
    Ok(data)
}
pub fn access(data: &[u8]) -> Result<[u8; 4]> {
    ensure!(data.len() == 16, "尾块须为 16 字节");
    let (c1, c2, c3) = (data[7] >> 4, data[8] & 15, data[8] >> 4);
    ensure!(
        data[6] & 15 == c1 ^ 15 && data[6] >> 4 == c2 ^ 15 && data[7] & 15 == c3 ^ 15,
        "访问位冗余校验失败"
    );
    Ok(std::array::from_fn(|i| {
        ((c1 >> i & 1) << 2) | ((c2 >> i & 1) << 1) | (c3 >> i & 1)
    }))
}
pub fn value_block(value: i32, address: u8) -> [u8; 16] {
    let mut d = [0u8; 16];
    let v = value.to_le_bytes();
    d[..4].copy_from_slice(&v);
    d[8..12].copy_from_slice(&v);
    for i in 0..4 {
        d[4 + i] = !v[i];
    }
    d[12..].copy_from_slice(&[address, !address, address, !address]);
    d
}
pub fn decode_value(d: &[u8]) -> Result<(i32, u8)> {
    ensure!(d.len() == 16, "值块须为 16 字节");
    let value = i32::from_le_bytes(d[..4].try_into()?);
    ensure!(value_block(value, d[12]) == d, "不是有效值块");
    Ok((value, d[12]))
}
pub fn parse_keys(text: &str) -> Result<Vec<String>> {
    let mut keys = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let k = hex::encode_upper(bytes(line, Some(6))?);
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    ensure!(!keys.is_empty(), "请输入至少一个密钥");
    Ok(keys)
}
impl Document {
    pub fn empty(n: usize) -> Result<Self> {
        sectors(n)?;
        Ok(Self {
            format: "pcr532-document".into(),
            version: 1,
            blocks: vec![None; n],
            keys: BTreeMap::new(),
            card: BTreeMap::new(),
            notes: BTreeMap::new(),
        })
    }
    pub fn blank(n: usize) -> Result<Self> {
        let mut d = Self::empty(n)?;
        for b in 0..n {
            d.blocks[b] = Some(if is_trailer(b) {
                "FFFFFFFFFFFFFF078069FFFFFFFFFFFF".into()
            } else {
                "00".repeat(16)
            });
        }
        Ok(d)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.format == "pcr532-document" && self.version == 1,
            "不支持的文档版本"
        );
        let count = sectors(self.blocks.len())?;
        for b in self.blocks.iter().flatten() {
            bytes(b, Some(16))?;
        }
        for (s, keys) in &self.keys {
            ensure!(s.parse::<usize>()? < count, "密钥扇区越界");
            for (k, v) in keys {
                ensure!(k == "A" || k == "B", "密钥类型必须为 A/B");
                bytes(v, Some(6))?;
            }
        }
        Ok(())
    }
    pub fn bind(&mut self, c: &Card) {
        self.card.insert("uid".into(), c.uid.clone());
        self.card.insert("atqa".into(), c.atqa.clone());
        self.card.insert("sak".into(), c.sak.clone());
    }
    pub fn known(&self) -> usize {
        self.blocks.iter().filter(|x| x.is_some()).count()
    }
    pub fn live(&self) -> bool {
        self.notes.get("source").and_then(|v| v.as_str()) == Some("live")
    }
    pub fn trailer_keys_known(&self, s: usize) -> bool {
        self.keys
            .get(&s.to_string())
            .is_some_and(|k| k.contains_key("A") && k.contains_key("B"))
    }
    pub fn set_block(&mut self, b: usize, text: &str) -> Result<()> {
        ensure!(b < self.blocks.len(), "块号越界");
        let data = hex::encode_upper(bytes(text, Some(16))?);
        if is_trailer(b) {
            self.keys.insert(
                sector_of(b)?.to_string(),
                BTreeMap::from([
                    ("A".into(), data[..12].into()),
                    ("B".into(), data[20..].into()),
                ]),
            );
            self.notes
                .insert("edited_keys".into(), "编辑密钥未经目标卡验证".into());
        }
        self.blocks[b] = Some(data);
        Ok(())
    }
    pub fn load(path: &Path) -> Result<Self> {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        if ext == "json" {
            let d: Self = serde_json::from_slice(&std::fs::read(path)?)?;
            d.validate()?;
            return Ok(d);
        }
        let data = if ["mct", "eml", "txt"].contains(&ext.as_str()) {
            let text = std::fs::read_to_string(path)?;
            let mut data = Vec::new();
            let mut next_sector = 0;
            let mut limit = None;
            for raw in text.lines() {
                let line = raw.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if line.starts_with('+') {
                    ensure!(
                        line == format!("+Sector: {next_sector}"),
                        "MCT 扇区须从 0 连续排列"
                    );
                    let range = sector_blocks(next_sector)?;
                    ensure!(data.len() / 16 == range.start, "MCT 扇区块数错误");
                    limit = Some(range.end);
                    next_sector += 1;
                } else {
                    if let Some(limit) = limit {
                        ensure!(data.len() / 16 < limit, "MCT 缺少扇区标题");
                    }
                    data.extend(bytes(line, Some(16))?);
                }
            }
            if let Some(limit) = limit {
                ensure!(data.len() / 16 == limit, "MCT 最后一个扇区不完整");
            }
            data
        } else {
            std::fs::read(path)?
        };
        ensure!(data.len() % 16 == 0, "备份长度错误");
        let mut d = Self::empty(data.len() / 16)?;
        for (i, b) in data.as_chunks::<16>().0.iter().enumerate() {
            d.set_block(i, &hex::encode_upper(b))?;
        }
        d.notes.insert("keys".into(), "导入密钥未经实卡验证".into());
        Ok(d)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        let data = if ext == "json" {
            serde_json::to_vec_pretty(self)?
        } else {
            ensure!(
                self.known() == self.blocks.len(),
                "存在未读块，请保存 JSON 保留缺失状态"
            );
            if self.live() {
                for s in 0..sectors(self.blocks.len())? {
                    ensure!(self.trailer_keys_known(s), "存在隐藏密钥，请保存 JSON");
                }
            }
            let mut all = Vec::new();
            for (i, b) in self.blocks.iter().enumerate() {
                let mut raw = bytes(b.as_deref().unwrap_or(""), Some(16))?;
                if is_trailer(i)
                    && let Some(keys) = self.keys.get(&sector_of(i)?.to_string())
                {
                    for (key, off) in [("A", 0), ("B", 10)] {
                        if let Some(k) = keys.get(key) {
                            raw[off..off + 6].copy_from_slice(&bytes(k, Some(6))?);
                        }
                    }
                }
                all.extend(raw);
            }
            if ["mct", "eml", "txt"].contains(&ext.as_str()) {
                let mut out = String::new();
                for s in 0..sectors(self.blocks.len())? {
                    if ext != "eml" {
                        out.push_str(&format!("+Sector: {s}\n"));
                    }
                    for b in sector_blocks(s)? {
                        out.push_str(&hex::encode_upper(&all[b * 16..b * 16 + 16]));
                        out.push('\n');
                    }
                }
                out.into_bytes()
            } else {
                all
            }
        };
        atomic_write(path, &data)
    }
    pub fn merge(docs: &[Self]) -> Result<Self> {
        let first = docs.first().context("没有备份")?;
        let mut out = Self::empty(first.blocks.len())?;
        for doc in docs {
            doc.validate()?;
            ensure!(doc.blocks.len() == out.blocks.len(), "容量冲突");
            if let (Some(a), Some(b)) = (out.card.get("uid"), doc.card.get("uid")) {
                ensure!(a == b, "不同 UID 不能合并");
            }
            out.card.extend(doc.card.clone());
            for (i, b) in doc.blocks.iter().enumerate() {
                if let Some(b) = b {
                    if let Some(a) = &out.blocks[i] {
                        ensure!(a.eq_ignore_ascii_case(b), "块 {i} 冲突");
                    }
                    out.blocks[i] = Some(b.to_uppercase());
                }
            }
            for (s, ks) in &doc.keys {
                let target = out.keys.entry(s.clone()).or_default();
                for (k, v) in ks {
                    if let Some(a) = target.get(k) {
                        ensure!(a.eq_ignore_ascii_case(v), "扇区 {s} 密钥冲突");
                    }
                    target.insert(k.clone(), v.to_uppercase());
                }
            }
            if doc.live() {
                out.notes.insert("source".into(), "live".into());
            }
        }
        Ok(out)
    }
}
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let tmp = parent.join(format!(".pcr532-{}-{stamp}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn geometry() {
        let all: Vec<_> = (0..40).flat_map(|s| sector_blocks(s).unwrap()).collect();
        assert_eq!(all, (0..256).collect::<Vec<_>>());
        for s in 0..40 {
            for b in sector_blocks(s).unwrap() {
                assert_eq!(sector_of(b).unwrap(), s);
            }
        }
    }
    #[test]
    fn value_redundancy() {
        for n in [i32::MIN, -1, 0, i32::MAX] {
            let mut d = value_block(n, 127);
            assert_eq!(decode_value(&d).unwrap(), (n, 127));
            d[7] ^= 1;
            assert!(decode_value(&d).is_err());
        }
    }
    #[test]
    fn access_redundancy() {
        let d = bytes("FFFFFFFFFFFFFF078069FFFFFFFFFFFF", Some(16)).unwrap();
        assert_eq!(access(&d).unwrap(), [0, 0, 0, 1]);
        for i in 0..24 {
            let mut bad = d.clone();
            bad[6 + i / 8] ^= 1 << (i % 8);
            assert!(access(&bad).is_err());
        }
    }
    #[test]
    fn merge_conflict() {
        let mut a = Document::empty(64).unwrap();
        a.set_block(1, &"AA".repeat(16)).unwrap();
        let mut b = Document::empty(64).unwrap();
        b.set_block(2, &"BB".repeat(16)).unwrap();
        assert_eq!(Document::merge(&[a.clone(), b.clone()]).unwrap().known(), 2);
        b.set_block(1, &"CC".repeat(16)).unwrap();
        assert!(Document::merge(&[a, b]).is_err());
    }
    #[test]
    fn legacy_json() {
        let d:Document=serde_json::from_str(r#"{"format":"pcr532-document","version":1,"blocks":[null],"keys":{},"card":{},"notes":{}}"#).unwrap();
        assert!(d.validate().is_err());
    }
}
