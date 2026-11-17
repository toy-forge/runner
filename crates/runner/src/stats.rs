//! Live statistics of a running program (CodeRunner's `stats.sh`: `ps` over the
//! process and its descendants).

use std::process::Command;
use std::time::Duration;

use crate::path::output_with_timeout;

/// CPU and memory of a process tree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProcessStats {
    /// Summed CPU usage in percent of one core (`ps %cpu`).
    pub cpu_percent: f32,
    /// Summed resident memory in bytes.
    pub rss_bytes: u64,
    /// How many processes are in the tree.
    pub processes: usize,
}

/// Parses `ps -A -o pid=,ppid=,pcpu=,rss=` output and sums the tree below `root`
/// (including `root`).
pub fn sum_tree(ps_output: &str, root: u32) -> Option<ProcessStats> {
    let rows: Vec<(u32, u32, f32, u64)> = ps_output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((
                fields.next()?.parse().ok()?,
                fields.next()?.parse().ok()?,
                fields.next()?.parse().ok()?,
                fields.next()?.parse().ok()?,
            ))
        })
        .collect();
    if !rows.iter().any(|row| row.0 == root) {
        return None;
    }
    let mut members = vec![root];
    let mut index = 0;
    while index < members.len() {
        let parent = members[index];
        for row in &rows {
            if row.1 == parent && !members.contains(&row.0) {
                members.push(row.0);
            }
        }
        index += 1;
    }
    let mut stats = ProcessStats {
        cpu_percent: 0.0,
        rss_bytes: 0,
        processes: 0,
    };
    for row in rows.iter().filter(|row| members.contains(&row.0)) {
        stats.cpu_percent += row.2;
        stats.rss_bytes += row.3 * 1024;
        stats.processes += 1;
    }
    Some(stats)
}

/// Samples the tree below `pid` with one `ps` call. `None` when the process is
/// gone or `ps` fails.
pub fn sample_tree(pid: u32) -> Option<ProcessStats> {
    let mut command = Command::new("/bin/ps");
    command.args(["-A", "-o", "pid=,ppid=,pcpu=,rss="]);
    let (status, stdout) = output_with_timeout(command, Duration::from_secs(3)).ok()?;
    if !status.success() {
        return None;
    }
    sum_tree(&String::from_utf8_lossy(&stdout), pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_a_tree() {
        let ps =
            "  1 0 0.0 100\n 10 1 1.5 2000\n 11 10 2.5 3000\n 12 10 0.5 1000\n 20 1 9.0 9000\n";
        let stats = sum_tree(ps, 10).unwrap();
        assert_eq!(stats.processes, 3);
        assert_eq!(stats.rss_bytes, 6000 * 1024);
        assert!((stats.cpu_percent - 4.5).abs() < 1e-6);
        assert!(sum_tree(ps, 99).is_none());
    }

    #[test]
    fn samples_this_process() {
        let stats = sample_tree(std::process::id()).expect("ps works");
        assert!(stats.rss_bytes > 0);
        assert!(stats.processes >= 1);
    }
}
