//! 远程桌面画面的纯逻辑：datagram 分片按 frame id 拼回整帧，丢帧检测，统计。
//! 网络和解码线程在 [`crate::desk`]（只有 macOS 有）。

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use lolidesk_proto::{FRAG_KEY, Frag};

/// 一帧拼齐了。`gap` 表示它和上一帧之间丢了帧（解码要等关键帧）。
#[derive(Debug, PartialEq, Eq)]
pub struct Assembled {
    pub frame: u32,
    pub data: Vec<u8>,
    pub key: bool,
    pub gap: bool,
}

/// 拼帧中的一帧。
struct Pending {
    parts: Vec<Option<Vec<u8>>>,
    have: usize,
    key: bool,
}

/// 按 frame id 拼帧。比已经交出去的旧的分片（迟到的）丢掉；落后最新分片 4 帧还没拼齐的帧丢掉。
#[derive(Default)]
pub struct Reassembler {
    pending: HashMap<u32, Pending>,
    last_done: Option<u32>,
    /// 没拼齐就丢掉的帧数
    pub incomplete: u64,
}

/// `a` 比 `b` 新（frame id 会回绕）。
fn newer(a: u32, b: u32) -> bool {
    a != b && a.wrapping_sub(b) < u32::MAX / 2
}

impl Reassembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// 收一个分片；凑齐一帧时返回它。
    pub fn push(&mut self, frag: Frag, body: &[u8]) -> Option<Assembled> {
        if frag.count == 0 {
            return None;
        }
        if let Some(last) = self.last_done
            && !newer(frag.frame, last)
        {
            return None;
        }
        let entry = self.pending.entry(frag.frame).or_insert_with(|| Pending {
            parts: vec![None; frag.count as usize],
            have: 0,
            key: frag.flags & FRAG_KEY != 0,
        });
        let i = frag.index as usize;
        if i < entry.parts.len() && entry.parts[i].is_none() {
            entry.parts[i] = Some(body.to_vec());
            entry.have += 1;
        }
        if entry.have < entry.parts.len() {
            // 太旧还没齐的不要了
            let before = self.pending.len();
            self.pending
                .retain(|id, _| frag.frame.wrapping_sub(*id) < 4 || newer(*id, frag.frame));
            self.incomplete += (before - self.pending.len()) as u64;
            return None;
        }
        let done = self.pending.remove(&frag.frame)?;
        // 比这一帧旧、还没拼齐的都不要了
        let before = self.pending.len();
        self.pending.retain(|id, _| newer(*id, frag.frame));
        self.incomplete += (before - self.pending.len()) as u64;

        let gap = self
            .last_done
            .is_some_and(|l| frag.frame != l.wrapping_add(1));
        self.last_done = Some(frag.frame);
        let mut data = Vec::with_capacity(done.parts.iter().flatten().map(Vec::len).sum());
        for p in done.parts.into_iter().flatten() {
            data.extend_from_slice(&p);
        }
        Some(Assembled {
            frame: frag.frame,
            data,
            key: done.key,
            gap,
        })
    }
}

/// 解码这边的关键帧请求节流：缺关键帧时最多每 250 ms 要一次。
pub struct KeyframeGate {
    last: Option<Instant>,
    every: Duration,
}

impl Default for KeyframeGate {
    fn default() -> Self {
        Self {
            last: None,
            every: Duration::from_millis(250),
        }
    }
}

