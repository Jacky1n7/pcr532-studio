// SPDX-License-Identifier: GPL-2.0-or-later
// Rust adaptation of crypto1.c / crapto1.c, Copyright (C) 2008–2014
// bla <blapost@gmail.com>, nfc-tools/mfoc-hardnested a6007437405a.
// Safe vector-based search replaces C pointer tables. See THIRD_PARTY.md.
use anyhow::{Result, ensure};
use std::sync::atomic::{AtomicBool, Ordering};
const O: u32 = 0x29ce5c;
const E: u32 = 0x870804;
fn parity(v: u32) -> u32 {
    v.count_ones() & 1
}
pub fn odd_parity(v: u8) -> u8 {
    (1 ^ parity(v.into())) as u8
}
pub fn filter(x: u32) -> u32 {
    let f = ((0xf22c0 >> (x & 15)) & 16)
        | ((0x6c9c0 >> ((x >> 4) & 15)) & 8)
        | ((0x3c8b0 >> ((x >> 8) & 15)) & 4)
        | ((0x1e458 >> ((x >> 12) & 15)) & 2)
        | ((0x0d938 >> ((x >> 16) & 15)) & 1);
    (0xec57e80au32 >> f) & 1
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crypto1 {
    pub odd: u32,
    pub even: u32,
}
impl Crypto1 {
    pub fn new(key: u64) -> Self {
        let mut s = Self { odd: 0, even: 0 };
        for i in (1..48).step_by(2).rev() {
            s.odd = (s.odd << 1) | ((key >> ((i - 1) ^ 7)) & 1) as u32;
            s.even = (s.even << 1) | ((key >> (i ^ 7)) & 1) as u32;
        }
        s
    }
    pub fn key(self) -> u64 {
        let mut key = 0;
        for i in (0..24).rev() {
            key = (key << 1) | u64::from((self.odd >> (i ^ 3)) & 1);
            key = (key << 1) | u64::from((self.even >> (i ^ 3)) & 1);
        }
        key
    }
    pub fn bit(&mut self, input: u32, encrypted: bool) -> u32 {
        let output = filter(self.odd);
        let feedback =
            (input & 1) ^ (output & u32::from(encrypted)) ^ (O & self.odd) ^ (E & self.even);
        self.even = (self.even << 1) | parity(feedback);
        std::mem::swap(&mut self.odd, &mut self.even);
        output
    }
    pub fn byte(&mut self, input: u8, encrypted: bool) -> u8 {
        let mut output = 0;
        for i in 0..8 {
            output |= (self.bit(u32::from(input >> i), encrypted) as u8) << i;
        }
        output
    }
    pub fn word(&mut self, input: u32, encrypted: bool) -> u32 {
        let mut output = 0;
        for i in 0..32 {
            output |= self.bit(input >> (i ^ 24), encrypted) << (i ^ 24);
        }
        output
    }
    pub fn rollback_word(&mut self, input: u32, encrypted: bool) {
        for i in (0..32).rev() {
            self.odd &= 0xffffff;
            std::mem::swap(&mut self.odd, &mut self.even);
            let mut feedback = self.even & 1;
            self.even >>= 1;
            feedback ^= (E & self.even)
                ^ (O & self.odd)
                ^ ((input >> (i ^ 24)) & 1)
                ^ (filter(self.odd) & u32::from(encrypted));
            self.even |= parity(feedback) << 23;
        }
    }
}
pub fn successor(nonce: u32, steps: usize) -> u32 {
    let mut x = nonce.swap_bytes();
    for _ in 0..steps {
        x = (x >> 1) | ((x >> 16 ^ x >> 18 ^ x >> 19 ^ x >> 21) << 31);
    }
    x.swap_bytes()
}
pub fn weak_nonce(nonce: u32) -> bool {
    let mut state = ((nonce >> 16) as u16).swap_bytes();
    if state == 0 {
        return false;
    }
    for _ in 0..16 {
        state = (state >> 1) | ((state ^ (state >> 2) ^ (state >> 3) ^ (state >> 5)) << 15);
    }
    u32::from(state.swap_bytes()) == nonce & 0xffff
}
pub fn distance(from: u32, to: u32) -> Option<usize> {
    let mut n = from;
    for i in 0..65535 {
        if n == to {
            return Some(i);
        }
        n = successor(n, 1);
    }
    None
}
fn extend(table: Vec<u32>, bit: u32, contribution: Option<(u32, u32, u32)>) -> Vec<u32> {
    let mut out = Vec::with_capacity(table.len());
    for x in table {
        for mut v in [x << 1, (x << 1) | 1] {
            if filter(v) != bit {
                continue;
            }
            if let Some((m1, m2, input)) = contribution {
                let mut p = v >> 25;
                p = (p << 1) | parity(v & m1);
                p = (p << 1) | parity(v & m2);
                v = ((p << 24) | (v & 0xffffff)) ^ (input << 24);
            }
            out.push(v);
        }
    }
    out
}
struct Search<'a> {
    cancel: &'a AtomicBool,
    states: Vec<Crypto1>,
}
impl Search<'_> {
    fn recover(
        &mut self,
        mut odd: Vec<u32>,
        mut even: Vec<u32>,
        mut streams: [u32; 2],
        mut remaining: i32,
        mut input: u32,
    ) -> Result<()> {
        ensure!(!self.cancel.load(Ordering::Relaxed), "任务已停止");
        if remaining == -1 {
            ensure!(
                self.states
                    .len()
                    .saturating_add(odd.len().saturating_mul(even.len()))
                    <= 1_000_000,
                "恢复候选过多"
            );
            for e in even {
                let e = (e << 1) ^ parity(e & E) ^ u32::from(input & 4 != 0);
                for o in &odd {
                    self.states.push(Crypto1 {
                        even: *o,
                        odd: e ^ parity(*o & O),
                    });
                }
            }
            return Ok(());
        }
        for _ in 0..4 {
            let before = remaining;
            remaining -= 1;
            if before == 0 {
                break;
            }
            streams[0] >>= 1;
            streams[1] >>= 1;
            input >>= 2;
            odd = extend(odd, streams[0] & 1, Some(((E << 1) | 1, O << 1, 0)));
            even = extend(even, streams[1] & 1, Some((O, (E << 1) | 1, input & 3)));
            if odd.is_empty() || even.is_empty() {
                return Ok(());
            }
        }
        let mut ob: [Vec<u32>; 256] = std::array::from_fn(|_| Vec::new());
        let mut eb: [Vec<u32>; 256] = std::array::from_fn(|_| Vec::new());
        for o in odd {
            ob[(o >> 24) as usize].push(o);
        }
        for e in even {
            eb[(e >> 24) as usize].push(e);
        }
        for (o, e) in ob.into_iter().zip(eb) {
            if !o.is_empty() && !e.is_empty() {
                self.recover(o, e, streams, remaining, input)?;
            }
        }
        Ok(())
    }
}
/// Recover all compatible states after consuming a known 32-bit keystream.
pub fn recover32(keystream: u32, input: u32, cancel: &AtomicBool) -> Result<Vec<Crypto1>> {
    let mut streams = [0u32; 2];
    for i in (1..32).step_by(2).rev() {
        streams[0] = (streams[0] << 1) | ((keystream >> (i ^ 24)) & 1);
    }
    for i in (0..32).step_by(2).rev() {
        streams[1] = (streams[1] << 1) | ((keystream >> (i ^ 24)) & 1);
    }
    let mut odd = Vec::new();
    let mut even = Vec::new();
    for i in (0..=1 << 20).rev() {
        if i & 0x3fff == 0 {
            ensure!(!cancel.load(Ordering::Relaxed), "任务已停止");
        }
        let f = filter(i);
        if f == streams[0] & 1 {
            odd.push(i);
        }
        if f == streams[1] & 1 {
            even.push(i);
        }
    }
    for _ in 0..4 {
        streams[0] >>= 1;
        streams[1] >>= 1;
        odd = extend(odd, streams[0] & 1, None);
        even = extend(even, streams[1] & 1, None);
    }
    let input = ((input >> 16) & 0xff) | (input << 16) | (input & 0xff00);
    let mut search = Search {
        cancel,
        states: Vec::new(),
    };
    search.recover(odd, even, streams, 11, input << 1)?;
    Ok(search.states)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upstream_keystream_vectors() {
        // Compared with upstream crypto1.c; no C code is used by this test or application.
        for (key, expected) in [
            (0xa0a1a2a3a4a5, 0x30794609),
            (0xffffffffffff, 0xff9f1c66),
            (0, 0x902d4280),
            (0x123456789abc, 0xe2ca3aee),
        ] {
            assert_eq!(Crypto1::new(key).word(0x12345678, false), expected);
        }
    }
    #[test]
    fn forward_reverse_and_recovery() {
        let key = 0xa0a1a2a3a4a5;
        let input = 0x12345678;
        let mut s = Crypto1::new(key);
        assert_eq!(s.key(), key);
        let ks = s.word(input, false);
        s.rollback_word(input, false);
        assert_eq!(s.key(), key);
        let states = recover32(ks, input, &AtomicBool::new(false)).unwrap();
        assert!(states.into_iter().any(|mut s| {
            s.rollback_word(input, false);
            s.key() == key
        }));
    }
    #[test]
    fn cancellation_and_nonce_classification() {
        assert!(recover32(0, 0, &AtomicBool::new(true)).is_err());
        assert!(!weak_nonce(0));
        assert!(!weak_nonce(0x12345678));
        assert!(weak_nonce(0x01000168));
        assert_eq!(distance(0x01000168, successor(0x01000168, 96)), Some(96));
    }
}
