use serde::{Deserialize, Serialize};
use splitdesk_core::Error;
use std::sync::LazyLock;
use std::time::Instant;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Stage {
    T0ClientInput = 0,
    T1InputSent = 1,
    T2ServerInputReceived = 2,
    T3InputInjected = 3,
    T4CompositorFrame = 4,
    T5Captured = 5,
    T6Converted = 6,
    T7Encoded = 7,
    T8PacketSent = 8,
    T9PacketReceived = 9,
    T10Decoded = 10,
    T11Composed = 11,
    T12Presented = 12,
}

impl Stage {
    pub const ALL: [Stage; 13] = [
        Stage::T0ClientInput,
        Stage::T1InputSent,
        Stage::T2ServerInputReceived,
        Stage::T3InputInjected,
        Stage::T4CompositorFrame,
        Stage::T5Captured,
        Stage::T6Converted,
        Stage::T7Encoded,
        Stage::T8PacketSent,
        Stage::T9PacketReceived,
        Stage::T10Decoded,
        Stage::T11Composed,
        Stage::T12Presented,
    ];

    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Stage::T0ClientInput => "T0ClientInput",
            Stage::T1InputSent => "T1InputSent",
            Stage::T2ServerInputReceived => "T2ServerInputReceived",
            Stage::T3InputInjected => "T3InputInjected",
            Stage::T4CompositorFrame => "T4CompositorFrame",
            Stage::T5Captured => "T5Captured",
            Stage::T6Converted => "T6Converted",
            Stage::T7Encoded => "T7Encoded",
            Stage::T8PacketSent => "T8PacketSent",
            Stage::T9PacketReceived => "T9PacketReceived",
            Stage::T10Decoded => "T10Decoded",
            Stage::T11Composed => "T11Composed",
            Stage::T12Presented => "T12Presented",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClockDomain {
    ClientLocal,
    ServerLocal,
    NetworkRtt,
    EstimatedOneWay,
}

impl ClockDomain {
    pub const fn is_estimate(self) -> bool {
        matches!(self, ClockDomain::EstimatedOneWay)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timestamp {
    pub ns: u64,
    pub domain: ClockDomain,
}

impl Timestamp {
    pub fn now_server() -> Self {
        Self {
            ns: monotonic_ns(),
            domain: ClockDomain::ServerLocal,
        }
    }

    pub fn client(ns: u64) -> Self {
        Self {
            ns,
            domain: ClockDomain::ClientLocal,
        }
    }

    pub fn server(ns: u64) -> Self {
        Self {
            ns,
            domain: ClockDomain::ServerLocal,
        }
    }

    pub fn duration_to(
        self,
        end: Timestamp,
        offset_ns: Option<i64>,
    ) -> Result<(u64, ClockDomain, bool), Error> {
        if self.domain == end.domain {
            let estimated = self.domain.is_estimate();
            return Ok((end.ns.saturating_sub(self.ns), self.domain, estimated));
        }
        match (self.domain, end.domain) {
            (ClockDomain::ClientLocal, ClockDomain::ServerLocal)
            | (ClockDomain::ServerLocal, ClockDomain::ClientLocal) => {
                let offset = offset_ns.ok_or(Error::Unsupported)?;
                let start_in_end = if self.domain == ClockDomain::ClientLocal {
                    (self.ns as i128) + (offset as i128)
                } else {
                    (self.ns as i128) - (offset as i128)
                };
                let delta = (end.ns as i128) - start_in_end;
                Ok((delta.max(0) as u64, ClockDomain::EstimatedOneWay, true))
            }
            _ => Err(Error::Unsupported),
        }
    }
}

static START: LazyLock<Instant> = LazyLock::new(Instant::now);

fn monotonic_ns() -> u64 {
    Instant::now().saturating_duration_since(*START).as_nanos() as u64
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LatencySample {
    pub from: Stage,
    pub to: Stage,
    pub duration_ns: u64,
    pub domain: ClockDomain,
    pub estimated: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LatencyStats {
    pub mean: f64,
    pub median: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
    pub n: u64,
}

impl LatencyStats {
    pub fn empty() -> Self {
        Self {
            mean: 0.0,
            median: 0.0,
            p95: 0.0,
            p99: 0.0,
            max: 0.0,
            n: 0,
        }
    }

    pub fn from_samples(samples: &[f64]) -> Self {
        if samples.is_empty() {
            return Self::empty();
        }
        let mut sorted = samples.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = sorted.len() as u64;
        let sum: f64 = sorted.iter().copied().sum();
        Self {
            mean: sum / n as f64,
            median: median(&sorted),
            p95: percentile(&sorted, 95.0),
            p99: percentile(&sorted, 99.0),
            max: sorted[sorted.len() - 1],
            n,
        }
    }
}

fn median(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let n = sorted.len();
    let rank = (p / 100.0 * n as f64).ceil() as usize;
    let idx = rank.saturating_sub(1).min(n - 1);
    sorted[idx]
}

#[derive(Clone, Debug)]
pub struct LatencyWindow {
    samples: Vec<f64>,
    cap: usize,
}

impl LatencyWindow {
    pub fn new(cap: usize) -> Self {
        let cap = cap.max(1);
        Self {
            samples: Vec::with_capacity(cap.min(64)),
            cap,
        }
    }

    pub fn push(&mut self, sample: f64) {
        if self.samples.len() == self.cap {
            self.samples.remove(0);
        }
        self.samples.push(sample);
    }

    pub fn stats(&self) -> LatencyStats {
        LatencyStats::from_samples(&self.samples)
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct StageMarks {
    marks: [Option<Timestamp>; 13],
}

impl Default for StageMarks {
    fn default() -> Self {
        Self::new()
    }
}

impl StageMarks {
    pub fn new() -> Self {
        Self { marks: [None; 13] }
    }

    pub fn mark(&mut self, stage: Stage, ts: Timestamp) {
        self.marks[stage.index()] = Some(ts);
    }

    pub fn get(&self, stage: Stage) -> Option<Timestamp> {
        self.marks[stage.index()]
    }

    pub fn span(
        &self,
        from: Stage,
        to: Stage,
        offset_ns: Option<i64>,
    ) -> Result<LatencySample, Error> {
        let start = self.get(from).ok_or(Error::Unsupported)?;
        let end = self.get(to).ok_or(Error::Unsupported)?;
        let (duration_ns, domain, estimated) = start.duration_to(end, offset_ns)?;
        Ok(LatencySample {
            from,
            to,
            duration_ns,
            domain,
            estimated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thirteen_stages_t0_through_t12() {
        assert_eq!(Stage::ALL.len(), 13);
        assert_eq!(Stage::T0ClientInput.index(), 0);
        assert_eq!(Stage::T12Presented.index(), 12);
        for (i, stage) in Stage::ALL.iter().enumerate() {
            assert_eq!(stage.index(), i);
        }
    }

    #[test]
    fn stats_p50_p95() {
        let samples: Vec<f64> = (1..=10).map(|n| n as f64).collect();
        let stats = LatencyStats::from_samples(&samples);
        assert_eq!(stats.n, 10);
        assert!((stats.mean - 5.5).abs() < 1e-9);
        assert!((stats.median - 5.5).abs() < 1e-9);
        assert!((stats.p95 - 10.0).abs() < 1e-9);
        assert!((stats.p99 - 10.0).abs() < 1e-9);
        assert!((stats.max - 10.0).abs() < 1e-9);

        let mut window = LatencyWindow::new(4);
        for sample in [1.0, 2.0, 3.0, 4.0, 100.0] {
            window.push(sample);
        }
        assert_eq!(window.len(), 4);
        let bounded = window.stats();
        assert_eq!(bounded.n, 4);
        assert!((bounded.max - 100.0).abs() < 1e-9);
        assert!((bounded.median - 3.5).abs() < 1e-9);
    }

    #[test]
    fn refuses_cross_domain_subtract_without_offset() {
        let start = Timestamp::client(1_000);
        let end = Timestamp::server(2_000);
        assert!(matches!(
            start.duration_to(end, None),
            Err(Error::Unsupported)
        ));
        let (ns, domain, estimated) = start.duration_to(end, Some(500)).unwrap();
        assert!(estimated);
        assert_eq!(domain, ClockDomain::EstimatedOneWay);
        assert_eq!(ns, 500);

        let same = Timestamp::server(10)
            .duration_to(Timestamp::server(40), None)
            .unwrap();
        assert_eq!(same, (30, ClockDomain::ServerLocal, false));
    }

    #[test]
    fn stage_marks_span() {
        let mut marks = StageMarks::new();
        marks.mark(Stage::T0ClientInput, Timestamp::client(0));
        marks.mark(Stage::T12Presented, Timestamp::client(1_000));
        let sample = marks
            .span(Stage::T0ClientInput, Stage::T12Presented, None)
            .unwrap();
        assert_eq!(sample.duration_ns, 1_000);
        assert!(!sample.estimated);
        assert_eq!(sample.domain, ClockDomain::ClientLocal);
    }
}
