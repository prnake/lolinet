//! 读别的进程的名字和当前目录：会话列表显示前台程序，存会话时记下目录（重启后在那里起新 shell）。

use std::path::PathBuf;

/// 进程的名字（可执行文件名），读不到是 None。
pub fn name(pid: i32) -> Option<String> {
    imp::name(pid).filter(|s| !s.is_empty())
}

/// 进程的当前目录，读不到是 None。
pub fn cwd(pid: i32) -> Option<PathBuf> {
    imp::cwd(pid)
}

#[cfg(target_os = "macos")]
mod imp {
    use std::{ffi::CStr, mem, path::PathBuf};

    pub fn name(pid: i32) -> Option<String> {
        let mut buf = [0u8; 256];
        // SAFETY: buf 是可写的，长度如实给出；proc_name 最多写这么多并以 NUL 结尾。
        let n = unsafe { libc::proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
        if n <= 0 {
            return None;
        }
        Some(String::from_utf8_lossy(&buf[..n as usize]).into_owned())
    }

    pub fn cwd(pid: i32) -> Option<PathBuf> {
        // SAFETY: proc_vnodepathinfo 是纯数据结构，全零是合法值。
        let mut info: libc::proc_vnodepathinfo = unsafe { mem::zeroed() };
        let size = mem::size_of::<libc::proc_vnodepathinfo>() as i32;
        // SAFETY: info 可写、大小如实给出。
        let n = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDVNODEPATHINFO,
                0,
                (&mut info as *mut libc::proc_vnodepathinfo).cast(),
                size,
            )
        };
        if n != size {
            return None;
        }
        // SAFETY: vip_path 是内核填的、NUL 结尾的 C 字符串。
        let path = unsafe { CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr().cast()) };
        let path = path.to_str().ok()?;
        (!path.is_empty()).then(|| PathBuf::from(path))
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::path::PathBuf;

    pub fn name(pid: i32) -> Option<String> {
        std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .ok()
            .map(|s| s.trim().to_string())
    }

    pub fn cwd(pid: i32) -> Option<PathBuf> {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod imp {
    use std::path::PathBuf;

    pub fn name(_pid: i32) -> Option<String> {
        None
    }

    pub fn cwd(_pid: i32) -> Option<PathBuf> {
        None
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn this_process() {
        let pid = std::process::id() as i32;
        assert!(super::name(pid).is_some());
        assert_eq!(super::cwd(pid), std::env::current_dir().ok());
        assert_eq!(super::name(-1), None);
    }
}
