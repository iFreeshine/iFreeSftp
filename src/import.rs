use std::collections::BTreeMap;

use crate::hosts::{AuthMode, HostProfile};

pub(crate) const MAX_SESSIONS: usize = 5000;

const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

const ENC_PREFIX: &str = "enc:";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SourceKind {

    WinScpIni,

    WinScpRegistry,

    FileZilla,

    PuTTY,

    OpenSsh,

    OwnStore,
}

impl SourceKind {

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::WinScpIni => "WinSCP (INI 文件)",
            Self::WinScpRegistry => "WinSCP (注册表)",
            Self::FileZilla => "FileZilla 站点管理器",
            Self::PuTTY => "PuTTY 会话",
            Self::OpenSsh => "OpenSSH (~/.ssh/config)",
            Self::OwnStore => "iFreeSftp 存档 (JSON)",
        }
    }

    pub(crate) fn index(self) -> usize {
        match self {
            Self::WinScpIni => 0,
            Self::WinScpRegistry => 1,
            Self::FileZilla => 2,
            Self::PuTTY => 3,
            Self::OpenSsh => 4,
            Self::OwnStore => 5,
        }
    }
}

pub(crate) struct ImportedSession {
    pub name: String,
    pub host: String,
    pub port: i32,
    pub username: String,
    pub auth_mode: AuthMode,
    pub key_file_path: String,

    pub warning: &'static str,

    pub own_store_password: String,
}

pub(crate) struct SourceStat {
    #[allow(dead_code)]
    pub kind: SourceKind,
    pub found: usize,
    pub imported: usize,
    pub skipped_non_sftp: usize,
    pub skipped_invalid: usize,
    pub truncated: usize,
}

impl SourceStat {
    fn new(kind: SourceKind) -> Self {
        Self {
            kind,
            found: 0,
            imported: 0,
            skipped_non_sftp: 0,
            skipped_invalid: 0,
            truncated: 0,
        }
    }
}

pub(crate) struct ScanResult {
    pub sessions: Vec<ImportedSession>,

    pub stats: Vec<SourceStat>,

    pub missing: Vec<SourceKind>,

    pub errors: Vec<(SourceKind, &'static str)>,

    pub total_truncated: usize,
}

#[derive(Clone, Copy)]
pub(crate) struct SourceOptions {
    pub kinds: [bool; 6],
}

fn parse_winscp_ini(text: &str, stat: &mut SourceStat) -> Vec<ImportedSession> {
    let mut result = Vec::new();
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    let mut site_name = String::new();
    let mut in_session = false;

    for raw in text.split('\n') {
        let line = raw.trim_end_matches('\r').trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            if in_session {
                flush_winscp(&mut values, &site_name, stat, &mut result, &mut in_session);
            }
            let end = line.find(']').unwrap_or(line.len() - 1);
            let section = &line[1..end];
            if section.to_ascii_uppercase().starts_with("SESSIONS\\") {
                in_session = true;

                site_name = percent_decode(last_segment(section, '\\'));
            }
        } else if in_session
            && let Some(eq) = line.find('=')
            && eq > 0
        {
            values.insert(
                line[..eq].trim().to_owned(),
                line[eq + 1..].trim().to_owned(),
            );
        }
    }
    if in_session {
        flush_winscp(&mut values, &site_name, stat, &mut result, &mut in_session);
    }
    result
}

fn flush_winscp(
    values: &mut BTreeMap<String, String>,
    site_name: &str,
    stat: &mut SourceStat,
    result: &mut Vec<ImportedSession>,
    in_session: &mut bool,
) {
    *in_session = false;
    stat.found += 1;
    let name = std::mem::take(&mut site_name.to_owned());
    if let Some(s) = from_winscp_values(values, &name, stat) {
        add_capped(result, s, stat);
    }
    values.clear();
}

fn from_winscp_values(
    v: &BTreeMap<String, String>,
    name: &str,
    stat: &mut SourceStat,
) -> Option<ImportedSession> {
    let protocol = get_str(v, "Protocol");
    if !protocol.is_empty() && !protocol.eq_ignore_ascii_case("SFTP") {
        stat.skipped_non_sftp += 1;
        return None;
    }
    let host = get_str(v, "HostName").trim().to_owned();
    if host.is_empty() {
        stat.skipped_invalid += 1;
        return None;
    }
    let port = parse_port(&get_str(v, "PortNumber"), stat)?;
    let key_path = get_str(v, "PublicKeyFile").trim().to_owned();
    Some(ImportedSession {
        name: if name.is_empty() {
            host.clone()
        } else {
            name.to_owned()
        },
        host,
        port,
        username: get_str(v, "UserName").trim().to_owned(),
        auth_mode: if key_path.is_empty() {
            AuthMode::Password
        } else {
            AuthMode::KeyFile
        },
        key_file_path: key_path,
        warning: "",
        own_store_password: String::new(),
    })
}

#[cfg(windows)]
fn from_putty_map(
    v: &BTreeMap<String, RegVal>,
    name: &str,
    stat: &mut SourceStat,
) -> Option<ImportedSession> {
    let protocol = get_val_str(v, "Protocol");
    if !protocol.is_empty() && !protocol.eq_ignore_ascii_case("ssh") {
        stat.skipped_non_sftp += 1;
        return None;
    }

    let (user_from_host, host) = split_user_host(get_val_str(v, "HostName").trim());
    if host.is_empty() {
        stat.skipped_invalid += 1;
        return None;
    }
    let mut user = get_val_str(v, "UserName").trim().to_owned();
    if user.is_empty() {
        user = user_from_host;
    }
    let mut port = 22;
    if let Some(&RegVal::Dword(d)) = v
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("PortNumber"))
        .map(|(_, v)| v)
        && (1..=65535).contains(&d)
    {
        port = d as i32;
    }
    let key_path = get_val_str(v, "PublicKeyFile").trim().to_owned();
    Some(ImportedSession {
        name: if name.is_empty() {
            host.clone()
        } else {
            name.to_owned()
        },
        host,
        port,
        username: user,
        auth_mode: if key_path.is_empty() {
            AuthMode::Password
        } else {
            AuthMode::KeyFile
        },
        key_file_path: key_path,
        warning: "",
        own_store_password: String::new(),
    })
}

fn is_forbidden_fz_element(el: &str) -> bool {
    el.eq_ignore_ascii_case("Pass")
        || el.eq_ignore_ascii_case("Password")
        || el.eq_ignore_ascii_case("Passkey")
        || el.eq_ignore_ascii_case("POST_LOGIN_COMMANDS")
}

