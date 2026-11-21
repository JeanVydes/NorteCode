//! Process counter parsing and memory formatting.

use super::{Kilobytes, MemoryUsage, parse_process_sample};

#[test]
fn process_sample_reads_resident_memory_and_cpu() {
    let sample = parse_process_sample("  65536   0.7\n").expect("valid sample");

    assert_eq!(sample.rss, Kilobytes::new(65536));
    assert!((sample.cpu_percent - 0.7).abs() < f64::EPSILON);
}

#[test]
fn process_sample_rejects_incomplete_output() {
    assert!(parse_process_sample("65536\n").is_none());
    assert!(parse_process_sample("65536 nan\n").is_none());
    assert!(parse_process_sample("").is_none());
}

#[test]
fn memory_usage_reports_mebibytes_and_growth() {
    let usage = MemoryUsage {
        current: Kilobytes::new(64 * 1024),
        baseline: Kilobytes::new(60 * 1024),
    };

    assert!((usage.current.mib() - 64.0).abs() < f64::EPSILON);
    assert!((usage.current.delta_mib(usage.baseline) - 4.0).abs() < f64::EPSILON);
}
