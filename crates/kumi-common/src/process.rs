//! Other processes, as the system records them.

/// When process `pid` started, in milliseconds since the Unix epoch; None when it isn't running, can't be read, or the
/// platform keeps no such record. A pid is reused once its process is gone, so a lock naming a pid whose process
/// started after the lock was taken was left by a process that's gone.
pub fn started_at_ms(pid: u32) -> Option<i64> {
    imp::started_at_ms(pid)
}

#[cfg(target_os = "macos")]
mod imp {
    pub fn started_at_ms(pid: u32) -> Option<i64> {
        let pid = i32::try_from(pid).ok()?;
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: the buffer is one proc_bsdinfo, of the size given, which is all proc_pidinfo writes.
        let written = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, info.as_mut_ptr().cast(), size) };
        if written != size {
            return None;
        }
        // SAFETY: proc_pidinfo filled it whole.
        let info = unsafe { info.assume_init() };
        Some(info.pbi_start_tvsec as i64 * 1000 + info.pbi_start_tvusec as i64 / 1000)
    }
}

#[cfg(target_os = "linux")]
mod imp {
    pub fn started_at_ms(pid: u32) -> Option<i64> {
        // /proc/<pid>/stat's field 22 is the start in clock ticks after boot; the fields are counted after the
        // command's closing parenthesis, which the command itself may contain.
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let ticks: i64 = stat.get(stat.rfind(')')? + 1..)?.split_whitespace().nth(19)?.parse().ok()?;
        let boot: i64 =
            std::fs::read_to_string("/proc/stat").ok()?.lines().find_map(|line| line.strip_prefix("btime "))?.trim().parse().ok()?;
        // SAFETY: sysconf reads a constant.
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        (hz > 0).then(|| boot * 1000 + ticks * 1000 / hz as i64)
    }
}

#[cfg(windows)]
mod imp {
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
        fn GetProcessTimes(
            process: *mut std::ffi::c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    pub fn started_at_ms(pid: u32) -> Option<i64> {
        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
        // SAFETY: a plain query; the handle is closed below.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return None;
        }
        let (mut creation, mut exit, mut kernel, mut user) =
            (FileTime::default(), FileTime::default(), FileTime::default(), FileTime::default());
        // SAFETY: each pointer is a FileTime of this frame.
        let ok = unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
        // SAFETY: the handle OpenProcess returned.
        unsafe { CloseHandle(handle) };
        let _ = (exit, kernel, user);
        // 100-nanosecond intervals since 1601-01-01.
        let intervals = ((creation.high as i64) << 32) | creation.low as i64;
        (ok != 0).then(|| (intervals - 116_444_736_000_000_000) / 10_000)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod imp {
    pub fn started_at_ms(_: u32) -> Option<i64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::started_at_ms;
    #[test]
    fn a_running_process_has_a_start_and_one_started_later_starts_later() {
        let now = crate::time::now_ms();
        let mine = started_at_ms(std::process::id()).expect("this process's start");
        assert!(mine <= now + 1000 && mine > now - 24 * 3_600_000, "{mine} against {now}");
        let mut command = if cfg!(windows) {
            let mut command = std::process::Command::new("ping");
            command.args(["-n", "3", "127.0.0.1"]);
            command
        } else {
            let mut command = std::process::Command::new("sleep");
            command.arg("2");
            command
        };
        let mut child = command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).spawn().unwrap();
        let theirs = started_at_ms(child.id());
        let _ = child.kill();
        child.wait().unwrap();
        let theirs = theirs.expect("the child's start");
        assert!(theirs >= mine && theirs <= crate::time::now_ms() + 1000, "{theirs} against {mine}");
    }
}