#[allow(clippy::too_many_lines)]
fn parse_filezilla(text: &str, stat: &mut SourceStat) -> Vec<ImportedSession> {
    let mut result = Vec::new();
    let mut in_server = false;
    let mut host = String::new();
    let mut port = String::new();
    let mut user = String::new();
    let mut name = String::new();
    let mut keyfile = String::new();
    let mut logontype = String::new();
    let mut protocol = String::new();

    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] != '<' {
            i += 1;
            continue;
        }

        let mut j = i + 1;
        let is_end = j < bytes.len() && bytes[j] == '/';
        if is_end {
            j += 1;
        }
        let name_start = j;
        while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == '_') {
            j += 1;
        }
        let tag: String = bytes[name_start..j].iter().collect();
        if tag.is_empty() {
            i += 1;
            continue;
        }

        let close = match bytes[i..].iter().position(|&c| c == '>') {
            Some(p) => i + p,
            None => return result,
        };
        let self_closing = close > 0 && bytes[close - 1] == '/';
        let lower = tag.to_ascii_lowercase();

        if lower == "server" {
            if is_end {
                if in_server {
                    flush_filezilla(
                        &mut host,
                        &mut port,
                        &mut user,
                        &mut name,
                        &mut keyfile,
                        &mut logontype,
                        &mut protocol,
                        &mut in_server,
                        stat,
                        &mut result,
                    );
                }
            } else {

                if in_server {
                    flush_filezilla(
                        &mut host,
                        &mut port,
                        &mut user,
                        &mut name,
                        &mut keyfile,
                        &mut logontype,
                        &mut protocol,
                        &mut in_server,
                        stat,
                        &mut result,
                    );
                }
                in_server = true;
                host.clear();
                port.clear();
                user.clear();
                name.clear();
                keyfile.clear();
                logontype.clear();
                protocol.clear();
            }
            i = close + 1;
            continue;
        }
        if !in_server || is_end || self_closing {
            i = close + 1;
            continue;
        }

        let text_end = bytes[close + 1..]
            .iter()
            .position(|&c| c == '<')
            .map(|p| close + 1 + p)
            .unwrap_or(bytes.len());
        if is_forbidden_fz_element(&lower) {

            i = skip_to_close(&bytes, close + 1, &lower);
            continue;
        }
        let value: String = bytes[close + 1..text_end].iter().collect();
        match lower.as_str() {
            "host" => host = xml_unescape(&value),
            "port" => port = xml_unescape(&value),
            "user" => user = xml_unescape(&value),
            "name" => name = xml_unescape(&value),
            "keyfile" => keyfile = xml_unescape(&value),
            "logontype" => logontype = xml_unescape(&value),
            "protocol" => protocol = xml_unescape(&value),
            _ => {}
        }
        i = skip_to_close(&bytes, close + 1, &lower);
    }
    if in_server {
        flush_filezilla(
            &mut host,
            &mut port,
            &mut user,
            &mut name,
            &mut keyfile,
            &mut logontype,
            &mut protocol,
            &mut in_server,
            stat,
            &mut result,
        );
    }
    result
}

#[expect(clippy::too_many_arguments)]
fn flush_filezilla(
    host: &mut String,
    port: &mut String,
    user: &mut String,
    name: &mut String,
    keyfile: &mut String,
    logontype: &mut String,
    protocol: &mut String,
    in_server: &mut bool,
    stat: &mut SourceStat,
    result: &mut Vec<ImportedSession>,
) {
    *in_server = false;
    stat.found += 1;
    if !protocol.trim().is_empty() && protocol.trim() != "1" {
        stat.skipped_non_sftp += 1;
        return;
    }
    let host_trimmed = host.trim().to_owned();
    if host_trimmed.is_empty() {
        stat.skipped_invalid += 1;
        return;
    }
    let Some(port_num) = parse_port(port.trim(), stat) else {
        return;
    };

    let is_key = logontype.trim() == "5" && !keyfile.trim().is_empty();
    add_capped(
        result,
        ImportedSession {
            name: if name.trim().is_empty() {
                host_trimmed.clone()
            } else {
                name.trim().to_owned()
            },
            host: host_trimmed,
            port: port_num,
            username: user.trim().to_owned(),
            auth_mode: if is_key {
                AuthMode::KeyFile
            } else {
                AuthMode::Password
            },
            key_file_path: if is_key {
                keyfile.trim().to_owned()
            } else {
                String::new()
            },
            warning: "",
            own_store_password: String::new(),
        },
        stat,
    );
    host.clear();
    port.clear();
    user.clear();
    name.clear();
    keyfile.clear();
    logontype.clear();
    protocol.clear();
}

fn skip_to_close(bytes: &[char], from: usize, tag: &str) -> usize {
    let mut depth = 1usize;
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] != '<' {
            i += 1;
            continue;
        }
        let close = match bytes[i..].iter().position(|&c| c == '>') {
            Some(p) => i + p,
            None => return bytes.len(),
        };
        let seg: String = bytes[i + 1..close].iter().collect();
        let t = seg.trim_matches('/');
        if t.eq_ignore_ascii_case(tag) {
            if seg.starts_with('/') {
                depth -= 1;
                if depth == 0 {
                    return close + 1;
                }
            } else if !seg.ends_with('/') {
                depth += 1;
            }
        }
        i = close + 1;
    }
    bytes.len()
}

fn xml_unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        let semi = tail.find(';');
        match semi {

            Some(1..=10) => {
                let ent = &tail[1..semi.unwrap()];
                let rep = if let Some(digits) = ent.strip_prefix("#x") {
                    numeric_entity(u32::from_str_radix(digits, 16).ok())
                } else if let Some(digits) = ent.strip_prefix('#') {
                    numeric_entity(digits.parse::<u32>().ok())
                } else {
                    match ent {
                        "amp" => '&',
                        "lt" => '<',
                        "gt" => '>',
                        "quot" => '"',
                        "apos" => '\'',
                        _ => '\u{FFFD}',
                    }
                };
                out.push(rep);
                rest = &tail[semi.unwrap() + 1..];
            }
            _ => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn numeric_entity(code: Option<u32>) -> char {
    code.and_then(char::from_u32)
        .filter(|&c| {
            matches!(
                c,
                '\u{9}' | '\u{A}' | '\u{D}' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}'
            )
        })
        .unwrap_or('\u{FFFD}')
}

#[allow(clippy::too_many_lines)]
fn parse_openssh(text: &str, stat: &mut SourceStat) -> Vec<ImportedSession> {
    let mut result = Vec::new();
    let mut in_block = false;
    let mut skip_block = false;
    let mut pattern: Option<String> = None;
    let mut host_name: Option<String> = None;
    let mut user: Option<String> = None;
    let mut identity: Option<String> = None;
    let mut port = 0;

    for raw in text.split('\n') {
        let line = raw.trim_end_matches('\r').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(sep) = line.find([' ', '\t', '=']) else {
            continue;
        };
        let keyword = &line[..sep];
        let rest = line[sep + 1..].trim();

        if keyword.eq_ignore_ascii_case("Host") {
            if in_block {
                flush_openssh(
                    &pattern,
                    &host_name,
                    &user,
                    &identity,
                    &mut port,
                    &mut skip_block,
                    &mut in_block,
                    stat,
                    &mut result,
                );
            }
            let patterns = split_tokens(rest);
            in_block = true;
            skip_block =
                patterns.is_empty() || patterns.iter().any(|p| p.contains('*') || p.contains('?'));
            pattern = patterns.first().cloned();
            host_name = None;
            user = None;
            identity = None;
            port = 0;
        } else if keyword.eq_ignore_ascii_case("Match") {
            if in_block {
                flush_openssh(
                    &pattern,
                    &host_name,
                    &user,
                    &identity,
                    &mut port,
                    &mut skip_block,
                    &mut in_block,
                    stat,
                    &mut result,
                );
            }
            in_block = true;
            skip_block = true;
            pattern = None;
            host_name = None;
            user = None;
            identity = None;
            port = 0;
        } else if !in_block || skip_block {
            continue;
        } else if keyword.eq_ignore_ascii_case("HostName") {
            if host_name.is_none() {
                host_name = split_tokens(rest).first().cloned();
            }
        } else if keyword.eq_ignore_ascii_case("Port") {
            if port == 0
                && let Ok(p) = split_tokens(rest)
                    .first()
                    .map(|s| s.as_str())
                    .unwrap_or("")
                    .parse::<i32>()
                && (1..=65535).contains(&p)
            {
                port = p;
            }
        } else if keyword.eq_ignore_ascii_case("User") {
            if user.is_none() {
                user = split_tokens(rest).first().cloned();
            }
        } else if keyword.eq_ignore_ascii_case("IdentityFile") && identity.is_none() {
            identity = expand_identity_file(rest);
        }
    }
    if in_block {
        flush_openssh(
            &pattern,
            &host_name,
            &user,
            &identity,
            &mut port,
            &mut skip_block,
            &mut in_block,
            stat,
            &mut result,
        );
    }
    result
}