impl KeyframeGate {
    /// 现在该不该再要一次。
    pub fn should_request(&mut self, now: Instant) -> bool {
        if self
            .last
            .is_some_and(|t| now.duration_since(t) < self.every)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

/// 累计的计数。
#[derive(Debug, Default, Clone)]
pub struct Counters {
    /// 拼齐的帧
    pub frames: u64,
    /// 拼齐的帧的字节数
    pub bytes: u64,
    /// 解出画面的帧
    pub decoded: u64,
    /// 没拼齐丢掉的帧
    pub incomplete: u64,
    /// 解码跟不上丢掉的帧
    pub overrun: u64,
    pub keyframe_requests: u64,
    pub decode_us: u64,
}

/// 工具条上显示的统计（速率按上次取样以来算）。
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct StatsSnapshot {
    pub fps: f64,
    pub mbps: f64,
    pub decode_ms: f64,
    pub dropped: u64,
    pub keyframes: u64,
    pub rtt_ms: f64,
}

/// 把累计计数换成速率。
#[derive(Default)]
pub struct RateMeter {
    last: Option<(Instant, Counters)>,
}

impl RateMeter {
    pub fn sample(&mut self, now: Instant, c: &Counters, rtt_ms: f64) -> StatsSnapshot {
        let mut s = StatsSnapshot {
            dropped: c.incomplete + c.overrun,
            keyframes: c.keyframe_requests,
            rtt_ms,
            ..Default::default()
        };
        if let Some((t, prev)) = &self.last {
            let dt = now.duration_since(*t).as_secs_f64().max(0.001);
            let n = c.decoded.saturating_sub(prev.decoded);
            s.fps = n as f64 / dt;
            s.mbps = c.bytes.saturating_sub(prev.bytes) as f64 * 8.0 / dt / 1e6;
            if n > 0 {
                s.decode_ms = c.decode_us.saturating_sub(prev.decode_us) as f64 / n as f64 / 1000.0;
            }
        }
        self.last = Some((now, c.clone()));
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frag(frame: u32, index: u16, count: u16, key: bool) -> Frag {
        Frag {
            frame,
            index,
            count,
            flags: if key { FRAG_KEY } else { 0 },
        }
    }

    #[test]
    fn in_order_frames() {
        let mut r = Reassembler::new();
        assert_eq!(r.push(frag(1, 0, 2, true), b"ab"), None);
        let a = r.push(frag(1, 1, 2, true), b"cd").unwrap();
        assert_eq!(
            (a.frame, a.data.as_slice(), a.key, a.gap),
            (1, &b"abcd"[..], true, false)
        );
        let b = r.push(frag(2, 0, 1, false), b"x").unwrap();
        assert_eq!((b.key, b.gap), (false, false));
    }

    #[test]
    fn fragments_out_of_order_and_duplicates() {
        let mut r = Reassembler::new();
        assert!(r.push(frag(7, 2, 3, false), b"3").is_none());
        assert!(r.push(frag(7, 2, 3, false), b"3").is_none(), "重复分片不算");
        assert!(r.push(frag(7, 0, 3, false), b"1").is_none());
        let a = r.push(frag(7, 1, 3, false), b"2").unwrap();
        assert_eq!(a.data, b"123");
        assert!(!a.gap, "第一帧不算断");
        // 越界的分片序号忽略
        assert!(r.push(frag(8, 5, 2, false), b"?").is_none());
    }

    #[test]
    fn gap_and_late_fragments() {
        let mut r = Reassembler::new();
        r.push(frag(1, 0, 1, true), b"k").unwrap();
        // 帧 2 只来了一半，帧 3 齐了：2 丢掉、3 带 gap
        assert!(r.push(frag(2, 0, 2, false), b"a").is_none());
        let three = r.push(frag(3, 0, 1, false), b"c").unwrap();
        assert!(three.gap);
        assert_eq!(r.incomplete, 1);
        // 帧 2 的另一半迟到了：不要
        assert!(r.push(frag(2, 1, 2, false), b"b").is_none());
        // 已经交出去的帧的分片再来：不要
        assert!(r.push(frag(3, 0, 1, false), b"c").is_none());
        let four = r.push(frag(4, 0, 1, false), b"d").unwrap();
        assert!(!four.gap);
    }

    #[test]
    fn stale_incomplete_frames_are_dropped() {
        let mut r = Reassembler::new();
        assert!(r.push(frag(10, 0, 2, false), b"a").is_none());
        // 一直没齐，后面的帧也都只来一半：落后 4 帧以上的丢掉
        for f in 11..=14 {
            assert!(r.push(frag(f, 0, 2, false), b"a").is_none());
        }
        assert_eq!(r.incomplete, 1);
        assert_eq!(r.pending.len(), 4);
    }

    #[test]
    fn frame_ids_wrap() {
        let mut r = Reassembler::new();
        r.push(frag(u32::MAX, 0, 1, true), b"a").unwrap();
        let next = r.push(frag(0, 0, 1, false), b"b").unwrap();
        assert!(!next.gap);
        assert!(r.push(frag(u32::MAX - 1, 0, 1, false), b"old").is_none());
    }

    #[test]
    fn keyframe_requests_are_throttled() {
        let mut g = KeyframeGate::default();
        let t = Instant::now();
        assert!(g.should_request(t));
        assert!(!g.should_request(t + Duration::from_millis(100)));
        assert!(g.should_request(t + Duration::from_millis(300)));
    }

    #[test]
    fn rates() {
        let mut m = RateMeter::default();
        let t = Instant::now();
        let mut c = Counters::default();
        let first = m.sample(t, &c, 3.0);
        assert_eq!((first.fps, first.rtt_ms), (0.0, 3.0));
        c.decoded = 60;
        c.bytes = 1_250_000;
        c.decode_us = 60 * 1500;
        c.incomplete = 2;
        c.overrun = 1;
        let s = m.sample(t + Duration::from_secs(1), &c, 3.0);
        assert!((s.fps - 60.0).abs() < 1e-9);
        assert!((s.mbps - 10.0).abs() < 1e-9);
        assert!((s.decode_ms - 1.5).abs() < 1e-9);
        assert_eq!(s.dropped, 3);
    }
}
