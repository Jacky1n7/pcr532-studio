//! PN532 UART framing and card commands, implemented in Rust from NXP UM0701-02.
use crate::document::{self, Card, Document};
use anyhow::{Context, Result, bail, ensure};
use serialport::{ClearBuffer, SerialPort};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub const ACK: [u8; 6] = [0, 0, 255, 0, 255, 0];
pub fn encode(payload: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        !payload.is_empty() && payload.len() <= 1024,
        "PN532 帧长度超限"
    );
    let mut out = vec![0, 0, 255];
    if payload.len() < 255 {
        let n = payload.len() as u8;
        out.extend([n, 0u8.wrapping_sub(n)]);
    } else {
        let n = (payload.len() as u16).to_be_bytes();
        out.extend([
            255,
            255,
            n[0],
            n[1],
            0u8.wrapping_sub(n[0].wrapping_add(n[1])),
        ]);
    }
    out.extend(payload);
    out.extend([
        0u8.wrapping_sub(payload.iter().fold(0u8, |a, b| a.wrapping_add(*b))),
        0,
    ]);
    Ok(out)
}
#[derive(Debug, PartialEq)]
pub enum Frame {
    Ack,
    Data(Vec<u8>),
}
pub fn decode(buf: &mut Vec<u8>) -> Result<Option<Frame>> {
    let Some(pos) = buf.windows(3).position(|w| w == [0, 0, 255]) else {
        if buf.len() > 2 {
            buf.drain(..buf.len() - 2);
        }
        return Ok(None);
    };
    if pos > 0 {
        buf.drain(..pos);
    }
    if buf.len() < 6 {
        return Ok(None);
    }
    if buf[..6] == ACK {
        buf.drain(..6);
        return Ok(Some(Frame::Ack));
    }
    if buf[..6] == [0, 0, 255, 255, 0, 0] {
        buf.drain(..6);
        bail!("读卡器返回 NACK");
    }
    let (n, start) = if buf[3] == 255 && buf[4] == 255 {
        if buf.len() < 8 {
            return Ok(None);
        }
        ensure!(
            buf[5].wrapping_add(buf[6]).wrapping_add(buf[7]) == 0,
            "扩展帧长度校验失败"
        );
        (u16::from_be_bytes([buf[5], buf[6]]) as usize, 8)
    } else {
        ensure!(buf[3].wrapping_add(buf[4]) == 0, "帧长度校验失败");
        (buf[3] as usize, 5)
    };
    ensure!((1..=1024).contains(&n), "响应长度超限");
    if buf.len() < start + n + 2 {
        return Ok(None);
    }
    ensure!(
        buf[start..=start + n]
            .iter()
            .fold(0u8, |a, b| a.wrapping_add(*b))
            == 0
            && buf[start + n + 1] == 0,
        "响应校验失败"
    );
    let data = buf[start..start + n].to_vec();
    buf.drain(..start + n + 2);
    Ok(Some(Frame::Data(data)))
}
pub struct Reader {
    port: Box<dyn SerialPort>,
    buffer: Vec<u8>,
    pub cancel: Arc<AtomicBool>,
    pub card: Option<Card>,
    target: u8,
}
impl Drop for Reader {
    fn drop(&mut self) {
        let _ = self.port.write_all(&ACK);
    }
}
impl Reader {
    pub fn open(path: &str, baud: u32, cancel: Arc<AtomicBool>) -> Result<Self> {
        let mut port = serialport::new(path, baud)
            .timeout(Duration::from_millis(100))
            .open()
            .context("无法打开串口（检查端口或占用）")?;
        port.clear(ClearBuffer::All)?;
        port.write_all(&[0x55, 0x55, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])?;
        std::thread::sleep(Duration::from_millis(50));
        let mut r = Self {
            port,
            buffer: Vec::new(),
            cancel,
            card: None,
            target: 1,
        };
        r.command(0x14, &[1], Duration::from_secs(2))?;
        r.command(0x32, &[5, 0xff, 1, 2], Duration::from_secs(2))?;
        Ok(r)
    }
    pub fn check_cancel(&self) -> Result<()> {
        ensure!(!self.cancel.load(Ordering::Relaxed), "任务已停止");
        Ok(())
    }
    pub fn command(&mut self, cmd: u8, args: &[u8], timeout: Duration) -> Result<Vec<u8>> {
        self.check_cancel()?;
        let mut payload = vec![0xd4, cmd];
        payload.extend(args);
        self.port.write_all(&encode(&payload)?)?;
        self.port.flush()?;
        let deadline = Instant::now() + timeout;
        let mut acknowledged = false;
        loop {
            self.check_cancel()?;
            ensure!(Instant::now() < deadline, "PN532 指令 {cmd:02X} 超时");
            while let Some(frame) = decode(&mut self.buffer)? {
                match frame {
                    Frame::Ack => acknowledged = true,
                    Frame::Data(data) => {
                        ensure!(acknowledged, "响应缺少 ACK");
                        ensure!(
                            data.len() >= 2 && data[0] == 0xd5 && data[1] == cmd.wrapping_add(1),
                            "响应指令不匹配"
                        );
                        return Ok(data[2..].to_vec());
                    }
                }
            }
            let mut temp = [0u8; 512];
            match self.port.read(&mut temp) {
                Ok(n) => self.buffer.extend_from_slice(&temp[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
    pub fn firmware(&mut self) -> Result<String> {
        let d = self.command(2, &[], Duration::from_secs(2))?;
        ensure!(d.len() == 4, "固件响应错误");
        Ok(format!(
            "PN5{:02X} · firmware {}.{} · support {:02X}",
            d[0], d[1], d[2], d[3]
        ))
    }
    pub fn select(&mut self, expected: Option<&str>) -> Result<Card> {
        self.command(0x32, &[1, 0], Duration::from_secs(2))?;
        std::thread::sleep(Duration::from_millis(10));
        self.command(0x32, &[1, 1], Duration::from_secs(2))?;
        let data = self.command(0x4a, &[1, 0], Duration::from_secs(3))?;
        ensure!(data.len() >= 2, "卡片响应过短");
        self.target = data[1];
        let card = parse_target(&data)?;
        if let Some(uid) = expected {
            ensure!(
                uid.eq_ignore_ascii_case(&card.uid),
                "卡片已更换，请重新识别目标"
            );
        }
        self.card = Some(card.clone());
        Ok(card)
    }
    pub fn exchange(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        let mut args = vec![self.target];
        args.extend(data);
        let response = self.command(0x40, &args, Duration::from_secs(3))?;
        let status = *response.first().context("缺少卡片状态")?;
        ensure!(status == 0, "卡片操作失败，状态 0x{status:02X}");
        Ok(response[1..].to_vec())
    }
    pub fn auth(&mut self, b: usize, kind: &str, key: &str) -> Result<()> {
        ensure!(b < 256 && ["A", "B"].contains(&kind), "认证参数错误");
        let uid = document::bytes(&self.card.as_ref().context("尚未识卡")?.uid, None)?;
        let mut cmd = vec![if kind == "A" { 0x60 } else { 0x61 }, b as u8];
        cmd.extend(document::bytes(key, Some(6))?);
        cmd.extend(&uid[uid.len() - 4..]);
        self.exchange(&cmd)?;
        Ok(())
    }
    pub fn read_block(&mut self, b: usize) -> Result<Vec<u8>> {
        ensure!(b < 256, "块号越界");
        let d = self.exchange(&[0x30, b as u8])?;
        ensure!(d.len() == 16, "块长度不为 16");
        Ok(d)
    }
    pub fn read_classic(
        &mut self,
        keys: &[String],
        blocks: usize,
        mut progress: impl FnMut(String),
    ) -> Result<Document> {
        document::sectors(blocks)?;
        ensure!(!keys.is_empty(), "密钥为空");
        for k in keys {
            document::bytes(k, Some(6))?;
        }
        let card = self.select(None)?;
        ensure!(blocks <= capacity(&card)?, "容量超过目标卡");
        let mut doc = Document::empty(blocks)?;
        doc.bind(&card);
        doc.notes.insert("source".into(), "live".into());
        for s in 0..document::sectors(blocks)? {
            let range = document::sector_blocks(s)?;
            let mut found = BTreeMap::new();
            for kind in ["A", "B"] {
                for key in keys {
                    self.check_cancel()?;
                    self.select(Some(&card.uid))?;
                    if self.auth(range.start, kind, key).is_err() {
                        self.check_cancel()?;
                        continue;
                    }
                    found.insert(kind.into(), key.clone());
                    for b in range.clone() {
                        if doc.blocks[b].is_some() {
                            continue;
                        }
                        match self.read_block(b) {
                            Ok(data) => doc.blocks[b] = Some(hex::encode_upper(data)),
                            Err(_) => {
                                self.check_cancel()?;
                                self.select(Some(&card.uid))?;
                                if self.auth(range.start, kind, key).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    break;
                }
            }
            if let Some(t) = doc.blocks[range.end - 1].as_mut() {
                let mut raw = document::bytes(t, Some(16))?;
                for (k, off) in [("A", 0), ("B", 10)] {
                    if let Some(key) = found.get(k) {
                        raw[off..off + 6].copy_from_slice(&document::bytes(key, Some(6))?);
                    }
                }
                *t = hex::encode_upper(raw);
            }
            progress(format!(
                "扇区 {s:02}: {}/{} 块；密钥 {}",
                range.clone().filter(|b| doc.blocks[*b].is_some()).count(),
                range.len(),
                found.keys().cloned().collect::<Vec<String>>().join("/")
            ));
            doc.keys.insert(s.to_string(), found);
        }
        Ok(doc)
    }
    pub fn write_classic(
        &mut self,
        doc: &Document,
        indices: &[usize],
        expected: &str,
        keys: &[String],
        trailers: bool,
        mut progress: impl FnMut(String),
    ) -> Result<usize> {
        ensure!(!expected.is_empty(), "缺少目标 UID");
        let card = self.select(Some(expected))?;
        validate_write(doc, indices, capacity(&card)?, trailers)?;
        ensure!(!keys.is_empty(), "目标认证密钥为空");
        for k in keys {
            document::bytes(k, Some(6))?;
        }
        let mut indices = indices.to_vec();
        indices.sort_unstable();
        indices.dedup();
        let mut count = 0;
        for b in indices {
            self.check_cancel()?;
            let mut authenticated = false;
            'auth: for kind in ["A", "B"] {
                for key in keys {
                    self.select(Some(expected))?;
                    if self.auth(b, kind, key).is_ok() {
                        authenticated = true;
                        break 'auth;
                    }
                }
            }
            ensure!(authenticated, "块 {b} 认证失败，已有 {count} 块完成");
            let data = document::bytes(doc.blocks[b].as_deref().unwrap_or(""), Some(16))?;
            let mut cmd = vec![0xa0, b as u8];
            cmd.extend(&data);
            self.exchange(&cmd)?;
            if document::is_trailer(b) {
                self.select(Some(expected))?;
                self.auth(b, "A", &hex::encode(&data[..6]))?;
                ensure!(
                    self.read_block(b)?[6..10] == data[6..10],
                    "尾块 {b} 回读不符"
                );
            } else {
                ensure!(self.read_block(b)? == data, "块 {b} 回读不符");
            }
            count += 1;
            progress(format!("块 {b} 已写入并回读校验"));
        }
        Ok(count)
    }
    pub fn ntag_read(&mut self, pages: usize, password: &str) -> Result<(Card, Vec<u8>)> {
        ensure!((4..=256).contains(&pages), "页数范围 4–256");
        let card = self.select(None)?;
        ensure!(card.sak == "00", "不是 NTAG/Ultralight");
        self.password(password)?;
        let mut data = Vec::new();
        for p in (0..pages).step_by(4) {
            self.check_cancel()?;
            data.extend(self.read_block(p)?);
        }
        data.truncate(pages * 4);
        Ok((card, data))
    }
    fn password(&mut self, password: &str) -> Result<()> {
        if !password.trim().is_empty() {
            let mut cmd = vec![0x1b];
            cmd.extend(document::bytes(password, Some(4))?);
            ensure!(self.exchange(&cmd)?.len() == 2, "PWD_AUTH 响应错误");
        }
        Ok(())
    }
    pub fn ntag_write(
        &mut self,
        start: usize,
        data: &[u8],
        uid: &str,
        password: &str,
        mut progress: impl FnMut(String),
    ) -> Result<usize> {
        ensure!(!uid.is_empty() && !data.is_empty(), "目标 UID 或数据为空");
        let card = self.select(Some(uid))?;
        ensure!(card.sak == "00", "不是 NTAG");
        self.password(password)?;
        let v = self.exchange(&[0x60])?;
        ensure!(
            v.len() == 8 && v[1] == 4 && v[2] == 4,
            "未识别为 NXP NTAG21x"
        );
        let last = match v[6] {
            0x0f => 39,
            0x11 => 129,
            0x13 => 225,
            _ => bail!("未知 NTAG 容量"),
        };
        let pages = data.len().div_ceil(4);
        ensure!(
            start >= 4 && start.checked_add(pages).is_some_and(|end| end <= last + 1),
            "写入超出用户区"
        );
        for (i, chunk) in data.chunks(4).enumerate() {
            self.check_cancel()?;
            let page = start + i;
            let mut cmd = vec![0xa2, page as u8];
            cmd.extend(chunk);
            cmd.resize(6, 0);
            self.exchange(&cmd)?;
            ensure!(
                self.read_block(page)?[..4] == cmd[2..],
                "页 {page} 回读失败"
            );
            progress(format!("页 {page} 写入并验证"));
        }
        Ok(pages)
    }
}
pub fn parse_target(d: &[u8]) -> Result<Card> {
    ensure!(d.len() >= 6 && d[0] == 1, "未检测到 ISO14443A 卡片");
    let n = d[5] as usize;
    ensure!(
        [4, 7, 10].contains(&n) && d.len() >= 6 + n,
        "卡片响应长度错误"
    );
    Ok(Card {
        uid: hex::encode_upper(&d[6..6 + n]),
        atqa: hex::encode_upper(&d[2..4]),
        sak: format!("{:02X}", d[4]),
    })
}
pub fn capacity(card: &Card) -> Result<usize> {
    match card.sak.as_str() {
        "09" => Ok(20),
        "08" => Ok(64),
        "10" | "11" => Ok(128),
        "18" => Ok(256),
        _ => bail!("不是支持的 Classic 兼容卡"),
    }
}
pub fn validate_write(
    doc: &Document,
    indices: &[usize],
    capacity: usize,
    trailers: bool,
) -> Result<()> {
    doc.validate()?;
    ensure!(!indices.is_empty(), "没有选中块");
    for &b in indices {
        ensure!(
            b > 0 && b < capacity && b < doc.blocks.len(),
            "块 {b} 禁止写入或越界"
        );
        let data = document::bytes(doc.blocks[b].as_deref().context("存在未读块")?, Some(16))?;
        if document::is_trailer(b) {
            ensure!(trailers, "未允许密钥/访问位写入");
            ensure!(
                !doc.live() || doc.trailer_keys_known(document::sector_of(b)?),
                "不能写入隐藏的未知密钥"
            );
            document::access(&data)?;
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frames_fragmented() {
        for n in [2, 254, 255, 1024] {
            let payload = vec![0xa5; n];
            let encoded = encode(&payload).unwrap();
            let mut pending = vec![];
            for (i, b) in encoded.iter().enumerate() {
                pending.push(*b);
                let result = decode(&mut pending).unwrap();
                if i + 1 == encoded.len() {
                    assert_eq!(result, Some(Frame::Data(payload.clone())));
                } else {
                    assert_eq!(result, None);
                }
            }
        }
    }
    #[test]
    fn bad_checksum() {
        let mut frame = encode(&[0xd5, 3, 0x32]).unwrap();
        frame[6] ^= 1;
        assert!(decode(&mut frame).is_err());
    }
    #[test]
    fn targets() {
        let d = [1, 1, 0, 4, 8, 4, 1, 2, 3, 4];
        assert_eq!(parse_target(&d).unwrap().uid, "01020304");
        assert!(parse_target(&d[..8]).is_err());
    }
    #[test]
    fn preflight() {
        let doc = Document::blank(64).unwrap();
        assert!(validate_write(&doc, &[1, 0], 64, false).is_err());
        assert!(validate_write(&doc, &[1, 64], 64, false).is_err());
        assert!(validate_write(&doc, &[1, 3], 64, false).is_err());
        assert!(validate_write(&doc, &[1, 3], 64, true).is_ok());
    }
}

#[cfg(test)]
mod hardware_tests {
    use super::*;
    #[test]
    #[ignore = "Requires an authorized card and PCR532_TEST_PORT"]
    fn hardware_cancel_releases_device() {
        let port = std::env::var("PCR532_TEST_PORT").expect("set PCR532_TEST_PORT");
        let cancel = Arc::new(AtomicBool::new(false));
        let mut reader = Reader::open(&port, 115200, cancel.clone()).unwrap();
        reader.select(None).unwrap();
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            cancel.store(true, Ordering::Relaxed);
        });
        let start = Instant::now();
        let result = reader.read_classic(&["FFFFFFFFFFFF".into()], 64, |_| {});
        stopper.join().unwrap();
        assert!(result.unwrap_err().to_string().contains("任务已停止"));
        assert!(start.elapsed() < Duration::from_secs(3));
        drop(reader);
        let mut reopened = Reader::open(&port, 115200, Arc::new(AtomicBool::new(false))).unwrap();
        assert!(reopened.firmware().unwrap().starts_with("PN532"));
    }
}