#[expect(clippy::too_many_arguments)]
fn flush_openssh(
    pattern: &Option<String>,
    host_name: &Option<String>,
    user: &Option<String>,
    identity: &Option<String>,
    port: &mut i32,
    skip_block: &mut bool,
    in_block: &mut bool,
    stat: &mut SourceStat,
    result: &mut Vec<ImportedSession>,
) {
    *in_block = false;
    if *skip_block {
        *skip_block = false;
        return;
    }
    stat.found += 1;
    let host = host_name
        .clone()
        .or_else(|| pattern.clone())
        .unwrap_or_default();
    if host.trim().is_empty() {
        stat.skipped_invalid += 1;
        return;
    }
    add_capped(
        result,
        ImportedSession {
            name: pattern.clone().unwrap_or_else(|| host.clone()),
            host: host.trim().to_owned(),
            port: if *port == 0 { 22 } else { *port },
            username: user.clone().unwrap_or_default(),
            auth_mode: if identity.is_some() {
                AuthMode::KeyFile
            } else {
                AuthMode::Password
            },
            key_file_path: identity.clone().unwrap_or_default(),
            warning: "",
            own_store_password: String::new(),
        },
        stat,
    );
    *port = 0;
}

fn expand_identity_file(raw: &str) -> Option<String> {
    let mut s = raw.trim().to_owned();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s = s[1..s.len() - 1].trim().to_owned();
    }
    if s.is_empty() {
        return None;
    }
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    if s == "~" {
        s = home;
    } else if s.starts_with("~/") || s.starts_with("~\\") {
        s = home + &s[1..].replace('/', "\\");
    }
    if s.contains('%') {

        let mut out = String::new();
        let mut rest = s.as_str();
        while let Some(a) = rest.find('%') {
            out.push_str(&rest[..a]);
            let tail = &rest[a + 1..];
            match tail.find('%') {
                Some(b) if !tail[..b].contains('%') => {
                    out.push_str(&std::env::var(&tail[..b]).unwrap_or_default());
                    rest = &tail[b + 1..];
                }
                _ => {
                    out.push('%');
                    rest = tail;
                }
            }
        }
        out.push_str(rest);
        s = out;
    }
    if s.is_empty() { None } else { Some(s) }
}

fn parse_own_store_text(text: &str, stat: &mut SourceStat) -> Vec<ImportedSession> {
    let mut result = Vec::new();
    for p in crate::hosts::parse_snapshot(text) {
        stat.found += 1;
        if p.host.trim().is_empty() || !(1..=65535).contains(&p.port) {
            stat.skipped_invalid += 1;
            continue;
        }

        let cipher = if p.password.starts_with(ENC_PREFIX) {
            p.password.clone()
        } else {
            String::new()
        };
        add_capped(
            &mut result,
            ImportedSession {
                name: p.name.clone(),
                host: p.host.clone(),
                port: p.port,
                username: p.username.clone(),
                auth_mode: p.auth_mode,
                key_file_path: p.key_file_path.clone(),
                warning: "",
                own_store_password: cipher,
            },
            stat,
        );
    }
    result
}

#[cfg(windows)]
type Hkey = *mut core::ffi::c_void;

#[cfg(windows)]
const HKEY_CURRENT_USER: Hkey = 0x8000_0001usize as Hkey;
#[cfg(windows)]
const KEY_READ: u32 = 0x2_0019;
#[cfg(windows)]
const ERROR_SUCCESS: i32 = 0;
#[cfg(windows)]
const ERROR_NO_MORE_ITEMS: i32 = 259;
#[cfg(windows)]
const REG_SZ: u32 = 1;
#[cfg(windows)]
const REG_DWORD: u32 = 4;

#[cfg(windows)]
#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegOpenKeyExW(hkey: Hkey, sub: *const u16, opts: u32, sam: u32, out: *mut Hkey) -> i32;
    fn RegCloseKey(h: Hkey) -> i32;
    fn RegEnumKeyExW(
        h: Hkey,
        idx: u32,
        name: *mut u16,
        name_len: *mut u32,
        reserved: *mut u32,
        class: *mut u16,
        class_len: *mut u32,
        ft: *mut u64,
    ) -> i32;
    fn RegEnumValueW(
        h: Hkey,
        idx: u32,
        name: *mut u16,
        name_len: *mut u32,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        data_len: *mut u32,
    ) -> i32;
}

