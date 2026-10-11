use std::fs;
use std::path::Path;

use super::*;

// Encrypted outside ShellRS for alice, S-1-5-21-1-2-3-1001 (see
// `xshell_crypt`): "hunter2 密码" and "proxy-pass".
const PASSWORD: &str = "mFkyXWQc1eLwglvNmClOk5XQQTLBNz2dzvo2OipTJ59amW1zDhzERo3XTbixLw==";
const PROXY_PASSWORD: &str = "gF4zUXhDl6NmXtAXfQMOh1qEgz1fuY1d9CQdpx0dMtGIYWa/kKGPajvr";

fn key() -> XshellKey {
    XshellKey::new("alice", "S-1-5-21-1-2-3-1001")
}

/// Write `text` the way Xshell does: UTF-16 with a byte order mark, CRLF.
fn write_utf16(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut bytes = vec![0xFF, 0xFE];
    for unit in text.replace('\n', "\r\n").encode_utf16() {
        bytes.extend(unit.to_le_bytes());
    }
    fs::write(path, bytes).unwrap();
}

fn xsh(host: &str, port: &str, user: &str, auth: &str, password: &str, proxy: &str) -> String {
    format!(
        "[CONNECTION:PROXY]\nProxy={proxy}\nStartUp=0\n\
         [CONNECTION]\nPort={port}\nHost={host}\nProtocol=SSH\n\
         [CONNECTION:AUTHENTICATION]\nPassword={password}\nAuthMethodList={auth}\n\
         UserKey=Cong\nUserName={user}\n"
    )
}

fn xshell_folder() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let xshell = root.path().join("Xshell");
    let sessions = xshell.join("Sessions");
    write_utf16(
        &sessions.join("web.xsh"),
        &xsh("10.0.0.1", "22", "root", "01,10,20", PASSWORD, ""),
    );
    write_utf16(
        &sessions.join("db.xsh"),
        &xsh("10.0.0.2", "2222", "dba", "00,11,20", "", ""),
    );
    write_utf16(
        &sessions.join("生产/k8s/node.xsh"),
        &xsh("node", "22", "ops", "01,11", "", ""),
    );
    write_utf16(
        &sessions.join("web.sync-conflict-20260101-1-ABC.xsh"),
        &xsh("x", "22", "x", "", "", ""),
    );
    write_utf16(
        &sessions.join(".stversions/old~2025.xsh"),
        &xsh("y", "22", "y", "", "", ""),
    );
    write_utf16(
        &sessions.join("rdp.xsh"),
        "[CONNECTION]\nHost=win\nProtocol=RDP\n",
    );
    write_utf16(&sessions.join("default"), "[CONNECTION]\nHost=template\n");
    fs::write(sessions.join("broken.xsh"), [0xFF, 0xFE, 0x00, 0xD8]).unwrap();
    write_utf16(
        &sessions.join("proxied.xsh"),
        &xsh("10.0.0.3", "22", "root", "01", "", "office"),
    );
    write_utf16(
        &sessions.join("socks4.xsh"),
        &xsh("10.0.0.4", "22", "root", "01", "", "old"),
    );
    write_utf16(
        &sessions.join("missing.xsh"),
        &xsh("10.0.0.5", "22", "root", "01", "", "gone"),
    );
    let proxies = root.path().join("Common").join("Proxy");
    write_utf16(
        &proxies.join("office.ini"),
        &format!(
            "[SECTION]\nJUMPHOST=\nPASSWORD={PROXY_PASSWORD}\nPORT=1080\nTYPE=2\n\
             HOST=proxy.test\nSESSION=\nUSERNAME=me\n"
        ),
    );
    write_utf16(
        &proxies.join("old.ini"),
        "[SECTION]\nPORT=1080\nTYPE=0\nHOST=p\n",
    );
    root
}

fn session<'a>(data: &'a XshellData, name: &str) -> &'a XshellSession {
    data.sessions
        .iter()
        .find(|session| session.name == name)
        .unwrap()
}

#[test]
fn sessions_come_with_their_folders_and_saved_passwords() {
    let root = xshell_folder();
    let data = read_folder(&root.path().join("Xshell"), Some(&key()));

    let names: Vec<&str> = data.sessions.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["db", "missing", "proxied", "socks4", "web", "node"]);
    assert_eq!(data.other_protocols, ["rdp"]);
    assert_eq!(data.unreadable, ["broken.xsh"]);

    let web = session(&data, "web");
    assert_eq!(
        (web.address.as_str(), web.port, web.user.as_str()),
        ("10.0.0.1", 22, "root")
    );
    assert_eq!(web.password.as_deref(), Some("hunter2 密码"));
    assert!(!web.key_login && !web.password_unreadable);
    assert!(web.group.is_empty());

    let node = session(&data, "node");
    assert_eq!(node.group, ["生产", "k8s"]);
}

#[test]
fn a_session_with_only_a_key_on_logs_in_with_one() {
    let root = xshell_folder();
    let data = read_folder(&root.path().join("Xshell"), Some(&key()));

    let db = session(&data, "db");
    assert_eq!(db.port, 2222);
    assert!(db.key_login);
    assert_eq!(db.password, None);
    // Password and key both on, and no password saved: Xshell got in with
    // the key.
    assert!(session(&data, "node").key_login);
}

#[test]
fn a_password_another_user_saved_is_left_behind_and_said() {
    let root = xshell_folder();
    let other = XshellKey::new("bob", "S-1-5-21-9");
    let web_for_bob = read_folder(&root.path().join("Xshell"), Some(&other));
    let web = session(&web_for_bob, "web");
    assert_eq!(web.password, None);
    assert!(web.password_unreadable);
    assert!(!web.key_login);

    let without_key = read_folder(&root.path().join("Xshell"), None);
    assert!(session(&without_key, "web").password_unreadable);
}

