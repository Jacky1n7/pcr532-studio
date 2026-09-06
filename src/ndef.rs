//! NDEF record and Type 2 TLV encoding, plus a bounded read-only Type 4 APDU state machine.
use anyhow::{Result, bail, ensure};

pub const KINDS: [&str; 7] = [
    "文本",
    "网址",
    "电话",
    "名片",
    "Wi-Fi",
    "蓝牙",
    "Android 应用",
];
fn attribute(tag: u16, value: &[u8]) -> Result<Vec<u8>> {
    ensure!(value.len() <= u16::MAX as usize, "属性过长");
    let mut out = tag.to_be_bytes().to_vec();
    out.extend((value.len() as u16).to_be_bytes());
    out.extend(value);
    Ok(out)
}
pub fn record(tnf: u8, typ: &[u8], payload: &[u8]) -> Result<Vec<u8>> {
    ensure!(typ.len() <= 255 && payload.len() <= 65500, "NDEF 记录过长");
    let short = payload.len() < 256;
    let mut out = vec![0xc0 | tnf | if short { 0x10 } else { 0 }, typ.len() as u8];
    if short {
        out.push(payload.len() as u8);
    } else {
        out.extend((payload.len() as u32).to_be_bytes());
    }
    out.extend(typ);
    out.extend(payload);
    Ok(out)
}
pub fn generate(kind: &str, text: &str, extra: &str) -> Result<Vec<u8>> {
    let (tnf, typ, payload): (u8, Vec<u8>, Vec<u8>) = match kind {
        "文本" => (
            1,
            b"T".to_vec(),
            [b"\x02en".as_slice(), text.as_bytes()].concat(),
        ),
        "网址" => (
            1,
            b"U".to_vec(),
            [b"\0".as_slice(), text.as_bytes()].concat(),
        ),
        "电话" => (
            1,
            b"U".to_vec(),
            [b"\0tel:".as_slice(), text.as_bytes()].concat(),
        ),
        "名片" => (2, b"text/vcard".to_vec(), text.as_bytes().to_vec()),
        "Android 应用" => {
            ensure!(
                text.contains('.')
                    && text
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_'),
                "请输入应用包名"
            );
            (4, b"android.com:pkg".to_vec(), text.as_bytes().to_vec())
        }
        "蓝牙" => {
            let mac = hex::decode(text.replace(':', ""))?;
            ensure!(mac.len() == 6, "蓝牙地址须为 6 字节");
            ensure!(extra.len() <= 248, "设备名称过长");
            let mut payload = vec![0, 0];
            payload.extend(mac.iter().rev());
            if !extra.is_empty() {
                payload.extend([extra.len() as u8 + 1, 9]);
                payload.extend(extra.as_bytes());
            }
            let len = (payload.len() as u16).to_le_bytes();
            payload[..2].copy_from_slice(&len);
            (2, b"application/vnd.bluetooth.ep.oob".to_vec(), payload)
        }
        "Wi-Fi" => {
            ensure!((1..=32).contains(&text.len()), "SSID 须为 1–32 字节");
            ensure!(
                extra.is_empty() || (8..=63).contains(&extra.len()),
                "密码须为 8–63 字节；留空为开放网络"
            );
            let mut credential = Vec::new();
            for (tag, data) in [
                (0x1026, vec![1]),
                (0x1045, text.as_bytes().to_vec()),
                (
                    0x1003,
                    if extra.is_empty() {
                        vec![0, 1]
                    } else {
                        vec![0, 0x20]
                    },
                ),
                (
                    0x100f,
                    if extra.is_empty() {
                        vec![0, 1]
                    } else {
                        vec![0, 8]
                    },
                ),
                (0x1027, extra.as_bytes().to_vec()),
                (0x1020, vec![255; 6]),
            ] {
                credential.extend(attribute(tag, &data)?);
            }
            let mut payload = attribute(0x100e, &credential)?;
            payload.extend(attribute(0x1049, &[0, 0x37, 0x2a, 0, 1, 0x20])?);
            (2, b"application/vnd.wfa.wsc".to_vec(), payload)
        }
        _ => bail!("未知 NDEF 类型"),
    };
    record(tnf, &typ, &payload)
}
pub fn tlv(message: &[u8]) -> Result<Vec<u8>> {
    ensure!(message.len() <= 65535, "NDEF 过长");
    let mut out = vec![3];
    if message.len() < 255 {
        out.push(message.len() as u8);
    } else {
        out.push(255);
        out.extend((message.len() as u16).to_be_bytes());
    }
    out.extend(message);
    out.push(0xfe);
    Ok(out)
}
pub fn from_tlv(data: &[u8]) -> Result<Vec<u8>> {
    ensure!(data.len() >= 2 && data[0] == 3, "需要 NDEF TLV");
    let (start, n) = if data[1] == 255 {
        ensure!(data.len() >= 4, "长度字段不完整");
        (4, u16::from_be_bytes([data[2], data[3]]) as usize)
    } else {
        (2, data[1] as usize)
    };
    ensure!(data.len() >= start + n, "记录不完整");
    let result = data[start..start + n].to_vec();
    validate(&result)?;
    Ok(result)
}
pub fn validate(message: &[u8]) -> Result<()> {
    ensure!(
        !message.is_empty() && message.len() <= 65532,
        "NDEF 长度错误"
    );
    let mut p = 0;
    let mut first = true;
    let mut ended = false;
    while p < message.len() {
        ensure!(!ended && message.len() - p >= 3, "NDEF 记录头错误");
        let flags = message[p];
        let t = message[p + 1] as usize;
        p += 2;
        ensure!(
            (flags & 0x80 != 0) == first && flags & 0x20 == 0 && flags & 7 != 7,
            "不支持的分块或记录标记"
        );
        let n = if flags & 0x10 != 0 {
            let n = message[p] as usize;
            p += 1;
            n
        } else {
            ensure!(message.len() - p >= 4, "长度不完整");
            let n = u32::from_be_bytes(message[p..p + 4].try_into()?) as usize;
            p += 4;
            n
        };
        let id = if flags & 8 != 0 {
            ensure!(p < message.len(), "ID 长度不完整");
            let n = message[p] as usize;
            p += 1;
            n
        } else {
            0
        };
        p = p
            .checked_add(t)
            .and_then(|p| p.checked_add(id))
            .and_then(|p| p.checked_add(n))
            .ok_or_else(|| anyhow::anyhow!("长度溢出"))?;
        ensure!(p <= message.len(), "NDEF 记录截断");
        ended = flags & 0x40 != 0;
        first = false;
    }
    ensure!(ended, "NDEF 缺少结束标记");
    Ok(())
}
pub struct Type4 {
    message: Vec<u8>,
    selected: u16,
    application: bool,
}
impl Type4 {
    pub fn new(message: Vec<u8>) -> Result<Self> {
        validate(&message)?;
        let mut data = (message.len() as u16).to_be_bytes().to_vec();
        data.extend(message);
        Ok(Self {
            message: data,
            selected: 0,
            application: false,
        })
    }
    pub fn respond(&mut self, apdu: &[u8]) -> Vec<u8> {
        if apdu.len() < 4 || apdu[0] != 0 {
            return vec![0x6e, 0];
        }
        match apdu[1] {
            0xa4 => {
                if apdu.len() < 5 {
                    return vec![0x67, 0];
                }
                let n = apdu[4] as usize;
                if apdu.len() < 5 + n {
                    return vec![0x67, 0];
                }
                let data = &apdu[5..5 + n];
                if apdu[2] == 4 {
                    self.application = data == [0xd2, 0x76, 0, 0, 0x85, 1, 1]
                        || data == [0xd2, 0x76, 0, 0, 0x85, 1, 0];
                    self.selected = 0;
                    if self.application {
                        vec![0x90, 0]
                    } else {
                        vec![0x6a, 0x82]
                    }
                } else if apdu[2] == 0
                    && self.application
                    && (data == [0xe1, 3] || data == [0xe1, 4])
                {
                    self.selected = u16::from_be_bytes([data[0], data[1]]);
                    vec![0x90, 0]
                } else {
                    vec![0x6a, 0x82]
                }
            }
            0xb0 => {
                if apdu.len() != 5 {
                    return vec![0x67, 0];
                }
                let cc = [
                    0, 15, 0x20, 0, 0xff, 0, 0xff, 4, 6, 0xe1, 4, 0xff, 0xfe, 0, 0xff,
                ];
                let data = match self.selected {
                    0xe103 => cc.as_slice(),
                    0xe104 => self.message.as_slice(),
                    _ => return vec![0x69, 0x86],
                };
                let offset = u16::from_be_bytes([apdu[2], apdu[3]]) as usize;
                let n = if apdu[4] == 0 { 256 } else { apdu[4] as usize };
                if offset.checked_add(n).is_none_or(|end| end > data.len()) {
                    return vec![0x6b, 0];
                }
                let mut out = data[offset..offset + n].to_vec();
                out.extend([0x90, 0]);
                out
            }
            0xd6 => vec![0x69, 0x82],
            _ => vec![0x6d, 0],
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixtures() {
        assert_eq!(
            hex::encode_upper(tlv(&generate("文本", "Hi", "").unwrap()).unwrap()),
            "0309D101055402656E4869FE"
        );
        for kind in KINDS {
            let (text, extra) = match kind {
                "Wi-Fi" => ("Test", "12345678"),
                "蓝牙" => ("01:02:03:04:05:06", "Test"),
                "Android 应用" => ("com.example.app", ""),
                _ => ("hello", ""),
            };
            let m = generate(kind, text, extra).unwrap();
            validate(&m).unwrap();
            assert_eq!(from_tlv(&tlv(&m).unwrap()).unwrap(), m);
        }
    }
    #[test]
    fn reject_truncated() {
        for n in 0..9 {
            assert!(validate(&[0xd1, 1, 5, b'T', 2, b'e', b'n', b'H', b'i'][..n]).is_err());
        }
    }
    #[test]
    fn bounded_type4() {
        let mut card = Type4::new(generate("文本", "Hi", "").unwrap()).unwrap();
        assert_eq!(card.respond(&[0, 0xb0, 0, 0, 10]), [0x69, 0x86]);
        assert_eq!(
            card.respond(&[0, 0xa4, 4, 0, 7, 0xd2, 0x76, 0, 0, 0x85, 1, 1]),
            [0x90, 0]
        );
        assert_eq!(card.respond(&[0, 0xa4, 0, 12, 2, 0xe1, 4]), [0x90, 0]);
        assert_eq!(card.respond(&[0, 0xb0, 0xff, 0xff, 255]), [0x6b, 0]);
        assert_eq!(card.respond(&[0, 0xd6, 0, 0, 0]), [0x69, 0x82]);
    }
}
