//! The syncer: the account, the session, and when rounds run.
//!
//! The network, the keychain and the KDF run on one `shellrs-sync` thread,
//! one job at a time; the entity collects what comes back on a timer while
//! a job runs, as the updater does, so no worker ever wakes the window.
//!
//! A round starts when the user asks, when ShellRS starts or comes to the
//! front, and two seconds after a change here. There is no timer of its
//! own: what another device changed arrives with the next of those.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use async_channel::{Receiver, TryRecvError};
use gpui_kit::*;
use zeroize::Zeroizing;

use super::api::{ApiError, HttpSyncApi, SyncApi, normalized_server};
use super::apply::{Changes, Targets, apply};
use super::engine::{Round, RoundResult, SyncError, device_name, log_in};
use super::local::LocalSnapshot;
use super::state::{Account, StoredSession, SyncState};
use crate::host::{HostId, HostStore};
use crate::i18n::t;
use crate::secrets::SharedSecretStore;
use crate::settings::SettingsStore;

const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// How long after a change here its round starts: edits come in bursts.
const SETTLE: Duration = Duration::from_secs(2);

/// What the syncer works with. Production has [`SyncServices::system`];
/// the UI tests hand in a server kept in memory.
#[derive(Clone)]
pub struct SyncServices {
    pub connect: Arc<dyn Fn() -> Result<Arc<dyn SyncApi>, ApiError> + Send + Sync>,
    pub secrets: SharedSecretStore,
    /// `sync.json`; `None` keeps the state in memory.
    pub state_path: Option<PathBuf>,
    pub known_hosts: Option<PathBuf>,
}

impl SyncServices {
    pub fn system(secrets: SharedSecretStore) -> Self {
        Self {
            connect: Arc::new(|| HttpSyncApi::new().map(|api| Arc::new(api) as Arc<dyn SyncApi>)),
            secrets,
            state_path: Some(crate::app::sync_state_path()),
            known_hosts: Some(crate::app::data_dir().join("known_hosts")),
        }
    }

    pub fn with_api(api: Arc<dyn SyncApi>, secrets: SharedSecretStore) -> Self {
        Self {
            connect: Arc::new(move || Ok(api.clone())),
            secrets,
            state_path: None,
            known_hosts: None,
        }
    }
}

/// Where syncing stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncStatus {
    /// No services: the UI tests that do not sync.
    Off,
    SignedOut,
    /// Reading the session, or logging in.
    Starting,
    /// The account is known but has no session: log in again.
    NeedsLogin(SharedString),
    Syncing,
    Synced,
    /// The last round failed; the next one tries again.
    Failed(SharedString),
}

/// What the title bar's 立即同步 shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncBadge {
    pub busy: bool,
    pub trouble: bool,
    /// Where syncing stands, for the tooltip.
    pub label: SharedString,
}

/// A round's changes, waiting for the workspace to close the tabs of the
/// hosts it deletes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncerEvent {
    Arrived,
}

enum Job {
    LoadSession(Account),
    LogIn {
        account: Account,
        password: Zeroizing<String>,
    },
    Round {
        session: Arc<StoredSession>,
        account: Account,
        snapshot: LocalSnapshot,
        state: SyncState,
    },
    LogOut {
        account: Account,
        session: Option<Arc<StoredSession>>,
    },
}

enum Done {
    Session(Result<Option<StoredSession>, SyncError>),
    LoggedIn(Account, Result<StoredSession, SyncError>),
    Round(Result<RoundResult, SyncError>),
    LoggedOut,
}

/// A finished round, its changes not yet carried out.
struct Arrival {
    result: RoundResult,
    snapshot: LocalSnapshot,
}

pub struct Syncer {
    services: Option<SyncServices>,
    store: Entity<HostStore>,
    settings: Entity<SettingsStore>,
    state: SyncState,
    session: Option<Arc<StoredSession>>,
    status: SyncStatus,
    jobs: Option<mpsc::Sender<Job>>,
    done: Option<Receiver<Done>>,
    /// Jobs sent and not answered yet.
    running: usize,
    polling: bool,
    /// Another round is wanted once this one is done.
    again: bool,
    arrival: Option<Arrival>,
    /// What this device held when the running round started.
    round_snapshot: Option<LocalSnapshot>,
    /// Changes being carried out, which are not changes made here.
    applying: bool,
    settle: Option<Task<()>>,
    _subscriptions: [Subscription; 2],
}

impl EventEmitter<SyncerEvent> for Syncer {}

