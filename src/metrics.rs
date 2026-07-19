use std::ffi::CString;
use std::fs;
use std::mem::MaybeUninit;

const GPU_UTILIZATION_PATH: &str = "/sys/class/misc/mali0/device/utilization";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CpuTimes {
    total: u64,
    idle: u64,
}

#[derive(Debug, Clone)]
pub struct SystemInfo {
    pub kernel: String,
    pub alpine: String,
    pub memory: String,
    pub cpu_usage: Option<u8>,
    pub gpu_usage: Option<u8>,
    pub storage_used: String,
    pub storage_free: String,
    previous_cpu: Option<CpuTimes>,
}

impl SystemInfo {
    pub fn collect() -> Self {
        let kernel = read_trimmed("/proc/sys/kernel/osrelease").unwrap_or_else(|| "UNKNOWN".into());
        let alpine = read_trimmed("/etc/alpine-release").unwrap_or_else(|| "UNKNOWN".into());
        let memory = fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|text| parse_total_memory(&text))
            .unwrap_or_else(|| "UNKNOWN".into());
        let (storage_used, storage_free) = storage_usage("/")
            .map(|(used, free)| (format_bytes(used), format_bytes(free)))
            .unwrap_or_else(|| ("UNKNOWN".into(), "UNKNOWN".into()));

        Self {
            kernel,
            alpine,
            memory,
            cpu_usage: None,
            gpu_usage: read_gpu_usage(),
            storage_used,
            storage_free,
            previous_cpu: read_cpu_times(),
        }
    }

    pub fn refresh(&mut self) {
        if let Some(current) = read_cpu_times() {
            if let Some(previous) = self.previous_cpu {
                self.cpu_usage = cpu_usage(previous, current);
            }
            self.previous_cpu = Some(current);
        }
        self.gpu_usage = read_gpu_usage();
        if let Some((used, free)) = storage_usage("/") {
            self.storage_used = format_bytes(used);
            self.storage_free = format_bytes(free);
        }
    }
}

fn read_trimmed(path: &str) -> Option<String> {
    Some(fs::read_to_string(path).ok()?.trim().to_string())
}

fn parse_total_memory(meminfo: &str) -> Option<String> {
    meminfo.lines().find_map(|line| {
        let value = line.strip_prefix("MemTotal:")?.split_whitespace().next()?;
        let kib: u64 = value.parse().ok()?;
        Some(format!("{} MB", kib / 1024))
    })
}

fn read_cpu_times() -> Option<CpuTimes> {
    let stat = fs::read_to_string("/proc/stat").ok()?;
    parse_cpu_times(&stat)
}

fn parse_cpu_times(stat: &str) -> Option<CpuTimes> {
    let mut fields = stat.lines().next()?.split_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    let values: Vec<u64> = fields.map(str::parse).collect::<Result<_, _>>().ok()?;
    if values.len() < 5 {
        return None;
    }
    Some(CpuTimes {
        total: values.iter().copied().sum(),
        idle: values[3].saturating_add(values[4]),
    })
}

fn cpu_usage(previous: CpuTimes, current: CpuTimes) -> Option<u8> {
    let total = current.total.saturating_sub(previous.total);
    if total == 0 {
        return None;
    }
    let idle = current.idle.saturating_sub(previous.idle).min(total);
    let busy = total - idle;
    Some(((busy.saturating_mul(100) + total / 2) / total).min(100) as u8)
}

fn read_gpu_usage() -> Option<u8> {
    let contents = read_trimmed(GPU_UTILIZATION_PATH)?;
    parse_gpu_usage(&contents)
}

fn parse_gpu_usage(contents: &str) -> Option<u8> {
    contents
        .split_whitespace()
        .next()?
        .parse::<u16>()
        .ok()
        .map(|value| value.min(100) as u8)
}

fn storage_usage(path: &str) -> Option<(u64, u64)> {
    let path = CString::new(path).ok()?;
    let mut stats = MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is a live NUL-terminated string and `stats` points to
    // writable storage for one result. The struct is read only after success.
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: the successful libc call above initialized the complete struct.
    let stats = unsafe { stats.assume_init() };
    let block_size = stats.f_frsize.max(1);
    let total = stats.f_blocks.saturating_mul(block_size);
    let free = stats.f_bfree.saturating_mul(block_size);
    let available = stats.f_bavail.saturating_mul(block_size);
    Some((total.saturating_sub(free), available))
}

fn format_bytes(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    format!("{:.1} GB", bytes as f64 / GIB)
}

pub fn usage_text(usage: Option<u8>) -> String {
    usage.map_or_else(|| "--%".into(), |value| format!("{value}%"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_cpu_parser_and_delta_include_all_columns() {
        let previous = parse_cpu_times("cpu  100 20 30 700 10 5 5 0 0 0\n").unwrap();
        let current = parse_cpu_times("cpu  160 20 40 780 10 5 5 0 0 0\n").unwrap();
        assert_eq!(cpu_usage(previous, current), Some(47));
    }

    #[test]
    fn cpu_parser_rejects_per_core_line() {
        assert_eq!(parse_cpu_times("cpu0 1 2 3 4 5\n"), None);
    }

    #[test]
    fn gpu_parser_clamps_driver_values() {
        assert_eq!(parse_gpu_usage("47\n"), Some(47));
        assert_eq!(parse_gpu_usage("125 0\n"), Some(100));
        assert_eq!(parse_gpu_usage("unavailable\n"), None);
    }

    #[test]
    fn memory_and_storage_values_are_human_readable() {
        assert_eq!(
            parse_total_memory("MemTotal:       6144000 kB\n"),
            Some("6000 MB".into())
        );
        assert_eq!(format_bytes(16 * 1024 * 1024 * 1024), "16.0 GB");
    }
}
