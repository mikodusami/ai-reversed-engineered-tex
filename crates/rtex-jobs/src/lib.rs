//! rtex-jobs: job postings in, tailored resumes out.
//!
//! The engine behind RTeX's Jobs window and `rtex job`. It works on an
//! existing job-search folder (default `~/Projects/career-job-search`) and
//! uses the Claude Code CLI on this Mac for the tailoring itself.

pub mod capture;
pub mod claims;
pub mod claude;
pub mod context;
pub mod edits;
pub mod ffi;
pub mod linecheck;
pub mod prompt;
pub mod tailor;
pub mod workspace;

pub use workspace::{Job, Posting, Workspace};

/// rtex-core's C functions, re-exported so the app links one static library.
pub use rtex_core::ffi as core_ffi;

/// Today's local date as (year, month, day).
pub fn today() -> (i64, u32, u32) {
    civil(local_secs() / 86_400)
}

/// Local time as `2026-10-27T11:04:05-0400`.
pub fn now_iso() -> String {
    let secs = local_secs();
    let (y, m, d) = civil(secs.div_euclid(86_400));
    let t = secs.rem_euclid(86_400);
    let off = utc_offset_secs();
    let sign = if off < 0 { '-' } else { '+' };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}{sign}{:02}{:02}",
        t / 3600,
        (t / 60) % 60,
        t % 60,
        off.abs() / 3600,
        (off.abs() / 60) % 60
    )
}

fn local_secs() -> i64 {
    let utc = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    utc + utc_offset_secs()
}

/// The local UTC offset, from `date +%z` (no time-zone database needed).
fn utc_offset_secs() -> i64 {
    static OFF: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| {
        let out = std::process::Command::new("date").arg("+%z").output().ok();
        let s = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
        if s.len() != 5 {
            return 0;
        }
        let sign = if s.starts_with('-') { -1 } else { 1 };
        let h: i64 = s[1..3].parse().unwrap_or(0);
        let m: i64 = s[3..5].parse().unwrap_or(0);
        sign * (h * 3600 + m * 60)
    })
}

/// Days since 1970-01-01 → (y, m, d) (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    #[test]
    fn civil_dates() {
        assert_eq!(super::civil(0), (1970, 1, 1));
        assert_eq!(super::civil(20_753), (2026, 10, 27));
    }
}
