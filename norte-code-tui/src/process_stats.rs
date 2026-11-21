//! Sampling the TUI process's resident memory and CPU use.

use std::process::Command;

/// Resident memory in kibibytes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Kilobytes(u64);

impl Kilobytes {
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }

    pub(crate) fn mib(self) -> f64 {
        mib(self.0)
    }

    pub(crate) fn delta_mib(self, baseline: Self) -> f64 {
        mib(self.0) - mib(baseline.0)
    }
}

/// Current and starting resident memory.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct MemoryUsage {
    pub(crate) current: Kilobytes,
    pub(crate) baseline: Kilobytes,
}

/// Process counters shown by the debug overlay.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProcessUsage {
    pub(crate) memory: MemoryUsage,
    pub(crate) cpu_percent: f64,
}

/// One raw sample before the baseline is applied.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProcessSample {
    pub(crate) rss: Kilobytes,
    pub(crate) cpu_percent: f64,
}

/// Read RSS and lifetime CPU use from the process table.
pub(crate) fn read_process_usage() -> Option<ProcessSample> {
    let pid = std::process::id().to_string();
    let output = Command::new("ps")
        .args(["-o", "rss=,%cpu=", "-p", &pid])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    parse_process_sample(&text)
}

fn parse_process_sample(text: &str) -> Option<ProcessSample> {
    let mut fields = text.split_whitespace();
    let rss = Kilobytes::new(fields.next()?.parse().ok()?);
    let cpu_percent = fields.next()?.parse::<f64>().ok()?;
    if !cpu_percent.is_finite() {
        return None;
    }
    Some(ProcessSample { rss, cpu_percent })
}

fn mib(kib: u64) -> f64 {
    f64::from(u32::try_from(kib).unwrap_or(u32::MAX)) / 1024.0
}

#[cfg(test)]
#[path = "process_stats_tests.rs"]
mod tests;
