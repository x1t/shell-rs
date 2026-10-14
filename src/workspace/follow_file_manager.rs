//! 跟随文件管理器 as the workspace runs it: coming back from Directory Opus
//! or Explorer, the SFTP tab in front takes the folder it showed into its
//! local list.

use std::time::Duration;

use async_channel::Receiver;
use gpui_kit::*;

use crate::app::CenterTab;
use crate::explorer::{FollowFileManager, LoadIntent};

use super::Workspace;

/// How often the folders the watch reads are taken: worker threads do not
/// wake the UI, which looks, as for the CLI.
const POLL: Duration = Duration::from_millis(100);

impl Workspace {
    /// Follow the folders `returns` brings, until it closes. `Workspace::new`
    /// hands in the system's watch; the UI tests a channel of their own.
    pub fn follow_file_manager(
        &self,
        returns: Receiver<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                // Only the latest counts.
                let mut latest = None;
                while let Ok(folder) = returns.try_recv() {
                    latest = Some(folder);
                }
                let Some(folder) = latest else {
                    if returns.is_closed() {
                        break;
                    }
                    continue;
                };
                let followed = this.update_in(cx, |this, window, cx| {
                    this.follow_folder(folder, window, cx)
                });
                if followed.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn follow_folder(&mut self, folder: String, window: &mut Window, cx: &mut Context<Self>) {
        if !FollowFileManager::get(cx) {
            return;
        }
        let Some(CenterTab::Explorer(id)) = self.active_tab else {
            return;
        };
        let Some(panel) = self.explorers.get(&id).cloned() else {
            return;
        };
        if panel.read(cx).local().read(cx).path() == folder {
            return;
        }
        panel.update(cx, |panel, cx| {
            panel.navigate(false, folder, LoadIntent::Visit, window, cx)
        });
    }
}