#[cfg(windows)]
enum RegVal {
    Str(String),
    Dword(u32),
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

#[cfg(not(windows))]
fn scan_registry_sessions(
    result: &mut ScanResult,
    kind: SourceKind,
    _base_path: &str,
    _putty: bool,
) {
    result.missing.push(kind);
    result.stats.push(SourceStat::new(kind));
}

#[cfg(windows)]
fn scan_registry_sessions(result: &mut ScanResult, kind: SourceKind, base_path: &str, putty: bool) {
    let mut stat = SourceStat::new(kind);

    unsafe {
        let sub = wide(base_path);
        let mut key: Hkey = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_READ, &mut key) != ERROR_SUCCESS {
            result.missing.push(kind);
            result.stats.push(stat);
            return;
        }
        let mut idx = 0u32;
        let mut name_buf = [0u16; 256];
        loop {
            let mut len = name_buf.len() as u32;
            let rc = RegEnumKeyExW(
                key,
                idx,
                name_buf.as_mut_ptr(),
                &mut len,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            if rc == ERROR_NO_MORE_ITEMS {
                break;
            }
            if rc != ERROR_SUCCESS {
                idx += 1;
                continue;
            }
            let sub_name = from_wide(&name_buf[..len as usize]);
            let mut sub_key: Hkey = std::ptr::null_mut();
            if RegOpenKeyExW(key, name_buf.as_ptr(), 0, KEY_READ, &mut sub_key) == ERROR_SUCCESS {
                let mut values: BTreeMap<String, RegVal> = BTreeMap::new();
                let mut vi = 0u32;
                let mut vn_buf = [0u16; 256];
                loop {
                    let mut vn_len = vn_buf.len() as u32;
                    let mut vk = 0u32;
                    let mut vd_len = 0u32;
                    let rc = RegEnumValueW(
                        sub_key,
                        vi,
                        vn_buf.as_mut_ptr(),
                        &mut vn_len,
                        std::ptr::null_mut(),
                        &mut vk,
                        std::ptr::null_mut(),
                        &mut vd_len,
                    );
                    if rc == ERROR_NO_MORE_ITEMS {
                        break;
                    }
                    if rc == ERROR_SUCCESS && vn_len > 0 {
                        let val_name = from_wide(&vn_buf[..vn_len as usize]);
                        let mut data = [0u8; 1024];
                        let mut got = data.len() as u32;
                        if query_value_raw(sub_key, &vn_buf[..vn_len as usize], &mut data, &mut got)
                            == ERROR_SUCCESS
                        {
                            let val = match vk {
                                REG_SZ => {

                                    let n = (got as usize) & !1;
                                    let u16s: Vec<u16> = (0..n / 2)
                                        .map(|i| u16::from_le_bytes([data[i * 2], data[i * 2 + 1]]))
                                        .collect();
                                    RegVal::Str(from_wide(&u16s))
                                }
                                REG_DWORD if got >= 4 => {
                                    RegVal::Dword(u32::from_le_bytes(data[..4].try_into().unwrap()))
                                }
                                _ => continue,
                            };

                            values.insert(percent_decode(&val_name), val);
                        }
                    }
                    vi += 1;
                }
                RegCloseKey(sub_key);
                stat.found += 1;
                let sess = if putty {
                    from_putty_map(
                        &values,
                        percent_decode(last_segment(&sub_name, '\\')).as_str(),
                        &mut stat,
                    )
                } else {

                    let strs: BTreeMap<String, String> = values
                        .into_iter()
                        .map(|(k, v)| {
                            let s = match v {
                                RegVal::Str(s) => s,
                                RegVal::Dword(d) => d.to_string(),
                            };
                            (k, s)
                        })
                        .collect();
                    from_winscp_values(
                        &strs,
                        percent_decode(last_segment(&sub_name, '\\')).as_str(),
                        &mut stat,
                    )
                };
                if let Some(s) = sess {
                    add_capped(&mut result.sessions, s, &mut stat);
                }
            }
            idx += 1;
        }
        RegCloseKey(key);
    }
    result.stats.push(stat);
}

#[cfg(windows)]
unsafe fn query_value_raw(h: Hkey, name: &[u16], data: &mut [u8], got: &mut u32) -> i32 {
    let name = wide(&String::from_utf16_lossy(name));
    let mut kind = 0u32;
    unsafe extern "system" {
        fn RegQueryValueExW(
            h: Hkey,
            name: *const u16,
            reserved: *mut u32,
            kind: *mut u32,
            data: *mut u8,
            len: *mut u32,
        ) -> i32;
    }
    unsafe {
        RegQueryValueExW(
            h,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut kind,
            data.as_mut_ptr(),
            got,
        )
    }
}

#[allow(clippy::too_many_lines)]
pub(crate) fn scan_sources(opts: &SourceOptions) -> ScanResult {
    let mut result = ScanResult {
        sessions: Vec::new(),
        stats: Vec::new(),
        missing: Vec::new(),
        errors: Vec::new(),
        total_truncated: 0,
    };
    let wanted = |k: SourceKind| opts.kinds[k.index()];

    if wanted(SourceKind::WinScpIni) {
        scan_file_source(
            &mut result,
            SourceKind::WinScpIni,
            &default_winscp_ini_path(),
            winscp_ini_entry,
        );
    }
    if wanted(SourceKind::FileZilla) {
        scan_file_source(
            &mut result,
            SourceKind::FileZilla,
            &default_filezilla_path(),
            filezilla_entry,
        );
    }
    if wanted(SourceKind::OpenSsh) {
        scan_file_source(
            &mut result,
            SourceKind::OpenSsh,
            &default_openssh_path(),
            openssh_entry,
        );
    }
    if wanted(SourceKind::OwnStore) {
        scan_file_source(
            &mut result,
            SourceKind::OwnStore,
            &own_store_path(),
            own_store_entry,
        );
    }
    if wanted(SourceKind::WinScpRegistry) {
        scan_registry_sessions(
            &mut result,
            SourceKind::WinScpRegistry,
            "Software\\Martin Prikryl\\WinSCP 2\\Sessions",
            false,
        );
    }
    if wanted(SourceKind::PuTTY) {
        scan_registry_sessions(
            &mut result,
            SourceKind::PuTTY,
            "Software\\SimonTatham\\PuTTY\\Sessions",
            true,
        );
    }

    for s in &mut result.sessions {
        validate(s);
    }

    if result.sessions.len() > MAX_SESSIONS {
        result.total_truncated = result.sessions.len() - MAX_SESSIONS;
        result.sessions.truncate(MAX_SESSIONS);
    }
    result
}

fn winscp_ini_entry(
    text: &str,
    stat: &mut SourceStat,
    _errs: &mut Vec<&'static str>,
) -> Vec<ImportedSession> {
    parse_winscp_ini(text, stat)
}

fn filezilla_entry(
    text: &str,
    stat: &mut SourceStat,
    _errs: &mut Vec<&'static str>,
) -> Vec<ImportedSession> {
    parse_filezilla(text, stat)
}

fn openssh_entry(
    text: &str,
    stat: &mut SourceStat,
    _errs: &mut Vec<&'static str>,
) -> Vec<ImportedSession> {
    parse_openssh(text, stat)
}

fn own_store_entry(
    text: &str,
    stat: &mut SourceStat,
    errs: &mut Vec<&'static str>,
) -> Vec<ImportedSession> {
    let sessions = parse_own_store_text(text, stat);

    if stat.found == 0 {
        errs.push("parseFailed");
    }
    sessions
}

fn scan_file_source(
    result: &mut ScanResult,
    kind: SourceKind,
    path: &std::path::Path,
    parse: fn(&str, &mut SourceStat, &mut Vec<&'static str>) -> Vec<ImportedSession>,
) {
    let mut stat = SourceStat::new(kind);
    match std::fs::metadata(path) {
        Err(_) => result.missing.push(kind),
        Ok(md) if md.len() > MAX_FILE_BYTES => result.errors.push((kind, "fileTooLarge")),
        Ok(_) => match std::fs::read(path) {
            Err(_) => result.errors.push((kind, "fileReadFailed")),
            Ok(bytes) => {

                let (text, _) = crate::editor::decode(&bytes);
                let mut errs: Vec<&'static str> = Vec::new();
                for s in parse(&text, &mut stat, &mut errs) {
                    add_capped(&mut result.sessions, s, &mut stat);
                }
                for e in errs {
                    result.errors.push((kind, e));
                }
            }
        },
    }
    result.stats.push(stat);
}

fn default_winscp_ini_path() -> std::path::PathBuf {
    std::env::var_os("APPDATA")
        .map(|d| std::path::PathBuf::from(d).join("WinSCP.ini"))
        .unwrap_or_default()
}

fn default_filezilla_path() -> std::path::PathBuf {
    std::env::var_os("APPDATA")
        .map(|d| {
            std::path::PathBuf::from(d)
                .join("FileZilla")
                .join("sitemanager.xml")
        })
        .unwrap_or_default()
}

fn default_openssh_path() -> std::path::PathBuf {
    std::env::var_os("USERPROFILE")
        .map(|d| std::path::PathBuf::from(d).join(".ssh").join("config"))
        .unwrap_or_default()
}

fn own_store_path() -> std::path::PathBuf {
    crate::hosts::store_path_pub()
}

pub(crate) fn validate(s: &mut ImportedSession) {
    if s.auth_mode != AuthMode::KeyFile || s.key_file_path.trim().is_empty() {
        return;
    }
    if s.key_file_path.starts_with("\\\\") || s.key_file_path.starts_with("//") {
        s.key_file_path.clear();
        s.auth_mode = AuthMode::Password;
        s.warning = "uncPath";
        return;
    }

    if !std::path::Path::new(&s.key_file_path).exists() {
        s.warning = "keyNotFound";
    }
}

pub(crate) fn to_profile(s: &ImportedSession) -> HostProfile {
    let mut p = HostProfile {
        name: if s.name.trim().is_empty() {
            s.host.clone()
        } else {
            s.name.clone()
        },
        host: s.host.clone(),
        port: s.port,
        username: s.username.clone(),
        auth_mode: s.auth_mode,
        key_file_path: s.key_file_path.clone(),
        ..Default::default()
    };
    if !s.own_store_password.is_empty() {
        let plain = crate::hosts::unprotect_pub(&s.own_store_password);
        p.password = if plain.starts_with(ENC_PREFIX) {
            String::new()
        } else {
            plain
        };
    }
    p
}

pub(crate) fn dedup_key(host: &str, port: i32, username: &str) -> String {
    format!("{}:{port}:{}", host.trim(), username.trim())
}

fn add_capped(list: &mut Vec<ImportedSession>, s: ImportedSession, stat: &mut SourceStat) {
    if list.len() >= MAX_SESSIONS {
        stat.truncated += 1;
        return;
    }
    list.push(s);
    stat.imported += 1;
}

fn parse_port(raw: &str, stat: &mut SourceStat) -> Option<i32> {
    if raw.trim().is_empty() {
        return Some(22);
    }
    match raw.trim().parse::<i32>() {
        Ok(p) if (1..=65535).contains(&p) => Some(p),
        _ => {
            stat.skipped_invalid += 1;
            None
        }
    }
}

fn get_str(v: &BTreeMap<String, String>, key: &str) -> String {
    v.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

#[cfg(windows)]
fn get_val_str(v: &BTreeMap<String, RegVal>, key: &str) -> String {
    match v
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v)
    {
        Some(RegVal::Str(s)) => s.clone(),
        _ => String::new(),
    }
}

#[cfg(windows)]
fn split_user_host(raw: &str) -> (String, String) {
    match raw.find('@') {
        Some(at) if at > 0 && at < raw.len() - 1 => {
            (raw[..at].to_owned(), raw[at + 1..].to_owned())
        }
        _ => (String::new(), raw.to_owned()),
    }
}

fn last_segment(s: &str, sep: char) -> &str {
    s.rsplit(sep).next().unwrap_or(s)
}

fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_owned();
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(h) = (bytes[i + 1] as char).to_digit(16)
            && let Some(l) = (bytes[i + 2] as char).to_digit(16)
        {
            out.push(((h << 4) | l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn split_tokens(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut in_quote = false;
    let mut cur = String::new();
    for ch in s.chars() {
        match ch {
            '"' => in_quote = !in_quote,
            ' ' | '\t' if !in_quote => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

#[cfg(test)]
mod import_test_helpers {
    use super::{ImportedSession, SourceKind, SourceStat};

    pub(super) fn new_stat(kind: SourceKind) -> SourceStat {
        SourceStat::new(kind)
    }

    pub(super) fn assert_no_secret(sessions: &[ImportedSession], secrets: &[&str]) {
        for s in sessions {
            for field in [
                &s.name,
                &s.host,
                &s.username,
                &s.key_file_path,
                &s.own_store_password,
            ] {
                for secret in secrets {
                    assert!(
                        !field.to_lowercase().contains(&secret.to_lowercase()),
                        "红线泄露：字段 `{field}` 含机密 `{secret}`"
                    );
                }
            }
            assert_eq!(s.warning, "", "warning 字段不得承载机密");
        }
    }
}

#[cfg(test)]
mod decode_bytes_tests {
    #[test]
    fn utf8_bom_chinese_text() {
        let text = "主机配置";
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend(text.as_bytes());
        assert_eq!(crate::editor::decode(&bytes).0, text);
    }

    #[test]
    fn utf16le_bom_winscp_ini_default() {
        let text = "[Sessions\\prod]\r\nHostName=10.0.0.5\r\n";
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(crate::editor::decode(&bytes).0, text);
    }

    #[test]
    fn utf16be_bom() {
        let text = "HostName=example.com";
        let mut bytes = vec![0xFE, 0xFF];
        bytes.extend(text.encode_utf16().flat_map(u16::to_be_bytes));
        assert_eq!(crate::editor::decode(&bytes).0, text);
    }

    #[test]
    fn plain_utf8_no_bom() {
        let bytes = "HostName=example.com".as_bytes().to_vec();
        assert_eq!(crate::editor::decode(&bytes).0, "HostName=example.com");
    }

}

#[cfg(test)]
mod winscp_ini_tests {
    use super::import_test_helpers::{assert_no_secret, new_stat};
    use super::*;

    const SAMPLE_INI: &str = concat!(
        "[Configuration\\Interface]\n",
        "ShowWelcomeDialog=1\n",
        "\n",
        "[Sessions\\prod%20server]\n",
        "HostName=10.0.0.5\n",
        "PortNumber=2200\n",
        "UserName=ops\n",
        "PublicKeyFile=C:\\keys\\id_rsa.pub\n",
        "Protocol=SFTP\n",
        "Password=winscp-obfuscated-secret\n",
        "RemoteCommand=ls -la /var/www\n",
        "\n",
        "[Sessions\\work\\dev]\n",
        "HostName=dev.example.com\n",
        "PortNumber=22\n",
        "UserName=dev\n",
        "Protocol=SCP\n",
        "\n",
        "[Sessions\\legacy]\n",
        "HostName=old.example.com\n",
    );

    #[test]
    fn parses_sessions_folder_and_escaped_names() {
        let mut stat = new_stat(SourceKind::WinScpIni);
        let sessions = parse_winscp_ini(SAMPLE_INI, &mut stat);

        assert_eq!(sessions.len(), 2);
        assert_eq!(stat.found, 3);
        assert_eq!(stat.skipped_non_sftp, 1);

        let prod = &sessions[0];
        assert_eq!(prod.name, "prod server");
        assert_eq!(prod.host, "10.0.0.5");
        assert_eq!(prod.port, 2200);
        assert_eq!(prod.username, "ops");
        assert!(matches!(prod.auth_mode, AuthMode::KeyFile));
        assert_eq!(prod.key_file_path, r"C:\keys\id_rsa.pub");

        let legacy = &sessions[1];
        assert_eq!(legacy.host, "old.example.com");
        assert_eq!(legacy.port, 22);
        assert!(matches!(legacy.auth_mode, AuthMode::Password));
    }

    #[test]
    fn red_lines_never_import_password_or_remote_command() {
        let mut stat = new_stat(SourceKind::WinScpIni);
        let sessions = parse_winscp_ini(SAMPLE_INI, &mut stat);
        assert_no_secret(&sessions, &["winscp-obfuscated-secret", "ls -la"]);
        assert_eq!(sessions[0].own_store_password, "");
    }

    #[test]
    fn utf16le_encoded_file_decodes_and_parses() {
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(SAMPLE_INI.encode_utf16().flat_map(u16::to_le_bytes));
        let (text, _) = crate::editor::decode(&bytes);
        let mut stat = new_stat(SourceKind::WinScpIni);
        assert_eq!(parse_winscp_ini(&text, &mut stat).len(), 2);
    }

    #[test]
    fn invalid_port_skips_session() {
        let mut stat = new_stat(SourceKind::WinScpIni);
        let sessions =
            parse_winscp_ini("[Sessions\\bad]\nHostName=h\nPortNumber=70000\n", &mut stat);
        assert!(sessions.is_empty());
        assert_eq!(stat.skipped_invalid, 1);
    }
}

#[cfg(test)]
mod filezilla_tests {
    use super::import_test_helpers::{assert_no_secret, new_stat};
    use super::*;

    const SAMPLE_XML: &str = r#"
<?xml version="1.0" encoding="UTF-8"?>
<FileZilla3 version="3.67.0" platform="windows">
  <Servers>
    <Server>
      <Host>192.168.1.10</Host>
      <Port>22</Port>
      <Protocol>1</Protocol>
      <User>deploy</User>
      <Pass>filezilla-plaintext-secret</Pass>
      <Logontype>5</Logontype>
      <Keyfile>C:\keys\id_rsa</Keyfile>
      <Name>prod server</Name>
      <POST_LOGIN_COMMANDS>rm -rf /tmp/build</POST_LOGIN_COMMANDS>
    </Server>
    <Server>
      <Host>ftp.example.com</Host>
      <Port>21</Port>
      <Protocol>0</Protocol>
      <User>ftpuser</User>
      <Logontype>1</Logontype>
    </Server>
    <Server>
      <Host>sftp2.example.com</Host>
      <Port>2222</Port>
      <Protocol>1</Protocol>
      <User>root</User>
      <Logontype>1</Logontype>
      <Name>site two</Name>
    </Server>
  </Servers>
</FileZilla3>
"#;

    #[test]
    fn parses_keyfile_and_pw_sessions_skips_non_sftp() {
        let mut stat = new_stat(SourceKind::FileZilla);
        let sessions = parse_filezilla(SAMPLE_XML, &mut stat);

        assert_eq!(sessions.len(), 2);
        assert_eq!(stat.found, 3);
        assert_eq!(stat.skipped_non_sftp, 1);

        let prod = &sessions[0];
        assert_eq!(prod.name, "prod server");
        assert_eq!(prod.host, "192.168.1.10");
        assert!(matches!(prod.auth_mode, AuthMode::KeyFile));
        assert_eq!(prod.key_file_path, r"C:\keys\id_rsa");

        let second = &sessions[1];
        assert_eq!(second.host, "sftp2.example.com");
        assert_eq!(second.port, 2222);
        assert!(matches!(second.auth_mode, AuthMode::Password)); // Logontype=1
        assert_eq!(second.key_file_path, "");
    }

    #[test]
    fn red_lines_never_reads_pass_or_post_login_commands() {
        let mut stat = new_stat(SourceKind::FileZilla);
        let sessions = parse_filezilla(SAMPLE_XML, &mut stat);
        assert_no_secret(&sessions, &["filezilla-plaintext-secret", "rm -rf"]);
    }

    #[test]
    fn dtd_entity_is_inert() {
        // 与 C# DtdEntity_Prohibited 的语义差异：C# 用 XmlDocument，DOCTYPE 直接
        // 抛 XmlException；Rust 是手写扫描器，没有 XML 解析器——实体绝不展开
        // （零文件读取，XXE 结构上不可能）。未识别实体 "&xxe;" 经 xml_unescape
        // 被丢弃为 U+FFFD 替换符，产出一条垃圾会话，由预览表人工剔除。
        const EVIL: &str = r#"
<?xml version="1.0"?>
<!DOCTYPE FileZilla3 [<!ENTITY xxe SYSTEM "file:///C:/Windows/win.ini">]>
<FileZilla3><Servers><Server><Host>&xxe;</Host></Server></Servers></FileZilla3>
"#;
        let mut stat = new_stat(SourceKind::FileZilla);
        let sessions = parse_filezilla(EVIL, &mut stat);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].host, "\u{FFFD}"); // 实体未展开——win.ini 内容进不来
        assert_no_secret(&sessions, &["win.ini"]);
    }
}

// ── PuTTY 注册表键值映射（纯函数直测，免注册表依赖）──────────

// 依赖 Windows-only 符号（from_putty_map/RegVal）：随源码门控。
#[cfg(all(test, windows))]
mod putty_tests {
    use super::import_test_helpers::{assert_no_secret, new_stat};
    use super::*;

    /// 等价 C# Map(params (string,object)[])。注意 RegVal 无 Clone，按值收。
    fn map(pairs: Vec<(&str, RegVal)>) -> BTreeMap<String, RegVal> {
        pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }

    #[test]
    fn ssh_session_user_host_split_dword_port() {
        let mut stat = new_stat(SourceKind::PuTTY);
        let s = from_putty_map(
            &map(vec![
                ("HostName", RegVal::Str("ops@10.0.0.8".into())),
                ("PortNumber", RegVal::Dword(2222)),
                ("Protocol", RegVal::Str("ssh".into())),
                ("PublicKeyFile", RegVal::Str(r"C:\keys\putty.ppk".into())),
                (
                    "RemoteCommand",
                    RegVal::Str("tail -f /var/log/syslog".into()),
                ),
            ]),
            "putty site",
            &mut stat,
        );

        let s = s.expect("应产出会话");
        assert_eq!(s.name, "putty site");
        assert_eq!(s.host, "10.0.0.8");
        assert_eq!(s.username, "ops");
        assert_eq!(s.port, 2222);
        assert!(matches!(s.auth_mode, AuthMode::KeyFile));
        assert_eq!(s.key_file_path, r"C:\keys\putty.ppk");
    }

    #[test]
    fn non_ssh_protocol_skipped() {
        let mut stat = new_stat(SourceKind::PuTTY);
        let s = from_putty_map(
            &map(vec![
                ("HostName", RegVal::Str("serial.example.com".into())),
                ("Protocol", RegVal::Str("telnet".into())),
            ]),
            "x",
            &mut stat,
        );
        assert!(s.is_none());
        assert_eq!(stat.skipped_non_sftp, 1);
    }

    #[test]
    fn defaults_no_protocol_key_and_no_port() {
        let mut stat = new_stat(SourceKind::PuTTY);
        let s = from_putty_map(
            &map(vec![("HostName", RegVal::Str("bare.example.com".into()))]),
            "bare",
            &mut stat,
        );
        let s = s.expect("应产出会话");
        assert_eq!(s.port, 22);
        assert!(matches!(s.auth_mode, AuthMode::Password));
        assert_eq!(s.username, ""); // 无 @ → 不拆分
    }

    #[test]
    fn red_line_remote_command_never_imported() {
        let mut stat = new_stat(SourceKind::PuTTY);
        let s = from_putty_map(
            &map(vec![
                ("HostName", RegVal::Str("h".into())),
                ("Protocol", RegVal::Str("ssh".into())),
                ("RemoteCommand", RegVal::Str("secret-cmd".into())),
            ]),
            "h",
            &mut stat,
        );
        assert_no_secret(&[s.expect("应产出会话")], &["secret-cmd"]);
    }

    // N2：独立 UserName 键优先于 HostName 的 user@host 前缀。
    #[test]
    fn independent_user_name_takes_precedence_over_host_prefix() {
        let mut stat = new_stat(SourceKind::PuTTY);
        let s = from_putty_map(
            &map(vec![
                ("HostName", RegVal::Str("ops@10.0.0.8".into())),
                ("UserName", RegVal::Str("alice".into())),
                ("Protocol", RegVal::Str("ssh".into())),
            ]),
            "dupe",
            &mut stat,
        );
        let s = s.expect("应产出会话");
        assert_eq!(s.username, "alice");
        assert_eq!(s.host, "10.0.0.8");
    }

    #[test]
    fn blank_user_name_falls_back_to_host_prefix() {
        let mut stat = new_stat(SourceKind::PuTTY);
        let s = from_putty_map(
            &map(vec![
                ("HostName", RegVal::Str("ops@10.0.0.8".into())),
                ("UserName", RegVal::Str(String::new())),
                ("Protocol", RegVal::Str("ssh".into())),
            ]),
            "x",
            &mut stat,
        );
        let s = s.expect("应产出会话");
        assert_eq!(s.username, "ops"); // 独立键为空 → 回退拆分
    }
}

// ── OpenSSH ~/.ssh/config ────────────────────────────────────

#[cfg(test)]
mod openssh_tests {
    use super::import_test_helpers::{assert_no_secret, new_stat};
    use super::*;

    const SAMPLE_CONFIG: &str = r#"# 全局注释
Host *
    ServerAliveInterval 60

Host web1
    HostName 192.168.1.20
    User deploy
    Port 2222
    IdentityFile ~/.ssh/id_ed25519
    ProxyCommand nc %h %p

Host "db server"
    HostName 10.0.0.9
    IdentityFile "%USERPROFILE%\keys\db_key"

Match host *.corp
    User corpuser

Host alias-only
"#;

    #[test]
    fn parses_concrete_hosts_skips_wildcard_match_and_alias() {
        let mut stat = new_stat(SourceKind::OpenSsh);
        let sessions = parse_openssh(SAMPLE_CONFIG, &mut stat);

        assert_eq!(sessions.len(), 3);
        assert_eq!(stat.found, 3);

        let home = std::env::var("USERPROFILE").unwrap_or_default();

        let web1 = &sessions[0];
        assert_eq!(web1.name, "web1");
        assert_eq!(web1.host, "192.168.1.20");
        assert_eq!(web1.port, 2222);
        assert_eq!(web1.username, "deploy");
        assert!(matches!(web1.auth_mode, AuthMode::KeyFile));
        assert_eq!(web1.key_file_path, format!("{home}\\.ssh\\id_ed25519"));

        let db = &sessions[1];
        assert_eq!(db.name, "db server");
        assert_eq!(db.host, "10.0.0.9");
        assert_eq!(db.key_file_path, format!("{home}\\keys\\db_key"));

        let alias = &sessions[2];
        assert_eq!(alias.host, "alias-only"); // 无 HostName → pattern 作主机
        assert_eq!(alias.port, 22);
        assert!(matches!(alias.auth_mode, AuthMode::Password));
    }

    #[test]
    fn red_lines_proxy_command_never_imported() {
        let mut stat = new_stat(SourceKind::OpenSsh);
        let sessions = parse_openssh(SAMPLE_CONFIG, &mut stat);
        assert_no_secret(&sessions, &["nc %h"]);
    }

    #[test]
    fn include_and_unknown_keys_ignored() {
        const CFG: &str = r#"Include ~/.ssh/config.d/*.conf
Host h1
    HostName 10.0.0.1
    AddKeysToAgent yes
"#;
        let mut stat = new_stat(SourceKind::OpenSsh);
        let sessions = parse_openssh(CFG, &mut stat);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].host, "10.0.0.1");
    }
}

// ── 自有存档 hosts.json（经 parse_own_store_text 纯函数注入）──
// 注意：hosts::parse_snapshot 是行式解析（与存档写出格式互为镜像），
// 测试 JSON 必须多行构造，单行 JSON 一条记录都出不来。

#[cfg(test)]
mod own_store_tests {
    use super::import_test_helpers::new_stat;
    use super::*;

    /// C# 样例原样（多行）。第二条记录是 camelCase——Rust 现状不识别，见分歧钉子。
    const SAMPLE_JSON: &str = r#"{
  "hosts": [
    {
      "Name": "迁移A",
      "Host": "10.1.1.1",
      "Port": 2200,
      "AuthMode": "KeyFile",
      "KeyFilePath": "C:\\keys\\a",
      "Username": "ops",
      "Password": "enc:QUJDREVGRw=="
    },
    {
      "name": "camel",
      "host": "10.1.1.2",
      "port": 22,
      "username": "u2",
      "Password": "plaintext-must-not-migrate"
    }
  ]
}"#;

    #[test]
    fn parses_cipher_kept() {

        let mut stat = new_stat(SourceKind::OwnStore);
        let sessions = parse_own_store_text(SAMPLE_JSON, &mut stat);
        assert_eq!(sessions[0].own_store_password, "enc:QUJDREVGRw==");
        assert!(matches!(sessions[0].auth_mode, AuthMode::KeyFile));
        assert_eq!(sessions[0].host, "10.1.1.1");
        assert_eq!(sessions[0].name, "迁移A");
        // 明文口令（第二条记录的 PascalCase "Password" 键）绝不迁移。
        assert_eq!(
            sessions[0].own_store_password.len(),
            "enc:QUJDREVGRw==".len()
        );
        assert!(!sessions[0].own_store_password.contains("plaintext"));
    }

    #[test]
    fn plaintext_password_never_migrates() {
        // 红线 1：无 enc: 前缀的明文口令绝不迁移（独立于 camelCase 问题验证）。
        const JSON: &str = r#"{
  "hosts": [
    {
      "Name": "p",
      "Host": "10.9.9.9",
      "Port": 22,
      "Username": "u",
      "Password": "plaintext-must-not-migrate"
    }
  ]
}"#;
        let mut stat = new_stat(SourceKind::OwnStore);
        let sessions = parse_own_store_text(JSON, &mut stat);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].own_store_password, "");
    }

