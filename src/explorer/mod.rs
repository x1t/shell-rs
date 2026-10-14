//! The SFTP tab: local and remote file browsing, transfers and their queue.

mod explorer_panel;
mod external_edit;
mod external_editor;
mod file_dialogs;
mod file_edit;
mod file_icons;
mod file_icons_table;
mod file_listing;
mod file_manager;
mod file_pane;
mod history;
mod image_preview;
mod model;
mod open_directory_dialog;
mod pane_menu;
mod pane_operations;
mod path_label;
mod preview;
mod properties_dialog;
mod queue_panel;
mod selection;
mod transfer_queue;

pub use explorer_panel::{ExplorerPanel, ExplorerPanelEvent, ExplorerStatus};
pub use external_edit::EXTERNAL_LIMIT;
pub use external_editor::{EditorLauncher, ExternalEditor, ExternalEditors, SystemEditorLauncher};
pub use file_edit::FileLocation;
pub use file_icons::{FileIcon, FileIconKind, FileIconStyle};
pub use file_icons_table::FILE_ICON_ASSETS;
pub use file_listing::FileListing;
pub use file_manager::{file_manager_folder, file_manager_returns};
pub use file_pane::{FilePane, FilePaneEvent, PaneSide};
pub use history::{LoadIntent, NavigationHistory};
pub use model::*;
pub use pane_menu::CopyTargets;
pub use pane_operations::PaneOperation;
pub use preview::{IMAGE_LIMIT, PreviewKind};
pub use selection::{ClickMode, CursorMotion, Selection};
pub use transfer_queue::{
    QueueEntry, QueueId, QueueState, Removal, TransferJob, TransferQueue, percent,
};

mod transfer_dialog;
pub use transfer_dialog::confirm_close_transfer;
