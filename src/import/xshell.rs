//! What Xshell keeps in its data folder (`文档\NetSarang Computer\<版本>\Xshell`),
//! read for 从 Xshell 导入: the sessions under `Sessions` (a subfolder is a
//! group) and the quick command sets in `QuickButton Files`. Every file is
//! an INI in UTF-16 with a byte order mark.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::xshell_crypt::XshellKey;
use crate::host::{ProxyKind, ProxySettings};

const SESSIONS: &str = "Sessions";
const QUICK_COMMANDS: &str = "QuickButton Files";

/// The Xshell data folder of the newest Xshell in the user's documents.
pub fn default_folder() -> Option<PathBuf> {
    let base = dirs::document_dir()?.join("NetSarang Computer");
    fs::read_dir(base)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let version: u32 = entry.file_name().to_str()?.parse().ok()?;
            let folder = entry.path().join("Xshell");
            folder.is_dir().then_some((version, folder))
        })
        .max_by_key(|(version, _)| *version)
        .map(|(_, folder)| folder)
}

/// Whether `folder` is one Xshell keeps its data in.
pub fn is_xshell_folder(folder: &Path) -> bool {
    folder.join(SESSIONS).is_dir() || folder.join(QUICK_COMMANDS).is_dir()
}

/// Everything there is to import from one Xshell data folder.
#[derive(Debug, Default)]
pub struct XshellData {
    pub sessions: Vec<XshellSession>,
    /// Sessions of another protocol than SSH (Telnet, RDP, serial…).
    pub other_protocols: Vec<String>,
    pub command_sets: Vec<CommandSet>,
    /// Files that could not be read, by name.
    pub unreadable: Vec<String>,
}

/// One SSH session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XshellSession {
    /// The folders it is in under `Sessions`, outermost first.
    pub group: Vec<String>,
    pub name: String,
    pub address: String,
    pub port: u16,
    pub user: String,
    /// It logs in with a key of Xshell's own, which cannot come along.
    pub key_login: bool,
    pub password: Option<String>,
    /// It saved a password this Windows user cannot open.
    pub password_unreadable: bool,
    /// The HTTP or SOCKS5 proxy it goes through, with its password.
    pub proxy: Option<(ProxySettings, Option<String>)>,
    /// It goes through a proxy ShellRS cannot take (SOCKS4, a missing
    /// one…), so it connects directly.
    pub proxy_dropped: bool,
}

/// One quick command set: a `.qbl` file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandSet {
    /// The file's name without `.qbl`.
    pub name: String,
    pub commands: Vec<QuickCommand>,
    /// Buttons that run a script or do something else than send text.
    pub not_text: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickCommand {
    pub name: String,
    /// What it sends, a newline for each Enter but the last.
    pub command: String,
    /// It ends with Enter.
    pub run_on_click: bool,
    /// It names `$HOSTNAME`, `$USERNAME` or `$PATH`, which Xshell fills in
    /// and ShellRS does not.
    pub session_variables: bool,
}

/// Read what is in `folder`, opening saved passwords with `key`.
pub fn read_folder(folder: &Path, key: Option<&XshellKey>) -> XshellData {
    let mut data = XshellData::default();
    let proxies = folder
        .parent()
        .map(|parent| parent.join("Common").join("Proxy"));
    let mut files = Vec::new();
    session_files(&folder.join(SESSIONS), &[], &mut files);
    for (group, path) in files {
        match read_ini(&path) {
            Some(ini) => {
                let name = file_stem(&path);
                match session(&ini, group, name.clone(), key, proxies.as_deref()) {
                    Some(session) => data.sessions.push(session),
                    None => data.other_protocols.push(name),
                }
            }
            None => data.unreadable.push(file_name(&path)),
        }
    }
    for path in listed(&folder.join(QUICK_COMMANDS), "qbl") {
        match read_ini(&path) {
            Some(ini) => data.command_sets.push(command_set(&ini, file_stem(&path))),
            None => data.unreadable.push(file_name(&path)),
        }
    }
    data
}

/// The `.xsh` files under `dir`, each with the folders it is in. Hidden
/// folders (a sync tool's `.stversions`) are passed over.
fn session_files(dir: &Path, group: &[String], files: &mut Vec<(Vec<String>, PathBuf)>) {
    for path in listed(dir, "xsh") {
        files.push((group.to_vec(), path));
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut folders: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && !file_name(path).starts_with('.'))
        .collect();
    folders.sort();
    for folder in folders {
        let mut inner = group.to_vec();
        inner.push(file_name(&folder));
        session_files(&folder, &inner, files);
    }
}

/// The files in `dir` ending in `.extension`, by name, leaving out the
/// conflicting copies a sync tool leaves beside them.
fn listed(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
                && !file_name(path).contains(".sync-conflict-")
        })
        .collect();
    files.sort();
    files
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// An INI file's values by section and key.
#[derive(Debug, Default)]
struct Ini(HashMap<String, HashMap<String, String>>);

