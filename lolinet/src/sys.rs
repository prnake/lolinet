//! 平台差异：Unix（macOS、Linux）和 Windows。
//!
//! Windows 上现在只做第一步（docs/design.md 的 Windows 一节）：前台跑内核（无 TUN 入网）、出口节点的 `exit run`、
//! 看状态、邀请、打印 mihomo 配置。要装系统服务的命令在 [`crate::need_root`] 那里就停，服务控制管理器下一步接。
//! Unix 上这里的每个函数和原来写在各处的一样。

use std::{fs, io, path::Path};

use anyhow::Result;

/// 权限位设成 mode。Windows 没有权限位：什么都不做（ACL 跟着服务一起做）
pub fn chmod(path: impl AsRef<Path>, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

/// 新建文件（已经有了就失败）；Unix 上建出来就是 mode，中间没有别人读得到的时候
pub fn create_new(path: impl AsRef<Path>, mode: u32) -> io::Result<fs::File> {
    let mut o = fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        o.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    o.open(path)
}

/// 是不是 root。Windows：服务还没做，不认管理员，当作不是
pub fn is_root() -> bool {
    #[cfg(unix)]
    {
        unsafe { nix::libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// 这个目录当前用户写得了
pub fn writable_dir(dir: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
            return false;
        };
        unsafe { nix::libc::access(c.as_ptr(), nix::libc::W_OK) == 0 }
    }
    #[cfg(not(unix))]
    {
        let probe = dir.join(format!(".lolinet-{}", std::process::id()));
        let ok = fs::File::create(&probe).is_ok();
        let _ = fs::remove_file(&probe);
        ok
    }
}

/// 要以 root 跑的东西所在的地方：属于 root、组和其他人写不了。Windows 上还不以管理员跑任何东西：不认
pub fn root_only(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let m = fs::metadata(path).map_err(|e| anyhow::anyhow!("看 {}：{e}", path.display()))?;
        if m.uid() != 0 || m.mode() & 0o022 != 0 {
            anyhow::bail!(
                "{} 不是只有 root 能写（owner {}，权限 {:o}）",
                path.display(),
                m.uid(),
                m.mode() & 0o777
            );
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        anyhow::bail!("{}：Windows 上还不以管理员身份装东西", path.display())
    }
}

/// 符号链接 link -> target。Windows 上建符号链接要特权，现在也用不到：报不支持
pub fn symlink(target: impl AsRef<Path>, link: impl AsRef<Path>) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(not(unix))]
    {
        let _ = (target, link);
        Err(io::Error::new(io::ErrorKind::Unsupported, "Windows 上不建符号链接"))
    }
}

/// 连接失败的系统错误码是不是「网络不可达」「主机不可达」（SOCKS5 回应里分开报）
pub fn unreachable(raw: i32) -> Option<bool> {
    #[cfg(unix)]
    {
        match raw {
            nix::libc::ENETUNREACH => Some(false),
            nix::libc::EHOSTUNREACH => Some(true),
            _ => None,
        }
    }
    #[cfg(not(unix))]
    {
        // Windows：WSAENETUNREACH、WSAEHOSTUNREACH
        match raw {
            10051 => Some(false),
            10065 => Some(true),
            _ => None,
        }
    }
}