impl Syncer {
    pub fn new(
        store: Entity<HostStore>,
        settings: Entity<SettingsStore>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = [
            cx.observe(&store, |this, _, cx| this.changed_here(cx)),
            cx.observe(&settings, |this, _, cx| this.changed_here(cx)),
        ];
        Self {
            services: None,
            store,
            settings,
            state: SyncState::default(),
            session: None,
            status: SyncStatus::Off,
            jobs: None,
            done: None,
            running: 0,
            polling: false,
            again: false,
            arrival: None,
            round_snapshot: None,
            applying: false,
            settle: None,
            _subscriptions: subscriptions,
        }
    }

    /// Start: read the account, then the session from the keychain, then
    /// sync.
    pub fn set_services(&mut self, services: SyncServices, cx: &mut Context<Self>) {
        self.state = services
            .state_path
            .as_deref()
            .map(SyncState::load)
            .unwrap_or_default();
        self.services = Some(services);
        match self.state.account.clone() {
            Some(account) => {
                self.status = SyncStatus::Starting;
                self.send(Job::LoadSession(account), cx);
            }
            None => self.status = SyncStatus::SignedOut,
        }
        cx.notify();
    }

    pub fn status(&self) -> &SyncStatus {
        &self.status
    }

    pub fn account(&self) -> Option<&Account> {
        self.state.account.as_ref()
    }

    /// When the last round finished, in seconds since the Unix epoch.
    pub fn last_synced(&self) -> Option<u64> {
        self.state.last_synced
    }

    pub fn is_signed_in(&self) -> bool {
        self.session.is_some()
    }

    /// Where syncing stands, in words.
    pub fn status_text(&self) -> SharedString {
        match &self.status {
            SyncStatus::Off => t!("sync.status.off"),
            SyncStatus::SignedOut => t!("sync.status.signed_out"),
            SyncStatus::Starting => t!("sync.status.starting"),
            SyncStatus::Syncing => t!("sync.status.syncing"),
            SyncStatus::Synced => synced_text(self.last_synced()),
            SyncStatus::NeedsLogin(message) | SyncStatus::Failed(message) => message.clone(),
        }
    }

    pub fn has_failed(&self) -> bool {
        matches!(
            self.status,
            SyncStatus::NeedsLogin(_) | SyncStatus::Failed(_)
        )
    }

    /// The title bar's 立即同步, while logged in.
    pub fn badge(&self) -> Option<SyncBadge> {
        self.session.as_ref()?;
        Some(SyncBadge {
            busy: matches!(self.status, SyncStatus::Syncing),
            trouble: self.has_failed(),
            label: self.status_text(),
        })
    }

    /// Log in to `server` as `username`. Refused at once, saying why, when
    /// the address is not one to send a password to.
    pub fn log_in(
        &mut self,
        server: &str,
        username: &str,
        password: &str,
        cx: &mut Context<Self>,
    ) -> Result<(), SharedString> {
        let server = normalized_server(server).ok_or_else(|| t!("sync.error.bad_server"))?;
        let username = username.trim();
        if username.is_empty() || password.is_empty() {
            return Err(t!("sync.error.missing_fields"));
        }
        let account = Account {
            server,
            username: username.to_owned(),
        };
        self.status = SyncStatus::Starting;
        self.send(
            Job::LogIn {
                account,
                password: Zeroizing::new(password.to_owned()),
            },
            cx,
        );
        cx.notify();
        Ok(())
    }

    /// Forget the session here and on the server. What was synced stays.
    pub fn log_out(&mut self, cx: &mut Context<Self>) {
        let Some(account) = self.state.account.take() else {
            return;
        };
        let session = self.session.take();
        self.state = SyncState::default();
        self.save_state();
        self.status = SyncStatus::SignedOut;
        self.send(Job::LogOut { account, session }, cx);
        cx.notify();
    }

    /// Run a round now, or right after the one running.
    pub fn sync_now(&mut self, cx: &mut Context<Self>) {
        let (Some(session), Some(account)) = (self.session.clone(), self.state.account.clone())
        else {
            return;
        };
        if self.running > 0 || self.arrival.is_some() {
            self.again = true;
            return;
        }
        self.settle = None;
        let snapshot = self.snapshot(cx);
        self.round_snapshot = Some(snapshot.clone());
        self.status = SyncStatus::Syncing;
        self.send(
            Job::Round {
                session,
                account,
                snapshot,
                state: self.state.clone(),
            },
            cx,
        );
        cx.notify();
    }

