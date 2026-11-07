//! 机器标识与哈希（C# MachineId / CodeHash 逐项对齐）。
//!
//! - MachineId = 机器名 + "|" + 用户名 → UTF-8 → SHA-256 → hex 前 16 位小写
//!   （C# Environment.MachineName + Environment.UserName 同语义；Windows 用
//!   GetComputerNameW + USERNAME 环境变量，Unix 用 hostname + USER/LOGNAME。
//!   签发/上报/吊销均只认哈希，名字本身不出本机）。
//! - CodeHash = 注册码 Trim() → SHA-256 → hex 前 24 位小写
//!   （/v1/activate 的 h 字段、/v1/state 吊销名单的 key）。

use ring::digest;

/// 机器标识（激活上报 m 字段 / 吊销名单不直接使用）。
pub(crate) fn machine_id() -> String {
    let raw = format!("{}|{}", machine_name(), user_name());
    sha256_hex(raw.as_bytes())[..16].to_owned()
}

/// 注册码哈希（h 字段 / 吊销名单 key；C# CodeHash 同式：Trim 后哈希）。
pub(crate) fn code_hash(code: &str) -> String {
    sha256_hex(code.trim().as_bytes())[..24].to_owned()
}

fn sha256_hex(data: &[u8]) -> String {
    let d = digest::digest(&digest::SHA256, data);
    let mut out = String::with_capacity(d.as_ref().len() * 2);
    for b in d.as_ref() {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(windows)]
fn machine_name() -> String {
    // C# Environment.MachineName = GetComputerNameW（NetBIOS 名，≤15 字符）
    const MAX: u32 = 32; // MAX_COMPUTERNAME_LENGTH+1 = 16，留余量
    let mut buf = [0u16; MAX as usize];
    let mut len = MAX;
    // SAFETY：buf/len 均为调用方栈上变量；成功时系统写入以 len 计的字符数。
    let ok = unsafe { GetComputerNameW(buf.as_mut_ptr(), &mut len) };
    if ok != 0 && len > 0 {
        return String::from_utf16_lossy(&buf[..len as usize]);
    }
    // FFI 失败回退环境变量（交互会话两者一致）
    std::env::var("COMPUTERNAME").unwrap_or_default()
}

#[cfg(windows)]
fn user_name() -> String {
    // C# Environment.UserName = 进程令牌 GetUserName；交互会话与 USERNAME 一致
    std::env::var("USERNAME").unwrap_or_default()
}

#[cfg(not(windows))]
fn machine_name() -> String {
    // POSIX gethostname FFI（glibc/musl/macOS 全有；零新依赖）——环境变量
    // HOSTNAME 仅 bash 会话设置，systemd 服务/桌面启动下常常缺失，不可靠。
    unsafe extern "C" {
        fn gethostname(name: *mut core::ffi::c_char, len: usize) -> i32;
    }
    let mut buf = [0u8; 256];
    // SAFETY：buf 为调用方栈上缓冲区，len 为其长度；POSIX 约定成功时写入
    // NUL 结尾主机名。（macOS 原型第二参为 int，值恒 ≤256 不越界。）
    let rc = unsafe { gethostname(buf.as_mut_ptr() as *mut core::ffi::c_char, buf.len()) };
    if rc == 0 {
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        if end > 0 {
            return String::from_utf8_lossy(&buf[..end]).into_owned();
        }
    }
    // FFI 失败回退 /etc/hostname（无 init 注入的环境变量最后兜底）
    if let Ok(h) = std::fs::read_to_string("/etc/hostname") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_owned();
        }
    }
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("hostname"))
        .unwrap_or_default()
}

#[cfg(not(windows))]
fn user_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_default()
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetComputerNameW(lp_buffer: *mut u16, n_size: *mut u32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_id_shape() {
        let id = machine_id();
        assert_eq!(id.len(), 16);
        assert!(
            id.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn code_hash_matches_sha256_vector() {
        // SHA-256("abc") = ba7816bf…，截 24 位（C# CodeHash 同式同序）
        assert_eq!(code_hash("abc"), "ba7816bf8f01cfea414140de");
        // Trim 语义：带空白与不带同哈希（C# HashCode(Trim()) 同）
        assert_eq!(code_hash("  abc \n"), code_hash("abc"));
    }
}
