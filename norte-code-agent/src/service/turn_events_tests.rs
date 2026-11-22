use norte::Usage;

use super::{Stats, accumulate_usage};

#[test]
fn provider_counters_saturate_instead_of_overflowing() {
    let mut stats = Stats {
        requests: u64::MAX,
        input_tokens: u64::MAX,
        output_tokens: u64::MAX,
        cache_read_tokens: u64::MAX,
        cache_write_tokens: u64::MAX,
        ..Stats::default()
    };
    let mut usage = Usage::default();
    usage.input_tokens = Some(u64::MAX);
    usage.output_tokens = Some(u64::MAX);
    usage.cache_read_tokens = Some(u64::MAX);
    usage.cache_write_tokens = Some(u64::MAX);

    accumulate_usage(&mut stats, &usage);

    assert_eq!(stats.requests, u64::MAX);
    assert_eq!(stats.input_tokens, u64::MAX);
    assert_eq!(stats.output_tokens, u64::MAX);
    assert_eq!(stats.cache_read_tokens, u64::MAX);
    assert_eq!(stats.cache_write_tokens, u64::MAX);
}
