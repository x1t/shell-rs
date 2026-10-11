//! 云同步 as the workspace runs it: rounds when the window comes forward,
//! the settings page's commands, and what a round brings, after the tabs of
//! the hosts it deletes are closed.

use gpui_kit::*;

use crate::app::{SyncLogOut, SyncNow};
use crate::sync::{SyncServices, Syncer, SyncerEvent};

use super::Workspace;

impl Workspace {
    /// Sync through `services` from now on. `Workspace::new` hands in the
    /// real ones; the UI tests a server kept in memory.
    pub fn set_sync_services(
        &mut self,
        services: SyncServices,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.syncer
            .update(cx, |syncer, cx| syncer.set_services(services, cx));
        // What another device changed arrives when this one is used again.
        let subscription = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.syncer.update(cx, |syncer, cx| syncer.sync_now(cx));
            }
        });
        self._subscriptions.push(subscription);
    }

    pub fn syncer(&self) -> &Entity<Syncer> {
        &self.syncer
    }

    pub(super) fn on_syncer_event(
        &mut self,
        event: &SyncerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SyncerEvent::Arrived => {
                // Deleted on another device: as the CLI's `--force`, its
                // tabs go with it.
                for host in self.syncer.read(cx).removed_hosts(cx) {
                    self.close_host_tabs(host, window, cx);
                }
                self.syncer
                    .update(cx, |syncer, cx| syncer.apply_arrival(cx));
            }
        }
    }

    pub(super) fn on_sync_now(&mut self, _: &SyncNow, _: &mut Window, cx: &mut Context<Self>) {
        self.syncer.update(cx, |syncer, cx| syncer.sync_now(cx));
    }

    pub(super) fn on_sync_log_out(
        &mut self,
        _: &SyncLogOut,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.syncer.update(cx, |syncer, cx| syncer.log_out(cx));
    }
}
