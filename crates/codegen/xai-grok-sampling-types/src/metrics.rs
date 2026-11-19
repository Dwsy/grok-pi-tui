//! Token-level timing captured from streaming inference responses: TTFB, TTLB, and
//! inter-token latency (ITL) statistics.
//!
//! Pure computation over std `Instant`s — no I/O — so the shell can report latency
//! without depending on the sampler runtime.

use std::time::Instant;

use serde::{Deserialize, Serialize};

/// Returns (p50, p99, max, mean, sum) from a slice of sorted values.
/// Panics if `sorted` is empty.
pub fn compute_percentiles(sorted: &[u64]) -> (u64, u64, u64, u64, u64) {
    let len = sorted.len();
    assert!(len > 0, "Cannot compute percentiles from empty slice");

    let p50 = sorted[len / 2];
    let p99_idx = ((len as f64 * 0.99).ceil() as usize)
        .saturating_sub(1)
        .min(len - 1);
    let p99 = sorted[p99_idx];
    let max = sorted[len - 1];
    let sum: u64 = sorted.iter().sum();
    let mean = sum / len as u64;

    (p50, p99, max, mean, sum)
}

/// Per-response inference latency metrics computed from chunk timestamps.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InferenceLatencyStats {
    /// Time to first content token (ms)
    pub time_to_first_token_ms: Option<u64>,
    /// Measured at stream exhaustion, not at the last content chunk, so it includes
    /// trailing metadata chunks.
    pub time_to_last_byte_ms: u64,
    /// Number of content chunks received
    pub chunk_count: u32,
    /// Inter-token latency intervals (raw data for session aggregation)
    pub itl_intervals_ms: Vec<u64>,
    pub itl_p50_ms: Option<u64>,
    pub itl_p99_ms: Option<u64>,
    pub itl_max_ms: Option<u64>,
    pub itl_mean_ms: Option<u64>,
    /// Total request attempts (`1` means no retries); set by the retry loop on success.
    pub attempts: u32,
}

impl InferenceLatencyStats {
    pub fn record_on_span(&self, span: &tracing::Span) {
        if let Some(ttfb) = self.time_to_first_token_ms {
            span.record("ttfb_ms", ttfb);
        }
        span.record("ttlb_ms", self.time_to_last_byte_ms);
        span.record("chunk_count", self.chunk_count);
        if let Some(p50) = self.itl_p50_ms {
            span.record("itl_p50_ms", p50);
        }
        if let Some(p99) = self.itl_p99_ms {
            span.record("itl_p99_ms", p99);
        }
    }

    /// `stream_start` - `Instant::now()` captured before initiating the stream.
    /// `chunk_timestamps` - `Instant` recorded on each content-bearing chunk.
    /// `stream_end` - `Instant::now()` captured after the stream is fully exhausted
    /// (after trailing metadata/`[DONE]` chunks). Used for TTLB.
    pub fn from_timestamps(
        stream_start: Instant,
        chunk_timestamps: &[Instant],
        stream_end: Instant,
    ) -> Self {
        let ttlb = stream_end.duration_since(stream_start).as_millis() as u64;

        if chunk_timestamps.is_empty() {
            return Self {
                time_to_last_byte_ms: ttlb,
                ..Default::default()
            };
        }

        let ttfb = chunk_timestamps[0].duration_since(stream_start);

        let intervals: Vec<u64> = chunk_timestamps
            .windows(2)
            .map(|w| w[1].duration_since(w[0]).as_millis() as u64)
            .collect();

        let (itl_p50, itl_p99, itl_max, itl_mean) = if intervals.is_empty() {
            (None, None, None, None)
        } else {
            let mut sorted = intervals.clone();
            sorted.sort_unstable();
            let (p50, p99, max, mean, _sum) = compute_percentiles(&sorted);
            (Some(p50), Some(p99), Some(max), Some(mean))
        };

        Self {
            time_to_first_token_ms: Some(ttfb.as_millis() as u64),
            time_to_last_byte_ms: ttlb,
            chunk_count: u32::try_from(chunk_timestamps.len()).unwrap_or(u32::MAX),
            itl_intervals_ms: intervals,
            itl_p50_ms: itl_p50,
            itl_p99_ms: itl_p99,
            itl_max_ms: itl_max,
            itl_mean_ms: itl_mean,
            attempts: 0,
        }
    }
}
