//! 导入: hosts and snippets from another SSH client. Xshell is the one
//! there is: its sessions, with the passwords this Windows user saved, and
//! its quick command sets.

mod apply;
mod import_dialog;
mod xshell;
mod xshell_crypt;

pub use apply::{ImportReport, import};
pub use import_dialog::{XshellImportForm, open_xshell_import_dialog};
pub use xshell::{XshellData, default_folder, is_xshell_folder, read_folder};
pub use xshell_crypt::XshellKey;
