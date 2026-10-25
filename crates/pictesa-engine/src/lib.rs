//! Pictesa engine: everything except HTTP. The server, the CLI and the MCP endpoint call into
//! [`Engine`], so a generation behaves the same however it was requested.

pub mod auth;
pub mod billing;
pub mod catalog;
pub mod characters;
pub mod creds;
pub mod crypto;
pub mod enhance;
pub mod files;
pub mod jobs;
pub mod leaderboard;
pub mod netguard;
pub mod presets;
pub mod queue;
pub mod recipes;
pub mod store;
pub mod types;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    SqlitePool,
};
use tokio::sync::{broadcast, Notify};
use tokio_util::sync::CancellationToken;

pub use types::*;

/// The single user of local mode.
pub const LOCAL_USER: &str = "local";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// One person on their own machine. No accounts.
    Local,
    /// Many users with accounts (pictesa.com-style hosting).
    Hosted,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub mode: Mode,
    /// Extra folders scanned for presets (`*.json`).
    pub preset_dirs: Vec<PathBuf>,
    /// Public origin used to build absolute file URLs for providers that need them (hosted mode).
    pub public_url: Option<String>,
}

impl Config {
    pub fn default_data_dir() -> PathBuf {
        if let Ok(dir) = std::env::var("PICTESA_DATA_DIR") {
            return PathBuf::from(dir);
        }
        dirs::data_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".local/share")).join("pictesa")
    }
    pub fn local(data_dir: impl Into<PathBuf>) -> Self {
        Self { data_dir: data_dir.into(), mode: Mode::Local, preset_dirs: Vec::new(), public_url: None }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("{0}")]
    NotFound(String),
    #[error("{message}")]
    Validation { message: String, field: Option<String> },
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    TooMany(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl EngineError {
    pub fn invalid(message: impl Into<String>, field: Option<&str>) -> Self {
        Self::Validation { message: message.into(), field: field.map(String::from) }
    }
    pub fn not_found(what: &str) -> Self {
        Self::NotFound(format!("{what} not found"))
    }
}

impl From<sqlx::Error> for EngineError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => Self::NotFound("not found".into()),
            other => Self::Internal(other.into()),
        }
    }
}

pub type Result<T, E = EngineError> = std::result::Result<T, E>;

/// A change pushed to `/api/events` subscribers.
#[derive(Debug, Clone)]
pub struct Event {
    pub user_id: String,
    pub kind: &'static str,
    pub data: serde_json::Value,
}

pub(crate) struct Inner {
    pub cfg: Config,
    pub db: SqlitePool,
    pub files: files::FileStore,
    pub catalog: catalog::Catalog,
    pub crypto: crypto::Crypto,
    pub presets: Vec<Preset>,
    pub recipes: Vec<recipes::Recipe>,
    pub billing: billing::BillingConfig,
    /// Client for provider API calls.
    pub http: reqwest::Client,
    /// Client for downloading outputs (long timeouts, follows redirects).
    pub download: reqwest::Client,
    pub events: broadcast::Sender<Event>,
    pub wake: Notify,
    pub cancels: Mutex<HashMap<String, CancellationToken>>,
    /// Latest live preview per running job: (bytes, MIME type, version).
    pub previews: Mutex<HashMap<String, (Vec<u8>, String, u64)>>,
    pub running: Mutex<HashMap<String, usize>>,
    /// This process's id, written on the jobs it runs.
    pub instance: String,
    /// Same clients, but refusing private addresses and unchecked redirects: used in hosted mode
    /// for providers whose endpoint a user chose.
    pub http_guarded: reqwest::Client,
    pub download_guarded: reqwest::Client,
    /// Scoped engines (a CLI run next to a server) only process batches they created themselves.
    pub scoped: std::sync::atomic::AtomicBool,
    pub owned_batches: Mutex<std::collections::HashSet<String>>,
    /// When each local server's model list was last checked; held while checking, so parallel
    /// model lists wait for one check instead of repeating it.
    pub local_checks: tokio::sync::Mutex<HashMap<String, i64>>,
    /// LMArena leaderboards, matched to models by name.
    pub arenas: std::sync::RwLock<Arc<leaderboard::Boards>>,
}

/// Cheap to clone; all clones share the same state.
#[derive(Clone)]
pub struct Engine {
    pub(crate) inner: Arc<Inner>,
}

