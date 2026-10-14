//! 跟随文件管理器: the folder Directory Opus or Explorer shows, for the SFTP
//! local list to go to, after listray-lite. Explorer tells its own through
//! the shell's windows, Opus through its `dopusrt.exe /info`. Elsewhere there
//! is neither, and nothing to follow.

use async_channel::Receiver;

/// The folder the file manager used last shows: the frontmost of their
/// windows. `None` without one, or where it shows no folder, as 此电脑.
/// Blocks for as long as `dopusrt` takes, some tens of milliseconds.
pub fn file_manager_folder() -> Option<String> {
    imp::frontmost_folder()
}

/// Each time a window of this program comes forward straight from a file
/// manager's, the folder that one shows, read then. Watched from a thread of
/// its own from the first call on; every call shares it.
pub fn file_manager_returns() -> Receiver<String> {
    imp::returns()
}

#[cfg(not(windows))]
mod imp {
    use async_channel::Receiver;

    pub fn frontmost_folder() -> Option<String> {
        None
    }

    /// Closed: nothing will come.
    pub fn returns() -> Receiver<String> {
        async_channel::unbounded().1
    }
}

#[cfg(windows)]
mod imp {
    use std::cell::{Cell, OnceCell};
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU32, Ordering};

    use async_channel::{Receiver, Sender};
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
        CoUninitialize, IServiceProvider,
    };
    use windows::Win32::System::Threading::{
        CREATE_NO_WINDOW, GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
    use windows::Win32::UI::Shell::{
        GPFIDL_DEFAULT, IFolderView, IPersistFolder2, IShellBrowser, IShellWindows,
        SHGetPathFromIDListEx, SID_STopLevelBrowser, ShellWindows,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, EVENT_SYSTEM_FOREGROUND, EnumWindows, FindWindowExW, GetClassNameW,
        GetMessageW, GetWindowThreadProcessId, IsWindowVisible, MSG, WINEVENT_OUTOFCONTEXT,
    };
    use windows::core::{BOOL, Interface, PWSTR, w};

    const EXPLORER: &str = "CabinetWClass";
    const OPUS: &str = "dopus.lister";

    pub fn frontmost_folder() -> Option<String> {
        // Explorer hands out its browsers only to a single-threaded
        // apartment, which a pool thread may not be: a thread of its own.
        std::thread::spawn(|| {
            let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            let folder = frontmost_manager().and_then(folder_of);
            unsafe { CoUninitialize() };
            folder
        })
        .join()
        .ok()
        .flatten()
    }

    static RETURNS: OnceLock<Receiver<String>> = OnceLock::new();

    pub fn returns() -> Receiver<String> {
        RETURNS
            .get_or_init(|| {
                let (sender, receiver) = async_channel::unbounded();
                // Without the thread the sender goes, and the channel is
                // closed: nothing to follow, as elsewhere.
                let _ = std::thread::Builder::new()
                    .name("file-manager-watch".into())
                    .spawn(move || watch(sender));
                receiver
            })
            .clone()
    }

    thread_local! {
        static SENDER: OnceCell<Sender<String>> = const { OnceCell::new() };
        /// The window in front before the one now.
        static PREVIOUS: Cell<HWND> = Cell::new(HWND::default());
    }

    /// The foreground changes, as they come, on a thread that waits for
    /// nothing else: an out-of-context hook calls back through its messages.
    fn watch(sender: Sender<String>) {
        SENDER.with(|cell| {
            let _ = cell.set(sender);
        });
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let hook = SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(on_foreground),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            );
            if hook.is_invalid() {
                return;
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                DispatchMessageW(&msg);
            }
        }
    }

    unsafe extern "system" fn on_foreground(
        _: HWINEVENTHOOK,
        _: u32,
        hwnd: HWND,
        _: i32,
        _: i32,
        _: u32,
        _: u32,
    ) {
        // Set before reading the folder: the calls to Explorer let further
        // changes in meanwhile.
        let previous = PREVIOUS.replace(hwnd);
        if !is_own(hwnd) || !is_file_manager(previous) {
            return;
        }
        if let Some(folder) = folder_of(previous) {
            SENDER.with(|cell| {
                if let Some(sender) = cell.get() {
                    let _ = sender.try_send(folder);
                }
            });
        }
    }

    fn is_own(hwnd: HWND) -> bool {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        pid == unsafe { GetCurrentProcessId() }
    }

    fn class_name(hwnd: HWND) -> String {
        let mut buf = [0u16; 256];
        let len = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
        String::from_utf16_lossy(&buf[..len])
    }

    fn is_file_manager(hwnd: HWND) -> bool {
        matches!(class_name(hwnd).as_str(), EXPLORER | OPUS)
    }

    fn folder_of(hwnd: HWND) -> Option<String> {
        match class_name(hwnd).as_str() {
            EXPLORER => explorer_folder(hwnd),
            OPUS => opus_folder(hwnd),
            _ => None,
        }
    }

    /// The first file manager's window in Z order: the one used last.
    fn frontmost_manager() -> Option<HWND> {
        unsafe extern "system" fn visit(hwnd: HWND, found: LPARAM) -> BOOL {
            if unsafe { IsWindowVisible(hwnd) }.as_bool() && is_file_manager(hwnd) {
                unsafe { *(found.0 as *mut Option<HWND>) = Some(hwnd) };
                return false.into();
            }
            true.into()
        }
        let mut found: Option<HWND> = None;
        // Stopped early, it reports a failure.
        let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut found as *mut _ as isize)) };
        found
    }

    /// The tab in front of an Explorer window: Windows 11's tabs share it,
    /// and the shell's windows list one browser each.
    fn explorer_folder(hwnd: HWND) -> Option<String> {
        // The tab in front is first among the window's children.
        let active_tab =
            unsafe { FindWindowExW(Some(hwnd), None, w!("ShellTabWindowClass"), None) }.ok();
        let windows: IShellWindows =
            unsafe { CoCreateInstance(&ShellWindows, None, CLSCTX_ALL) }.ok()?;
        for i in 0..unsafe { windows.Count() }.ok()? {
            let Ok(browser) = shell_browser(&windows, i) else {
                continue;
            };
            let Ok(owner) = (unsafe { browser.GetWindow() }) else {
                continue;
            };
            if Some(owner) == active_tab || owner == hwnd {
                return browser_folder(&browser);
            }
        }
        None
    }

    fn shell_browser(windows: &IShellWindows, index: i32) -> windows::core::Result<IShellBrowser> {
        let provider: IServiceProvider = unsafe { windows.Item(&VARIANT::from(index)) }?.cast()?;
        unsafe { provider.QueryService(&SID_STopLevelBrowser) }
    }

    fn browser_folder(browser: &IShellBrowser) -> Option<String> {
        unsafe {
            let view: IFolderView = browser.QueryActiveShellView().ok()?.cast().ok()?;
            let folder: IPersistFolder2 = view.GetFolder().ok()?;
            let pidl = folder.GetCurFolder().ok()?;
            let mut buf = vec![0u16; 32768];
            // 此电脑 and the like have no path, and fail here.
            let ok = SHGetPathFromIDListEx(pidl, &mut buf, GPFIDL_DEFAULT).as_bool();
            CoTaskMemFree(Some(pidl as *const _));
            let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            ok.then(|| String::from_utf16_lossy(&buf[..len]))
        }
    }

    /// Opus's `dopusrt.exe`, beside the lister's program, writes every tab's
    /// path to a file.
    fn opus_folder(lister: HWND) -> Option<String> {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dopusrt = process_path(lister)?.with_file_name("dopusrt.exe");
        let xml_path = std::env::temp_dir().join(format!(
            "shellrs-dopus-{}-{}.xml",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        Command::new(dopusrt)
            .raw_arg(format!("/info \"{}\",paths", xml_path.display()))
            .creation_flags(CREATE_NO_WINDOW.0)
            .status()
            .ok()?;
        let xml = std::fs::read_to_string(&xml_path).ok();
        let _ = std::fs::remove_file(&xml_path);
        active_opus_path(&xml?)
    }

    fn process_path(hwnd: HWND) -> Option<PathBuf> {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let result = unsafe {
            QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            )
        };
        unsafe { CloseHandle(process) }.ok()?;
        result.ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(
            &buf[..len as usize],
        )))
    }

    /// One `<path …>` line a tab: `active_lister="1"` is the lister used
    /// last, `tab_state="1"` the tab in front of its source side.
    fn active_opus_path(xml: &str) -> Option<String> {
        xml.lines()
            .map(str::trim)
            .filter(|line| line.starts_with("<path "))
            .filter(|line| {
                line.contains(r#"active_lister="1""#) && line.contains(r#"tab_state="1""#)
            })
            .find_map(|line| {
                let start = line.find('>')? + 1;
                let end = line.rfind("</path>")?;
                Some(unescape_xml(&line[start..end]))
            })
    }

    fn unescape_xml(text: &str) -> String {
        text.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&")
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// What `dopusrt.exe /info` writes for a dual lister with tabs, and
        /// a second lister.
        const DUAL_PANE: &str = "\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<results command=\"paths\" result=\"1\">
	<path active_lister=\"1\" display_path=\"G:\\workspace\" lister=\"0xd08ba\" side=\"1\" tab=\"0x140ace\">G:\\workspace</path>
	<path active_lister=\"1\" active_tab=\"1\" display_path=\"F:\\workspace\" lister=\"0xd08ba\" side=\"1\" tab=\"0x60f48\" tab_state=\"2\">F:\\workspace</path>
	<path active_lister=\"1\" active_tab=\"2\" display_path=\"C:\\用户\\tester\\下载\" lister=\"0xd08ba\" side=\"2\" tab=\"0x140696\" tab_state=\"1\">C:\\Users\\tester\\Downloads</path>
	<path lister=\"0x2\" active_tab=\"1\" side=\"1\" tab=\"0x9\" tab_state=\"1\">D:\\other lister</path>
</results>";

        #[test]
        fn opus_gives_the_source_tab_of_the_lister_used_last() {
            assert_eq!(
                active_opus_path(DUAL_PANE).as_deref(),
                Some(r"C:\Users\tester\Downloads")
            );
            assert_eq!(
                active_opus_path("<results command=\"paths\" result=\"1\">\n</results>"),
                None
            );
        }

        #[test]
        fn opus_paths_are_unescaped() {
            let xml = r#"<path active_lister="1" tab_state="1">D:\A &amp; B\&lt;x&gt; &apos;q&apos;</path>"#;
            assert_eq!(active_opus_path(xml).as_deref(), Some(r"D:\A & B\<x> 'q'"));
        }

        /// Opens a real Explorer window on a folder of its own, reads it back
        /// as the frontmost file manager's, and closes it. On the desktop,
        /// so not with the rest: `cargo test -- --ignored file_manager`.
        #[test]
        #[ignore = "opens an Explorer window"]
        fn explorer_tells_the_folder_it_shows() {
            use std::time::{Duration, Instant};
            use windows::Win32::Foundation::WPARAM;
            use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};

            let folder = tempfile::tempdir().unwrap();
            let path = dunce::canonicalize(folder.path()).unwrap();
            Command::new("explorer.exe").arg(&path).status().unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            let window = loop {
                if let Some(hwnd) = frontmost_manager()
                    && frontmost_folder().as_deref() == path.to_str()
                {
                    break Some(hwnd);
                }
                if Instant::now() > deadline {
                    break None;
                }
                std::thread::sleep(Duration::from_millis(200));
            };
            let window = window.expect("Explorer did not show the folder in front");
            unsafe { PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)) }.unwrap();
        }
    }
}