    #[test]
    fn camel_case_keys_accepted() {
        // 原为分歧钉子（钉 parse_into 只认 PascalCase）；parse_into 已修复为
        // 键统一小写匹配（hosts.rs「OrdinalIgnoreCase」注释 + 守护用例），
        // 2026-11-06 翻转为修复后语义：camelCase 记录照常导入。
        let mut stat = new_stat(SourceKind::OwnStore);
        let sessions = parse_own_store_text(SAMPLE_JSON, &mut stat);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[1].host, "10.1.1.2");
    }

    #[test]
    fn broken_json_tolerated_divergence() {
        // ⚠ 分歧钉子：C# 抛 ImportScanException("parseFailed") 显式上报；
        // Rust 行式解析对坏输入容忍（本例无任何合法行 → 空结果，无错误码）。
        // 修复（scan_file_source 增加 parseFailed code）后改为断言错误上报。
        let mut stat = new_stat(SourceKind::OwnStore);
        let sessions = parse_own_store_text("{ hosts: [", &mut stat);
        assert!(sessions.is_empty());
    }

    #[test]
    fn session_cap_truncates_at_5000() {
        // 行式格式：每条记录占多行（与存档写出格式互为镜像）。
        let mut json = String::from("{\n\"hosts\": [\n");
        for i in 0..5001 {
            if i > 0 {
                json.push_str(",\n");
            }
            json.push_str(&format!(
                "{{\n\"Host\": \"10.0.0.{i}\",\n\"Port\": 22,\n\"Username\": \"u\"\n}}\n"
            ));
        }
        json.push_str("]\n}");

        let mut stat = new_stat(SourceKind::OwnStore);
        let sessions = parse_own_store_text(&json, &mut stat);
        assert_eq!(sessions.len(), MAX_SESSIONS);
        assert_eq!(stat.truncated, 1);
    }
}