impl Engine {
    /// Opens (and migrates) the data directory. Call [`Engine::start`] to begin processing jobs.
    pub async fn open(cfg: Config) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&cfg.data_dir)?;
        let db = open_db(&cfg.data_dir.join("pictesa.db")).await?;
        let crypto = crypto::Crypto::load_or_create(&cfg.data_dir.join("instance.key"))?;
        let files = files::FileStore::new(&cfg.data_dir)?;
        let catalog = catalog::Catalog::new(pictesa_providers::registry());
        let presets = presets::load(&cfg.preset_dirs);
        let recipes = recipes::load(&[cfg.data_dir.join("recipes")]);
        let ua = format!("pictesa/{}", env!("CARGO_PKG_VERSION"));
        let http = reqwest::Client::builder()
            .user_agent(&ua)
            .connect_timeout(Duration::from_secs(15))
            .pool_idle_timeout(Duration::from_secs(60))
            .build()?;
        let download = reqwest::Client::builder()
            .user_agent(&ua)
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(1800))
            .build()?;
        let http_guarded = netguard::guarded_client(&ua, Duration::from_secs(15), None)?;
        let download_guarded = netguard::guarded_client(&ua, Duration::from_secs(20), Some(Duration::from_secs(1800)))?;
        let (events, _) = broadcast::channel(1024);
        let inner = Inner {
            cfg,
            db,
            files,
            catalog,
            crypto,
            presets,
            recipes,
            billing: billing::BillingConfig::from_env(),
            http,
            download,
            instance: new_id(),
            http_guarded,
            download_guarded,
            events,
            wake: Notify::new(),
            cancels: Mutex::new(HashMap::new()),
            previews: Mutex::new(HashMap::new()),
            running: Mutex::new(HashMap::new()),
            scoped: std::sync::atomic::AtomicBool::new(false),
            owned_batches: Mutex::new(std::collections::HashSet::new()),
            local_checks: tokio::sync::Mutex::new(HashMap::new()),
            arenas: Default::default(),
        };
        let engine = Self { inner: Arc::new(inner) };
        engine.inner.catalog.load_cache(&engine.inner.db).await;
        engine.load_arenas().await;
        Ok(engine)
    }

    /// Starts the job dispatcher, resumes interrupted jobs and refreshes stale catalogs.
    pub fn start(&self) {
        let engine = self.clone();
        tokio::spawn(async move {
            engine.fail_orphan_runs().await;
            queue::run(engine).await;
        });
        let engine = self.clone();
        tokio::spawn(async move {
            engine.refresh_stale_catalogs(LOCAL_USER).await;
        });
    }

    /// Starts a dispatcher that only runs jobs created through this engine instance and never
    /// adopts other processes' work. For short-lived CLI runs.
    pub fn start_scoped(&self) {
        self.inner.scoped.store(true, std::sync::atomic::Ordering::SeqCst);
        let engine = self.clone();
        tokio::spawn(async move {
            queue::run(engine).await;
        });
    }

    pub fn config(&self) -> &Config {
        &self.inner.cfg
    }
    pub fn mode(&self) -> Mode {
        self.inner.cfg.mode
    }
    pub fn data_dir(&self) -> &Path {
        &self.inner.cfg.data_dir
    }
    pub fn has_ffmpeg(&self) -> bool {
        self.inner.files.has_ffmpeg()
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }
    pub fn db(&self) -> &SqlitePool {
        &self.inner.db
    }

    /// Latest live preview of a running job of this user.
    pub async fn job_preview(&self, user: &str, job_id: &str) -> Result<(Vec<u8>, String)> {
        self.job(user, job_id).await?;
        self.inner.previews.lock().unwrap().get(job_id).map(|(b, m, _)| (b.clone(), m.clone())).ok_or_else(|| EngineError::not_found("preview"))
    }

    pub(crate) fn emit(&self, user_id: &str, kind: &'static str, data: serde_json::Value) {
        let _ = self.inner.events.send(Event { user_id: user_id.to_string(), kind, data });
    }
}

async fn open_db(path: &Path) -> anyhow::Result<SqlitePool> {
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(15))
        .pragma("cache_size", "-32768")
        .pragma("temp_store", "memory");
    let pool = SqlitePoolOptions::new().max_connections(8).connect_with(opts).await?;
    sqlx::migrate!("../../migrations").run(&pool).await?;
    Ok(pool)
}

/// A new sortable id (ULID).
pub fn new_id() -> String {
    ulid::Ulid::new().to_string()
}
