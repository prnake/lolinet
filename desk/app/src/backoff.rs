//! 重连的退避：1 秒、2 秒、4 秒……最多 60 秒；连上了从头算。

use std::time::{Duration, Instant};

/// 第 `attempt` 次（从 0 数）失败后等多久。
pub fn delay(attempt: u32) -> Duration {
    Duration::from_secs((1u64 << attempt.min(6)).min(60))
}

/// 一台主机的重连状态。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Retry {
    /// 连续失败了几次
    pub attempt: u32,
    /// 下一次什么时候连；None 是不排队
    pub due: Option<Instant>,
}

impl Retry {
    /// 又失败了一次：排下一次，返回要等多久。
    pub fn schedule(&mut self, now: Instant) -> Duration {
        let d = delay(self.attempt);
        self.attempt = self.attempt.saturating_add(1);
        self.due = Some(now + d);
        d
    }

    /// 连上了（或者不再重连）：从头算。
    pub fn reset(&mut self) {
        *self = Retry::default();
    }

    /// 现在就连（窗口回到前台时），次数不清。
    pub fn hurry(&mut self, now: Instant) {
        if self.due.is_some() {
            self.due = Some(now);
        }
    }

    pub fn is_due(&self, now: Instant) -> bool {
        self.due.is_some_and(|d| d <= now)
    }

    /// 离下一次还有几秒（向上取整）。
    pub fn wait_secs(&self, now: Instant) -> Option<u64> {
        let d = self.due?.saturating_duration_since(now);
        Some(d.as_secs() + u64::from(d.subsec_nanos() > 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_doubles_and_caps() {
        let secs: Vec<u64> = (0..9).map(|a| delay(a).as_secs()).collect();
        assert_eq!(secs, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(delay(u32::MAX).as_secs(), 60);
    }

    #[test]
    fn retry_lifecycle() {
        let t = Instant::now();
        let mut r = Retry::default();
        assert!(!r.is_due(t));
        assert_eq!(r.schedule(t), Duration::from_secs(1));
        assert_eq!(r.schedule(t), Duration::from_secs(2));
        assert_eq!(r.schedule(t), Duration::from_secs(4));
        assert_eq!(r.attempt, 3);
        assert!(!r.is_due(t + Duration::from_secs(3)));
        assert!(r.is_due(t + Duration::from_secs(4)));
        assert_eq!(r.wait_secs(t + Duration::from_millis(1500)), Some(3));
        // 回到前台：马上连，次数不清
        r.hurry(t);
        assert!(r.is_due(t));
        assert_eq!(r.attempt, 3);
        r.reset();
        assert_eq!(r, Retry::default());
        // 没排队时 hurry 不排
        r.hurry(t);
        assert!(!r.is_due(t));
    }
}
