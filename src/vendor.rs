//! PCR532 vendor read-only extension. Frames differ from standard PN532 framing.
use anyhow::{Result, bail, ensure};
use std::{
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
fn receive(
    port: &mut dyn serialport::SerialPort,
    cancel: &AtomicBool,
    seconds: u64,
) -> Result<Vec<u8>> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut result = Vec::new();
    let mut buf = [0u8; 256];
    while Instant::now() < deadline {
        ensure!(!cancel.load(Ordering::Relaxed), "任务已停止");
        match port.read(&mut buf) {
            Ok(n) => {
                result.extend(&buf[..n]);
                ensure!(result.len() <= 4096, "厂商响应过长");
            }
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(result)
}
pub fn read(port: &str, baud: u32, hid: bool, cancel: Arc<AtomicBool>) -> Result<String> {
    let mut port = serialport::new(port, baud)
        .timeout(Duration::from_millis(100))
        .open()?;
    port.clear(serialport::ClearBuffer::All)?;
    port.write_all(&hex::decode("0000ff06fad4a55aa55aa55a0000")?)?;
    let reply = receive(&mut *port, &cancel, 1)?;
    let accepted = [
        "0000ff00ff000000ff0af6d5a6aa55aa55aa55235e0700",
        "0000ff00ff000000ff0af6d5a6a55aa55aa55a235e0700",
        "0000ff00ff000000ff0bf5d5a65aa55aa55aa5be715900",
    ];
    ensure!(
        accepted
            .iter()
            .any(|h| hex::decode(h).is_ok_and(|v| reply.windows(v.len()).any(|w| w == v))),
        "未收到 PCR532 厂商扩展握手"
    );
    port.write_all(&[
        0,
        0,
        255,
        3,
        253,
        0xd4,
        if hid { 0xad } else { 0xac },
        0,
        0,
        0,
        0,
    ])?;
    let reply = receive(&mut *port, &cancel, 3)?;
    let prefix = [0, 0, 255, 0, 255, 0, 0, 0, 255];
    let Some(pos) = reply.windows(prefix.len()).position(|w| w == prefix) else {
        bail!("未收到完整低频响应");
    };
    let body = &reply[pos + 6..];
    ensure!(body.len() >= 8, "响应截断");
    let n = body[3] as usize;
    ensure!(
        body[3].wrapping_add(body[4]) == 0 && body.len() >= n + 7,
        "厂商响应长度校验失败"
    );
    let data = &body[5..5 + n];
    ensure!(
        data.len() >= 3 && data[..3] == [0xd5, if hid { 0xae } else { 0xad }, 0xaa],
        "未检测到支持的低频卡"
    );
    Ok(format!(
        "{} 扩展响应（原始字段，具体卡号格式尚待验证）：\n{}",
        if hid { "HID" } else { "ID" },
        hex::encode_upper(data)
    ))
}