#[test]
fn http_and_socks5_proxies_come_along_and_the_rest_are_dropped() {
    let root = xshell_folder();
    let data = read_folder(&root.path().join("Xshell"), Some(&key()));

    let (settings, password) = session(&data, "proxied").proxy.clone().unwrap();
    assert_eq!(
        settings,
        ProxySettings::new(ProxyKind::Socks5, "proxy.test", 1080).with_user("me")
    );
    assert_eq!(password.as_deref(), Some("proxy-pass"));
    for name in ["socks4", "missing"] {
        let dropped = session(&data, name);
        assert_eq!(dropped.proxy, None);
        assert!(dropped.proxy_dropped, "{name}");
    }
    assert!(!session(&data, "web").proxy_dropped);
}

#[test]
fn auth_methods_read_the_list_or_the_older_method() {
    assert_eq!(auth_methods("01,10,20,30,40,50", ""), (true, false));
    assert_eq!(auth_methods("00,11,20,30,40,50", "0"), (false, true));
    assert_eq!(auth_methods("01,11,20,30", ""), (true, true));
    assert_eq!(auth_methods("", "1"), (false, true));
    assert_eq!(auth_methods("", ""), (true, false));
}

#[test]
fn the_file_escapes_backslashes_and_line_breaks() {
    assert_eq!(unescape_file(r"pm2 list\\r"), r"pm2 list\r");
    assert_eq!(unescape_file(r"a && \\r\n[ b ]\r\n"), "a && \\r\n[ b ]\r\n");
    // Older files kept a lone backslash as it was.
    assert_eq!(unescape_file(r"PS1='\[\e[32m\]'"), r"PS1='\[\e[32m\]'");
}

#[test]
fn what_is_sent_follows_xshells_escapes() {
    assert_eq!(unescape_sent(r"pm2 list\r"), "pm2 list\n");
    assert_eq!(unescape_sent("a\r\nb\nc"), "a\nb\nc");
    // `\r` and the line break after it are one Enter.
    assert_eq!(unescape_sent("a && \\r\n[ b ]"), "a && \n[ b ]");
    assert_eq!(unescape_sent("a \\r\r\nb"), "a \nb");
    assert_eq!(unescape_sent(r"\t\a\b"), "\t\u{7}\u{8}");
    assert_eq!(unescape_sent(r"echo \$PATH \\n"), r"echo $PATH \n");
    assert_eq!(unescape_sent(r"\033[Z \0 \1234"), "\u{1b}[Z \0 \u{53}4");
    assert_eq!(unescape_sent(r"\x1b \e \u\h"), r"\x1b \e \u\h");
    assert_eq!(unescape_sent("trailing \\"), "trailing \\");
}

#[test]
fn quick_commands_keep_their_order_and_say_whether_they_run() {
    let root = tempfile::tempdir().unwrap();
    let xshell = root.path().join("Xshell");
    write_utf16(
        &xshell.join("QuickButton Files").join("commands.qbl"),
        "[Info]\nVersion=8.2\nCount=5\n[QuickButton]\n\
         Button_10_Name=tmux\nButton_10_Action=tmux ls\\\\r\nButton_10_Type=1\n\
         Button_2_Name=multi\nButton_2_Action=cd /var \\\\\\r\\n  && ls\\r\\n\nButton_2_Type=1\n\
         Button_3_Name=script\nButton_3_Action=C:\\run.vbs\nButton_3_Type=2\n\
         Button_4_Name=\nButton_4_Action=df -h\nButton_4_Type=1\n\
         Button_5_Name=empty\nButton_5_Action=\\r\\n\nButton_5_Type=1\n\
         Button_6_Name=vars\nButton_6_Action=echo $HOSTNAME \\\\$PATH\nButton_6_Type=1\n",
    );
    write_utf16(
        &xshell
            .join("QuickButton Files")
            .join("commands.sync-conflict-1-A.qbl"),
        "[QuickButton]\nButton_1_Name=x\nButton_1_Action=x\n",
    );

    let data = read_folder(&xshell, None);

    assert_eq!(data.command_sets.len(), 1);
    let set = &data.command_sets[0];
    assert_eq!(set.name, "commands");
    assert_eq!(set.not_text, 1);
    let commands: Vec<(&str, &str, bool)> = set
        .commands
        .iter()
        .map(|c| (c.name.as_str(), c.command.as_str(), c.run_on_click))
        .collect();
    assert_eq!(
        commands,
        [
            ("multi", "cd /var \\\n  && ls", true),
            ("df -h", "df -h", false),
            ("vars", "echo $HOSTNAME $PATH", false),
            ("tmux", "tmux ls", true),
        ]
    );
    assert!(set.commands[2].session_variables);
    assert!(!set.commands[0].session_variables);
}

#[test]
fn session_variables_are_only_the_three_xshell_fills_in() {
    assert!(names_session_variables("echo $HOSTNAME"));
    assert!(names_session_variables("cd $PATH/x"));
    assert!(!names_session_variables(r"echo \$PATH"));
    assert!(!names_session_variables("echo $PATHS $USER ${PATH}"));
}

#[test]
fn the_newest_xshell_is_the_default_folder_only_when_it_is_there() {
    // `default_folder` reads the real documents folder, so only its rule
    // is checked here: a folder without Sessions or quick commands is not
    // Xshell's.
    let root = xshell_folder();
    assert!(is_xshell_folder(&root.path().join("Xshell")));
    assert!(!is_xshell_folder(root.path()));
}
