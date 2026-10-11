//! 用 VS Code / Zed 打开: the editors on this computer that a file can be
//! opened in from the SFTP lists, beside ShellRS's own. They are looked for
//! once, where their installers put them, then on the PATH; the menus offer
//! only those found.
//!
//! A remote file is copied here to open, and its copy is watched: each save
//! there goes back over the SFTP tab's connection (`external_edit`).

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use gpui_kit::{App, Global, SharedString};

/// An editor of another maker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExternalEditor {
    VsCode,
    Zed,
}

impl ExternalEditor {
    pub const ALL: [Self; 2] = [Self::VsCode, Self::Zed];

    /// Its name, the same in every language.
    pub fn name(self) -> SharedString {
        match self {
            Self::VsCode => "VS Code".into(),
            Self::Zed => "Zed".into(),
        }
    }

    /// Where it may be, in the order to look: where its installer puts it,
    /// then its command on the PATH. `env` reads an environment variable.
    fn candidates(self, env: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
        let under = |var: &str, rest: &[&str]| {
            env(var).map(|base| {
                rest.iter()
                    .fold(PathBuf::from(base), |path, part| path.join(part))
            })
        };
        let mut places: Vec<PathBuf> = Vec::new();
        if cfg!(windows) {
            // The command-line entry, not the window's `Zed.exe`, hands the
            // file to a Zed already running; VS Code's own does that too.
            places.extend(
                match self {
                    Self::VsCode => [
                        under(
                            "LOCALAPPDATA",
                            &["Programs", "Microsoft VS Code", "Code.exe"],
                        ),
                        under("ProgramFiles", &["Microsoft VS Code", "Code.exe"]),
                    ],
                    Self::Zed => [
                        under("LOCALAPPDATA", &["Programs", "Zed", "bin", "zed.exe"]),
                        under("ProgramFiles", &["Zed", "bin", "zed.exe"]),
                    ],
                }
                .into_iter()
                .flatten(),
            );
        } else if cfg!(target_os = "macos") {
            places.push(PathBuf::from(match self {
                Self::VsCode => {
                    "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code"
                }
                Self::Zed => "/Applications/Zed.app/Contents/MacOS/cli",
            }));
        }
        let commands: &[&str] = match self {
            Self::VsCode => &["code"],
            // Some Linux packages call it zeditor.
            Self::Zed => &["zed", "zeditor"],
        };
        if let Some(path) = env("PATH") {
            for dir in std::env::split_paths(&path) {
                for command in commands {
                    places.push(dir.join(command));
                    if cfg!(windows) {
                        places.push(dir.join(format!("{command}.exe")));
                    }
                }
            }
        }
        places
    }
}

/// Finds the editors and starts them. Behind a trait so that UI tests never
/// start one.
pub trait EditorLauncher {
    /// The editors there are, in the menus' order.
    fn installed(&self) -> Vec<ExternalEditor>;
    /// Open `file` in `editor`.
    fn open(&self, editor: ExternalEditor, file: &Path) -> io::Result<()>;
}

/// The editors on this computer, found when it is made.
pub struct SystemEditorLauncher {
    found: Vec<(ExternalEditor, PathBuf)>,
}

impl SystemEditorLauncher {
    pub fn find() -> Self {
        let env = |var: &str| std::env::var_os(var);
        let found = ExternalEditor::ALL
            .into_iter()
            .filter_map(|editor| {
                let program = editor
                    .candidates(&env)
                    .into_iter()
                    .find(|path| path.is_file())?;
                Some((editor, program))
            })
            .collect();
        Self { found }
    }
}

impl EditorLauncher for SystemEditorLauncher {
    fn installed(&self) -> Vec<ExternalEditor> {
        self.found.iter().map(|(editor, _)| *editor).collect()
    }

    fn open(&self, editor: ExternalEditor, file: &Path) -> io::Result<()> {
        let Some((_, program)) = self.found.iter().find(|(each, _)| *each == editor) else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                editor.name().to_string(),
            ));
        };
        let mut command = Command::new(program);
        command.arg(file);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            // A GUI app starting a console program would flash its window.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        // Left to run on its own; nothing waits for it.
        command.spawn().map(drop)
    }
}

/// The launcher the SFTP lists use. Not there in UI tests unless a test
/// sets one: the menus offer no other editor then.
#[derive(Clone)]
pub struct ExternalEditors(pub Arc<dyn EditorLauncher>);

impl Global for ExternalEditors {}

impl ExternalEditors {
    /// The editors there are; none without a launcher.
    pub fn installed(cx: &App) -> Vec<ExternalEditor> {
        cx.try_global::<Self>()
            .map(|editors| editors.0.installed())
            .unwrap_or_default()
    }

    pub fn open(editor: ExternalEditor, file: &Path, cx: &App) -> io::Result<()> {
        match cx.try_global::<Self>() {
            Some(editors) => editors.0.open(editor, file),
            None => Err(io::Error::new(
                io::ErrorKind::NotFound,
                editor.name().to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        move |key| {
            vars.iter()
                .find(|(each, _)| each == key)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn the_path_is_looked_through_last() {
        let separator = if cfg!(windows) { ";" } else { ":" };
        let path = format!("first{separator}second");
        let list = [("PATH", path.as_str()), ("LOCALAPPDATA", "local")];
        let vars = env(&list);
        let places = ExternalEditor::Zed.candidates(&vars);
        let on_path: Vec<PathBuf> = places
            .iter()
            .filter(|place| place.starts_with("first") || place.starts_with("second"))
            .cloned()
            .collect();
        assert_eq!(on_path.first(), Some(&Path::new("first").join("zed")));
        assert!(on_path.contains(&Path::new("second").join("zeditor")));
        // The installer's place, where there is one, comes before them.
        if cfg!(windows) || cfg!(target_os = "macos") {
            assert!(!places[0].starts_with("first"));
        }
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_the_installers_places_are_under_the_user_and_program_files() {
        let list = [("LOCALAPPDATA", r"C:\Users\me\AppData\Local")];
        let vars = env(&list);
        assert_eq!(
            ExternalEditor::VsCode.candidates(&vars),
            [PathBuf::from(
                r"C:\Users\me\AppData\Local\Programs\Microsoft VS Code\Code.exe"
            )]
        );
        assert_eq!(
            ExternalEditor::Zed.candidates(&vars),
            [PathBuf::from(
                r"C:\Users\me\AppData\Local\Programs\Zed\bin\zed.exe"
            )]
        );
    }

    #[test]
    fn an_editor_not_found_is_not_offered_and_does_not_open() {
        let launcher = SystemEditorLauncher { found: Vec::new() };
        assert!(launcher.installed().is_empty());
        let error = launcher
            .open(ExternalEditor::VsCode, Path::new("notes.md"))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }
}