// ── to_profile：fail-safe 解密 ───────────────────────────────

#[cfg(test)]
mod to_profile_tests {
    use super::*;

    fn session(_kind: SourceKind, cipher: &str) -> ImportedSession {
        ImportedSession {
            name: String::new(),
            host: "h".into(),
            port: 22,
            username: "u".into(),
            auth_mode: AuthMode::Password,
            key_file_path: String::new(),
            warning: "",
            own_store_password: cipher.into(),
        }
    }

    #[test]
    fn decryption_failure_failsafe_clears_password() {
        // DPAPI 解不开的密文 → unprotect 原样带回（仍带 enc: 前缀）→ 必须清空。
        let s = session(SourceKind::OwnStore, "enc:CIPHER");
        let p = to_profile(&s);
        assert!(p.password.is_empty());
    }

    #[test]
    fn non_own_store_never_carries_password() {
        let s = session(SourceKind::FileZilla, "");
        let p = to_profile(&s);
        assert!(p.password.is_empty());
    }

    #[test]
    fn empty_name_falls_back_to_host() {
        let s = ImportedSession {
            host: "1.2.3.4".into(),
            ..session(SourceKind::OwnStore, "")
        };
        assert_eq!(to_profile(&s).name, "1.2.3.4");
    }

    // C# DecryptedPlain_Assigned 不翻译：成功路径需要真实 DPAPI 密文，
    // hosts::protect 未公开（见 README 待办）。修复后可用 protect+unprotect 对拍。
}