impl Ini {
    fn parse(text: &str) -> Self {
        let mut ini = Self::default();
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = name.to_owned();
            } else if let Some((key, value)) = line.split_once('=') {
                ini.0
                    .entry(section.clone())
                    .or_default()
                    .insert(key.trim().to_owned(), value.to_owned());
            }
        }
        ini
    }

    /// The value, trimmed; empty when there is none.
    fn get(&self, section: &str, key: &str) -> &str {
        self.0
            .get(section)
            .and_then(|keys| keys.get(key))
            .map_or("", |value| value.trim())
    }
}

fn read_ini(path: &Path) -> Option<Ini> {
    Some(Ini::parse(&decode_text(&fs::read(path).ok()?)?))
}

/// Xshell's files are UTF-16 with a byte order mark; UTF-8 is taken too.
fn decode_text(bytes: &[u8]) -> Option<String> {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        return String::from_utf16(&units).ok();
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8(bytes.to_vec()).ok()
}

/// The session a `.xsh` file describes; `None` for another protocol.
fn session(
    ini: &Ini,
    group: Vec<String>,
    name: String,
    key: Option<&XshellKey>,
    proxies: Option<&Path>,
) -> Option<XshellSession> {
    const AUTH: &str = "CONNECTION:AUTHENTICATION";
    let protocol = ini.get("CONNECTION", "Protocol");
    let address = ini.get("CONNECTION", "Host");
    if !(protocol.is_empty() || protocol.eq_ignore_ascii_case("SSH")) || address.is_empty() {
        return None;
    }
    let (password_on, key_on) =
        auth_methods(ini.get(AUTH, "AuthMethodList"), ini.get(AUTH, "Method"));
    let saved = ini.get(AUTH, "Password");
    let password = (!saved.is_empty()).then(|| key.and_then(|key| key.decrypt(saved)));
    let password_unreadable = password_on && matches!(password, Some(None));
    let password = password.flatten().filter(|_| password_on);
    let mut session = XshellSession {
        group,
        name,
        address: address.to_owned(),
        port: ini
            .get("CONNECTION", "Port")
            .parse()
            .ok()
            .filter(|port| *port > 0)
            .unwrap_or(22),
        user: ini.get(AUTH, "UserName").to_owned(),
        key_login: key_on && password.is_none(),
        password,
        password_unreadable,
        proxy: None,
        proxy_dropped: false,
    };
    let proxy = ini.get("CONNECTION:PROXY", "Proxy");
    if !proxy.is_empty() {
        let found = proxies.and_then(|dir| read_ini(&dir.join(format!("{proxy}.ini"))));
        match found.and_then(|ini| proxy_settings(&ini, key)) {
            Some((settings, password, unreadable)) => {
                session.proxy = Some((settings, password));
                session.password_unreadable |= unreadable;
            }
            None => session.proxy_dropped = true,
        }
    }
    Some(session)
}

/// Whether a session logs in with a password, and with a key. Each entry of
/// `AuthMethodList` is the method (0 password, 1 public key, then
/// keyboard-interactive, GSSAPI…) and whether it is on; older files only
/// have `Method`.
fn auth_methods(list: &str, method: &str) -> (bool, bool) {
    if list.is_empty() {
        return match method {
            "1" => (false, true),
            _ => (true, false),
        };
    }
    let on = |wanted: &str| list.split(',').any(|entry| entry.trim() == wanted);
    (on("01"), on("11"))
}

/// A proxy Xshell keeps in `Common\Proxy`: `TYPE` 2 is SOCKS5 and 3 is
/// HTTP; the rest (SOCKS4, SOCKS4A…) ShellRS cannot go through. With the
/// password, and whether one was saved that cannot be opened.
fn proxy_settings(
    ini: &Ini,
    key: Option<&XshellKey>,
) -> Option<(ProxySettings, Option<String>, bool)> {
    const SECTION: &str = "SECTION";
    let kind = match ini.get(SECTION, "TYPE") {
        "2" => ProxyKind::Socks5,
        "3" => ProxyKind::Http,
        _ => return None,
    };
    let host = ini.get(SECTION, "HOST");
    let port = ini
        .get(SECTION, "PORT")
        .parse()
        .ok()
        .filter(|port| *port > 0)?;
    if host.is_empty() {
        return None;
    }
    let settings = ProxySettings::new(kind, host.to_owned(), port)
        .with_user(ini.get(SECTION, "USERNAME").to_owned());
    let saved = ini.get(SECTION, "PASSWORD");
    if saved.is_empty() || settings.user.is_none() {
        return Some((settings, None, false));
    }
    let password = key.and_then(|key| key.decrypt(saved));
    let unreadable = password.is_none();
    Some((settings, password, unreadable))
}

