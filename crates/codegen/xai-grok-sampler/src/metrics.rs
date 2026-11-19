//! Token-level timing from streaming inference responses.
//!
//! The types and computations moved to `xai_grok_sampling_types::metrics` so the
//! shell can report latency without the sampler runtime; re-exported here for the
//! sampler's own call sites and for this module's tests.

pub use xai_grok_sampling_types::{InferenceLatencyStats, compute_percentiles};

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn offset(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn test_empty_timestamps() {
        let start = Instant::now();
        let end = start + Duration::from_millis(500);

        let stats = InferenceLatencyStats::from_timestamps(start, &[], end);

        assert_eq!(stats.time_to_first_token_ms, None);
        assert_eq!(stats.time_to_last_byte_ms, 500);
        assert_eq!(stats.chunk_count, 0);
        assert_eq!(stats.itl_p50_ms, None);
        assert_eq!(stats.itl_p99_ms, None);
        assert_eq!(stats.itl_max_ms, None);
        assert_eq!(stats.itl_mean_ms, None);
    }

    #[test]
    fn test_single_chunk() {
        let start = Instant::now();
        let chunks = vec![offset(start, 100)];
        let end = offset(start, 200);

        let stats = InferenceLatencyStats::from_timestamps(start, &chunks, end);

        assert_eq!(stats.time_to_first_token_ms, Some(100));
        assert_eq!(stats.time_to_last_byte_ms, 200);
        assert_eq!(stats.chunk_count, 1);
        // One chunk yields no intervals, so there are no ITL stats
        assert_eq!(stats.itl_p50_ms, None);
        assert_eq!(stats.itl_p99_ms, None);
        assert_eq!(stats.itl_max_ms, None);
        assert_eq!(stats.itl_mean_ms, None);
    }

    #[test]
    fn test_two_chunks() {
        let start = Instant::now();
        let chunks = vec![offset(start, 100), offset(start, 150)];
        let end = offset(start, 200);

        let stats = InferenceLatencyStats::from_timestamps(start, &chunks, end);

        assert_eq!(stats.time_to_first_token_ms, Some(100));
        assert_eq!(stats.time_to_last_byte_ms, 200);
        assert_eq!(stats.chunk_count, 2);
        // One 50ms interval between the two chunks
        assert_eq!(stats.itl_p50_ms, Some(50));
        assert_eq!(stats.itl_p99_ms, Some(50));
        assert_eq!(stats.itl_max_ms, Some(50));
        assert_eq!(stats.itl_mean_ms, Some(50));
    }

    #[test]
    fn test_many_chunks() {
        let start = Instant::now();
        // 11 chunks: intervals are [10, 20, 30, 40, 50, 60, 70, 80, 90, 100]
        let chunks: Vec<Instant> = (0..11)
            .scan(100u64, |acc, i| {
                let t = *acc;
                *acc += (i + 1) * 10;
                Some(offset(start, t))
            })
            .collect();
        let end = offset(start, 1000);

        let stats = InferenceLatencyStats::from_timestamps(start, &chunks, end);

        assert_eq!(stats.time_to_first_token_ms, Some(100));
        assert_eq!(stats.time_to_last_byte_ms, 1000);
        assert_eq!(stats.chunk_count, 11);

        // p50 takes intervals[len / 2] = intervals[5]
        assert_eq!(stats.itl_p50_ms, Some(60));
        // p99_idx is ceil(10 * 0.99) - 1 = 9, so p99 reads intervals[9]
        assert_eq!(stats.itl_p99_ms, Some(100));
        assert_eq!(stats.itl_max_ms, Some(100));
        // The ten intervals sum to 550
        assert_eq!(stats.itl_mean_ms, Some(55));
    }

    #[test]
    fn test_p99_does_not_overflow() {
        let start = Instant::now();
        // 101 chunks give 100 intervals (indices 0..99)
        let chunks: Vec<Instant> = (0..101).map(|i| offset(start, 100 + i * 10)).collect();
        let end = offset(start, 2000);

        let stats = InferenceLatencyStats::from_timestamps(start, &chunks, end);

        assert_eq!(stats.chunk_count, 101);
        // All 100 intervals are 10ms
        // p99_idx is ceil(100 * 0.99) - 1 = 99, which stays in bounds
        assert_eq!(stats.itl_p99_ms, Some(10));
        assert_eq!(stats.itl_max_ms, Some(10));
        assert_eq!(stats.itl_p50_ms, Some(10));
        assert_eq!(stats.itl_mean_ms, Some(10));
    }

    #[test]
    fn test_ttlb_uses_stream_end_not_last_chunk() {
        let start = Instant::now();
        let chunks = vec![offset(start, 100), offset(start, 200)];
        // stream_end is 500ms after start, well past the last chunk at 200ms
        let end = offset(start, 500);

        let stats = InferenceLatencyStats::from_timestamps(start, &chunks, end);

        assert_eq!(stats.time_to_last_byte_ms, 500);
        assert_eq!(stats.time_to_first_token_ms, Some(100));
    }
}