// ── Validate：UNC 拒收 / 密钥不存在告警 ──────────────────────

#[cfg(test)]
mod validate_tests {
    use super::*;
    use std::fs;

    fn key_session(path: &str) -> ImportedSession {
        ImportedSession {
            name: String::new(),
            host: "h".into(),
            port: 22,
            username: String::new(),
            auth_mode: AuthMode::KeyFile,
            key_file_path: path.into(),
            warning: "",
            own_store_password: String::new(),
        }
    }

    #[test]
    fn unc_path_rejected_falls_back_to_password() {
        let mut s = key_session(r"\\nas\share\id_rsa");
        validate(&mut s);
        assert_eq!(s.key_file_path, "");
        assert!(matches!(s.auth_mode, AuthMode::Password));
        assert_eq!(s.warning, "uncPath");
    }

    #[test]
    fn forward_slash_unc_also_rejected() {
        let mut s = key_session("//nas/share/id_rsa");
        validate(&mut s);
        assert_eq!(s.warning, "uncPath");
    }

    #[test]
    fn missing_key_file_keeps_path_warns() {
        let missing = std::env::temp_dir()
            .join("ifree-test-missing-key-6f4d2a91.pem")
            .to_string_lossy()
            .into_owned();
        let mut s = key_session(&missing);
        validate(&mut s);
        assert_eq!(s.key_file_path, missing); // 保留路径供用户修正
        assert!(matches!(s.auth_mode, AuthMode::KeyFile));
        assert_eq!(s.warning, "keyNotFound");
    }