    /// Something changed here: sync once the edits settle.
    fn changed_here(&mut self, cx: &mut Context<Self>) {
        if self.applying || self.session.is_none() {
            return;
        }
        self.settle = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            _ = this.update(cx, |this, cx| this.sync_now(cx));
        }));
    }

    fn snapshot(&self, cx: &App) -> LocalSnapshot {
        let settings = serde_json::to_value(self.settings.read(cx).settings())
            .unwrap_or(serde_json::Value::Null);
        let known_hosts = self
            .services
            .as_ref()
            .and_then(|services| services.known_hosts.clone());
        LocalSnapshot::of(self.store.read(cx), settings, known_hosts)
    }

    fn save_state(&self) {
        if let Some(path) = self.services.as_ref().and_then(|s| s.state_path.as_deref())
            && let Err(error) = self.state.save(path)
        {
            eprintln!("shellrs: cannot save {}: {error}", path.display());
        }
    }

    fn send(&mut self, job: Job, cx: &mut Context<Self>) {
        let Some(services) = self.services.clone() else {
            return;
        };
        if self.jobs.is_none() {
            let (jobs, receiver) = mpsc::channel();
            let (sender, done) = async_channel::unbounded();
            let spawned = std::thread::Builder::new()
                .name("shellrs-sync".into())
                .spawn(move || work(services, receiver, sender));
            if let Err(error) = spawned {
                self.status = SyncStatus::Failed(error.to_string().into());
                return;
            }
            self.jobs = Some(jobs);
            self.done = Some(done);
        }
        if let Some(jobs) = &self.jobs
            && jobs.send(job).is_ok()
        {
            self.running += 1;
            self.poll(cx);
        }
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        if self.polling {
            return;
        }
        self.polling = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                let running = this.update(cx, |this, cx| {
                    this.collect(cx);
                    this.polling = this.running > 0;
                    this.polling
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn collect(&mut self, cx: &mut Context<Self>) {
        while let Some(done) = &self.done {
            match done.try_recv() {
                Ok(done) => {
                    self.running = self.running.saturating_sub(1);
                    self.on_done(done, cx);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Closed) => {
                    self.running = 0;
                    self.jobs = None;
                    self.done = None;
                }
            }
        }
    }

    fn on_done(&mut self, done: Done, cx: &mut Context<Self>) {
        match done {
            Done::Session(Ok(Some(session))) => self.signed_in(session, cx),
            Done::Session(Ok(None)) => {
                self.status = SyncStatus::NeedsLogin(t!("sync.status.log_in_again"));
            }
            Done::Session(Err(error)) | Done::Round(Err(error)) => self.failed(error),
            Done::LoggedIn(account, Ok(session)) => {
                self.state.signed_in(account);
                self.save_state();
                self.signed_in(session, cx);
            }
            Done::LoggedIn(_, Err(error)) => {
                self.status = match self.state.account {
                    Some(_) => SyncStatus::NeedsLogin(login_error(&error)),
                    None => SyncStatus::Failed(login_error(&error)),
                };
            }
            Done::Round(Ok(result)) => self.arrived(result, cx),
            Done::LoggedOut => {}
        }
        cx.notify();
    }

    fn signed_in(&mut self, session: StoredSession, cx: &mut Context<Self>) {
        self.session = Some(Arc::new(session));
        self.status = SyncStatus::Synced;
        self.sync_now(cx);
    }

    fn failed(&mut self, error: SyncError) {
        let message = round_error(&error);
        self.status = match error {
            SyncError::Api(ApiError::Unauthorized) | SyncError::WrongKey => {
                self.session = None;
                SyncStatus::NeedsLogin(message)
            }
            _ => SyncStatus::Failed(message),
        };
    }

    fn arrived(&mut self, result: RoundResult, cx: &mut Context<Self>) {
        let snapshot = self.round_snapshot.take();
        match snapshot {
            Some(snapshot) if !result.rekey.is_empty() || !result.apply.is_empty() => {
                self.arrival = Some(Arrival { result, snapshot });
                cx.emit(SyncerEvent::Arrived);
            }
            _ => self.finish(result, cx),
        }
    }

    /// The hosts the changes waiting to be carried out delete, those in
    /// deleted groups included.
    pub fn removed_hosts(&self, cx: &App) -> Vec<HostId> {
        self.arrival.as_ref().map_or_else(Vec::new, |arrival| {
            Changes::new(arrival.result.apply.clone()).removed_hosts(self.store.read(cx))
        })
    }

    /// Carry out what the round brought, but for the items changed here
    /// while it ran: those go up with the next round instead.
    pub fn apply_arrival(&mut self, cx: &mut Context<Self>) {
        let Some(Arrival {
            mut result,
            snapshot,
        }) = self.arrival.take()
        else {
            return;
        };
        let now = self.snapshot(cx);
        let here: HashMap<&str, &str> = result
            .rekey
            .iter()
            .map(|(local, server)| (server.as_str(), local.as_str()))
            .collect();
        let mut skipped = Vec::new();
        result.apply.retain(|(id, _)| {
            let local = here.get(id.as_str()).copied().unwrap_or(id);
            let changed = snapshot.item(local).map(|item| &item.payload)
                != now.item(local).map(|item| &item.payload);
            if changed {
                skipped.push(id.clone());
            }
            !changed
        });
        for id in skipped {
            if let Some(base) = result.base.get_mut(&id) {
                base.fingerprint.clear();
            }
        }
        self.applying = true;
        self.store.update(cx, |store, cx| {
            for (local, server) in &result.rekey {
                store.rename_sync_id(local, server, cx);
            }
        });
        let known_hosts = self
            .services
            .as_ref()
            .and_then(|services| services.known_hosts.clone());
        let targets = Targets {
            store: &self.store,
            settings: &self.settings,
            known_hosts: known_hosts.as_deref(),
        };
        apply(
            Changes::new(std::mem::take(&mut result.apply)),
            &targets,
            cx,
        );
        // After the notifications the changes sent out.
        let this = cx.weak_entity();
        cx.defer(move |cx| {
            _ = this.update(cx, |this, _| this.applying = false);
        });
        self.finish(result, cx);
    }

    fn finish(&mut self, result: RoundResult, cx: &mut Context<Self>) {
        self.state.base = result.base;
        self.state.cursor = result.cursor;
        self.state.last_synced = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|elapsed| elapsed.as_secs());
        self.save_state();
        self.status = SyncStatus::Synced;
        cx.notify();
        if std::mem::take(&mut self.again) {
            self.sync_now(cx);
        }
    }
}

/// The worker: one job at a time, the server reached through one client.
fn work(services: SyncServices, jobs: mpsc::Receiver<Job>, done: async_channel::Sender<Done>) {
    let mut api: Option<Arc<dyn SyncApi>> = None;
    for job in jobs {
        let client = match &api {
            Some(api) => Ok(api.clone()),
            None => (services.connect)(),
        };
        let answer = match client {
            Ok(client) => {
                api = Some(client.clone());
                run_job(client.as_ref(), &services, job)
            }
            Err(error) => failed_job(job, SyncError::Api(error)),
        };
        if done.send_blocking(answer).is_err() {
            return;
        }
    }
}

fn failed_job(job: Job, error: SyncError) -> Done {
    match job {
        Job::LoadSession(_) => Done::Session(Err(error)),
        Job::LogIn { account, .. } => Done::LoggedIn(account, Err(error)),
        Job::Round { .. } => Done::Round(Err(error)),
        Job::LogOut { .. } => Done::LoggedOut,
    }
}

fn run_job(api: &dyn SyncApi, services: &SyncServices, job: Job) -> Done {
    let secrets = services.secrets.as_ref();
    match job {
        Job::LoadSession(account) => Done::Session(
            StoredSession::load(secrets, &account)
                .map_err(|error| SyncError::Keychain(error.to_string())),
        ),
        Job::LogIn { account, password } => {
            let result = log_in(
                api,
                &account.server,
                &account.username,
                &password,
                &device_name(),
            )
            .and_then(|session| {
                session
                    .save(secrets, &account)
                    .map_err(|error| SyncError::Keychain(error.to_string()))?;
                Ok(session)
            });
            Done::LoggedIn(account, result)
        }
        Job::Round {
            session,
            account,
            snapshot,
            state,
        } => {
            let (local, unreadable) = snapshot.read(secrets);
            let round = Round {
                api,
                session: &session.session(&account),
                vault: &session.vault_key,
            };
            Done::Round(round.run(local, &unreadable, state.base, state.cursor))
        }
        Job::LogOut { account, session } => {
            if let Some(session) = session {
                _ = api.logout(&session.session(&account));
            }
            _ = secrets.delete(&account.secret());
            Done::LoggedOut
        }
    }
}

/// 已同步 and when, in this computer's time.
fn synced_text(at: Option<u64>) -> SharedString {
    let time = at
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds as i64, 0))
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%m-%d %H:%M")
                .to_string()
        });
    match time {
        Some(time) => t!("sync.status.synced_at", time = time),
        None => t!("sync.status.synced"),
    }
}

fn login_error(error: &SyncError) -> SharedString {
    match error {
        SyncError::Api(ApiError::Unauthorized) => t!("sync.error.wrong_password"),
        _ => round_error(error),
    }
}

fn round_error(error: &SyncError) -> SharedString {
    match error {
        SyncError::Api(ApiError::Network(reason)) => t!("sync.error.network", reason = reason),
        SyncError::Api(ApiError::Unauthorized) => t!("sync.status.log_in_again"),
        SyncError::Api(ApiError::TooManyAttempts) => t!("sync.error.too_many_attempts"),
        SyncError::Api(ApiError::Server(status)) => t!("sync.error.server", status = status),
        SyncError::Api(ApiError::NotSyncServer) => t!("sync.error.not_sync_server"),
        SyncError::UnsafeServer => t!("sync.error.unsafe_server"),
        SyncError::WrongKey => t!("sync.error.wrong_key"),
        SyncError::Keychain(reason) => t!("sync.error.keychain", reason = reason),
    }
}