/// The buttons of a `.qbl` file, in their order.
fn command_set(ini: &Ini, name: String) -> CommandSet {
    const SECTION: &str = "QuickButton";
    let mut numbers: Vec<u32> = ini
        .0
        .get(SECTION)
        .map(|keys| {
            keys.keys()
                .filter_map(|key| {
                    key.strip_prefix("Button_")?
                        .strip_suffix("_Action")?
                        .parse()
                        .ok()
                })
                .collect()
        })
        .unwrap_or_default();
    numbers.sort_unstable();
    let mut set = CommandSet {
        name,
        ..CommandSet::default()
    };
    for number in numbers {
        let field = |field: &str| {
            ini.get(SECTION, &format!("Button_{number}_{field}"))
                .to_owned()
        };
        let kind = field("Type");
        if !(kind.is_empty() || kind == "1") {
            set.not_text += 1;
            continue;
        }
        let raw = ini
            .0
            .get(SECTION)
            .and_then(|keys| keys.get(&format!("Button_{number}_Action")))
            .cloned()
            .unwrap_or_default();
        if let Some(command) = quick_command(field("Name"), &raw) {
            set.commands.push(command);
        }
    }
    set
}

/// A button that sends text, or `None` when it sends nothing. A button
/// without a name is called by its first line.
fn quick_command(name: String, raw: &str) -> Option<QuickCommand> {
    let box_text = unescape_file(raw);
    let session_variables = names_session_variables(&box_text);
    let sent = unescape_sent(&box_text);
    let run_on_click = sent.ends_with('\n');
    let command = sent.trim_end_matches('\n').to_owned();
    if command.trim().is_empty() {
        return None;
    }
    let name = match name.trim() {
        "" => command.lines().next().unwrap_or_default().trim().to_owned(),
        name => name.to_owned(),
    };
    Some(QuickCommand {
        name,
        command,
        run_on_click,
        session_variables,
    })
}

/// The text of a button's editing box from its `.qbl` value: there a
/// backslash is `\\` and a line break `\r` and `\n`.
fn unescape_file(raw: &str) -> String {
    let mut text = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            text.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => text.push('\\'),
            Some('r') => text.push('\r'),
            Some('n') => text.push('\n'),
            Some(other) => {
                text.push('\\');
                text.push(other);
            }
            None => text.push('\\'),
        }
    }
    text
}

/// What Xshell sends for the text in a button's box: a line break, `\r`
/// and `\n` are Enter (a `\r` the line breaks after counts once), `\t` a
/// tab, `\a` the bell, `\b` backspace, `\\` and `\$` themselves, `\` and
/// up to three octal digits that character. Any other `\` stays.
fn unescape_sent(text: &str) -> String {
    let mut sent = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' | '\n' => {
                skip_line_break(c, &mut chars);
                sent.push('\n');
            }
            '\\' => match chars.peek().copied() {
                Some('r' | 'n') => {
                    chars.next();
                    if let Some(next @ ('\r' | '\n')) = chars.peek().copied() {
                        chars.next();
                        skip_line_break(next, &mut chars);
                    }
                    sent.push('\n');
                }
                Some(escaped @ ('t' | 'a' | 'b' | '\\' | '$')) => {
                    chars.next();
                    sent.push(match escaped {
                        't' => '\t',
                        'a' => '\u{7}',
                        'b' => '\u{8}',
                        other => other,
                    });
                }
                Some('0'..='7') => sent.push(octal(&mut chars)),
                _ => sent.push('\\'),
            },
            other => sent.push(other),
        }
    }
    sent
}

/// After a `\r` line break, the `\n` that makes it `\r\n`.
fn skip_line_break(first: char, chars: &mut std::iter::Peekable<std::str::Chars>) {
    if first == '\r' && chars.peek() == Some(&'\n') {
        chars.next();
    }
}

/// The character up to three octal digits name.
fn octal(chars: &mut std::iter::Peekable<std::str::Chars>) -> char {
    let mut value = 0u32;
    for _ in 0..3 {
        match chars.peek().and_then(|c| c.to_digit(8)) {
            Some(digit) => {
                value = value * 8 + digit;
                chars.next();
            }
            None => break,
        }
    }
    char::from_u32(value).unwrap_or('\u{FFFD}')
}

/// Whether the box names one of the session variables Xshell fills in:
/// `$HOSTNAME`, `$USERNAME` or `$PATH`, not written `\$`.
fn names_session_variables(text: &str) -> bool {
    ["HOSTNAME", "USERNAME", "PATH"].iter().any(|name| {
        let wanted = format!("${name}");
        text.match_indices(&wanted).any(|(at, _)| {
            let escaped = text[..at].ends_with('\\');
            let after = text[at + wanted.len()..].chars().next();
            !escaped && !after.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        })
    })
}

#[cfg(test)]
#[path = "xshell_tests.rs"]
mod tests;