    #[test]
    fn existing_key_file_no_warning() {
        let path = std::env::temp_dir().join("ifree-test-key-exists.pem");
        fs::write(&path, b"test").expect("临时密钥文件应可写");
        let mut s = key_session(&path.to_string_lossy());
        validate(&mut s);
        assert_eq!(s.warning, "");
        let _ = fs::remove_file(&path); // 清理，失败无害
    }

    #[test]
    fn password_mode_skips_validation() {
        let mut s = key_session(r"\\x\y");
        s.auth_mode = AuthMode::Password;
        validate(&mut s);
        assert_eq!(s.key_file_path, r"\\x\y"); // 未被触碰
        assert_eq!(s.warning, "");
    }
}

// ── DedupKey ─────────────────────────────────────────────────

#[cfg(test)]
mod dedup_key_tests {
    use super::*;

    #[test]
    fn format_host_port_username() {
        assert_eq!(
            dedup_key(" example.com ", 22, " root "),
            "example.com:22:root"
        );
    }

    #[test]
    fn case_sensitive_consistent_with_store_lookup() {
        assert_ne!(
            dedup_key("Example.com", 22, "root"),
            dedup_key("example.com", 22, "root")
        );
    }
}

// ── ScanSources：来源白名单（N1）────────────────────────────

#[cfg(test)]
mod scan_sources_tests {
    use super::*;

    #[test]
    fn no_source_selected_produces_empty_result() {
        // 一个来源都不勾选：不做任何探测，结果全空（而非 missing 一串）。
        let result = scan_sources(&SourceOptions { kinds: [false; 6] });
        assert!(result.sessions.is_empty());
        assert!(result.stats.is_empty());
        assert!(result.missing.is_empty());
        assert!(result.errors.is_empty());
    }

    // C# UncheckedSources_AreNotReadAtAll（自定义 ini/json 路径）不直接翻译：
    // Rust scan_sources 的来源路径是固定默认值，测试无法注入（见 README 待办：
    // SourceOptions 增加路径覆盖字段后补全量 N1 用例）。
}

// ============================================================================
// 集成配套重构（一处，行为逐位等价）——parse_own_store 拆出纯函数：
//
// -    fn parse_own_store(stat: &mut SourceStat) -> Vec<ImportedSession> {
// -        let mut result = Vec::new();
// -        for p in
// -            crate::hosts::parse_snapshot(&std::fs::read_to_string(own_store_path()).unwrap_or_default())
// -        {
// +    fn parse_own_store(stat: &mut SourceStat) -> Vec<ImportedSession> {
// +        parse_own_store_text(
// +            &std::fs::read_to_string(own_store_path()).unwrap_or_default(),
// +            stat,
// +        )
// +    }
// +
// +    /// 纯函数部分（供单测注入文本，不触真实存档文件）。
// +    fn parse_own_store_text(text: &str, stat: &mut SourceStat) -> Vec<ImportedSession> {
// +        let mut result = Vec::new();
// +        for p in crate::hosts::parse_snapshot(text) {
//              ...（其余函数体原样不动）
//
// 后续批次（可选）：SourceOptions 增加路径覆盖，补 N1 全量用例。
// ============================================================================
