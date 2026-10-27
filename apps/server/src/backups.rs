//! Backing the library up to another computer over SFTP, and restoring it (spec §15). The point is
//! that a dead SD card never takes the designs with it: once a backup computer is set, every change
//! is copied there within minutes, and a new Pi gets the whole library back with Restore.
//!
//! - **The folder** on the other computer holds a marker (which Pi, encrypted or not), a manifest
//!   (every file with its size and SHA-256) and the files: plain folders by default, or with
//!   encryption, `age` objects under random names. What disappears from the Pi is kept 90 days.
//! - **Incremental:** only new or changed files go; the manifest says what's there.
//! - **Encryption** uses `age`: the Pi keeps only the public key; the private key goes to the backup
//!   folder locked with the person's password (scrypt), so scheduled backups never need it and a
//!   stolen Pi can't read a backup. Restore asks for the password.
//! - **SSH:** `russh` and `russh-sftp`. The other computer's identity is accepted by the person
//!   once and checked every time after.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use age::secrecy::{ExposeSecret, SecretString};
use cameo_pi_core::backup::{self, Entry, Manifest, Marker, Removed, Staged};
use cameo_pi_core::worker::lock;
use cutcraft_device::gpgl::CAMEO_1;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh_sftp::client::SftpSession;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::service::{ApiError, Service};

type Result<T> = std::result::Result<T, ApiError>;

/// How long after the last library change a backup on every change waits, so a burst of changes
/// makes one backup.
const QUIET_S: u64 = 120;
/// Retries after a failed backup: from a minute, doubling, to an hour; given up after a day.
const RETRY_FIRST_S: u64 = 60;
const RETRY_MOST_S: u64 = 3600;
const GIVE_UP_S: u64 = 86_400;
/// A backup on every change also runs at least this often, in case changes were missed.
const AT_LEAST_S: u64 = 86_400;

// Serializes short disk transactions, never an SSH transfer.
static BACKUP_FILES: Mutex<()> = Mutex::new(());

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn storage(error: impl std::fmt::Display) -> ApiError {
    ApiError::new(500, "storage", error.to_string())
}

fn refused(code: &str, message: impl Into<String>) -> ApiError {
    ApiError::new(409, code, message)
}

/// How often backups run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Every {
    /// A couple of minutes after the library changes.
    #[default]
    Change,
    Daily,
    Weekly,
    /// Only Back up now.
    Manual,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SignIn {
    #[default]
    Key,
    Password,
}

/// The other computer's identity, accepted by a person.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KnownHost {
    pub host: String,
    pub port: u16,
    /// `SHA256:…`, as `ssh-keygen -lf` prints it.
    pub fingerprint: String,
    pub algorithm: String,
    pub accepted_at: u64,
}

/// Encryption, when on: the public key, and the private key locked with the password.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Encryption {
    /// `age1…`.
    pub recipient: String,
    /// The private key, `age`-encrypted to the password (scrypt), base64.
    pub locked_key: String,
}

/// `private/backup/settings.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub host: String,
    #[serde(default = "ssh_port")]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub folder: String,
    #[serde(default)]
    pub sign_in: SignIn,
    /// Only with `sign_in: password`. Kept apart in `secrets.json` (0600), never in this file;
    /// read here only to move a password saved by 0.16–0.19 there.
    #[serde(default, skip_serializing)]
    pub password: Option<String>,
    #[serde(default)]
    pub every: Every,
    /// Daily and weekly: the local time, `HH:MM`.
    #[serde(default = "three_am")]
    pub at: String,
    /// Weekly: 0 = Monday … 6 = Sunday.
    #[serde(default)]
    pub weekday: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encryption: Option<Encryption>,
    #[serde(default)]
    pub known_hosts: Vec<KnownHost>,
    /// This Pi, as backup markers name it.
    #[serde(default)]
    pub pi_id: String,
    #[serde(default)]
    pub revision: u64,
}

fn ssh_port() -> u16 {
    22
}

fn three_am() -> String {
    "03:00".into()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 22,
            user: String::new(),
            folder: String::new(),
            sign_in: SignIn::Key,
            password: None,
            every: Every::Change,
            at: three_am(),
            weekday: 0,
            encryption: None,
            known_hosts: Vec::new(),
            pi_id: String::new(),
            revision: 0,
        }
    }
}

impl Settings {
    fn configured(&self) -> bool {
        !self.host.is_empty() && !self.user.is_empty() && !self.folder.is_empty()
    }

    fn known(&self) -> Option<&KnownHost> {
        self.known_hosts
            .iter()
            .find(|k| k.host.eq_ignore_ascii_case(&self.host) && k.port == self.port)
    }
}

/// `private/backup/secrets.json` (0600): the sign-in password, apart from the settings.
#[derive(Default, Serialize, Deserialize)]
struct Secrets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    password: Option<String>,
}

/// A run's outcome.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Last {
    pub at: u64,
    pub ok: bool,
    pub message: String,
}

/// `private/backup/state.json`: how backups have gone.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub last_success: Option<u64>,
    pub last_attempt: Option<Last>,
    /// The first failure of the current run of failures.
    pub failing_since: Option<u64>,
    pub next_retry: Option<u64>,
    pub retry_delay: u64,
    /// When a daily or weekly backup last began its cycle (its scheduled time).
    pub cycle: Option<u64>,
    /// The marker's `updated` when this Pi last wrote it: the local manifest matches the backup
    /// only while they agree.
    pub marker_updated: Option<u64>,
    #[serde(default)]
    pub marker_generation: Option<String>,
    #[serde(default)]
    pub destination: Option<String>,
    #[serde(default)]
    pub mirror_ok: bool,
    #[serde(default)]
    pub maintenance_warning: Option<String>,
}

/// What's running now, for the card's progress.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Progress {
    pub what: String,
    pub done_files: usize,
    pub total_files: usize,
    pub done_bytes: u64,
    pub total_bytes: u64,
}

// Keeps a fetched restore from overlapping a scheduled/manual backup during Replace.
struct ApplyingRestore<'a>(&'a Service);

impl Drop for ApplyingRestore<'_> {
    fn drop(&mut self) {
        self.0.runtime().running = None;
    }
}

/// A restore fetched and checked, waiting for the person's Replace.
pub struct StagedRestore {
    dir: PathBuf,
    staged: Staged,
    marker: Marker,
    manifest: Manifest,
    encryption: Option<Encryption>,
    settings_revision: u64,
}

/// In memory only.
#[derive(Default)]
pub struct Runtime {
    running: Option<Progress>,
    restore: Option<StagedRestore>,
    /// The library change count the last successful backup covered.
    backed_up_changes: Option<u64>,
}

pub type Shared = Mutex<Runtime>;

struct Generation<'a> {
    old: &'a Marker,
    previous: &'a Manifest,
    name: &'a str,
    force: bool,
}

// --- SSH and SFTP -------------------------------------------------------------------------------

/// Why a connection failed, in words the card can show.
enum Fail {
    /// The computer's identity isn't accepted yet: it can be, from the card.
    Unknown {
        fingerprint: String,
        algorithm: String,
    },
    /// Its identity changed: refused until the person forgets the old one.
    Changed {
        fingerprint: String,
    },
    Configuration(String),
    Other(String),
}

impl Fail {
    fn into_error(self) -> ApiError {
        match self {
            Self::Unknown { fingerprint, .. } => refused(
                "identity-unknown",
                format!(
                    "The backup computer's identity ({fingerprint}) isn't accepted yet: use Test connection, then Accept."
                ),
            ),
            Self::Changed { fingerprint } => refused(
                "identity-changed",
                format!(
                    "The backup computer's identity changed (now {fingerprint}), so nothing was sent. If that's expected, forget the old identity on the card."
                ),
            ),
            Self::Configuration(why) => refused("backup-configuration", why),
            Self::Other(why) => ApiError::new(502, "backup-unreachable", why),
        }
    }
}

struct Checker {
    expected: Option<String>,
    seen: Arc<Mutex<Option<(String, String)>>>,
}

impl russh::client::Handler for Checker {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> std::result::Result<bool, Self::Error> {
        let public = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            PublicKeyOrCertificate::Certificate(c) => c.public_key().clone().into(),
        };
        let fingerprint = public.fingerprint(HashAlg::Sha256).to_string();
        let algorithm = public.algorithm().to_string();
        if let Ok(mut seen) = self.seen.lock() {
            *seen = Some((fingerprint.clone(), algorithm));
        }
        Ok(self.expected.as_deref() == Some(fingerprint.as_str()))
    }
}

/// A connected SFTP session, with paths taken from the backup folder.
struct Remote {
    _handle: russh::client::Handle<Checker>,
    sftp: SftpSession,
    folder: String,
}

impl Remote {
    async fn connect(
        settings: &Settings,
        key: &std::path::Path,
    ) -> std::result::Result<Self, Fail> {
        let seen = Arc::new(Mutex::new(None));
        let checker = Checker {
            expected: settings.known().map(|k| k.fingerprint.clone()),
            seen: seen.clone(),
        };
        let config = russh::client::Config {
            inactivity_timeout: Some(Duration::from_secs(120)),
            ..Default::default()
        };
        let connecting = russh::client::connect(
            Arc::new(config),
            (settings.host.as_str(), settings.port),
            checker,
        );
        let handle = match tokio::time::timeout(Duration::from_secs(20), connecting).await {
            Err(_) => return Err(Fail::Other(format!("{} didn't answer.", settings.host))),
            Ok(Ok(handle)) => handle,
            Ok(Err(error)) => {
                let seen = seen.lock().ok().and_then(|s| s.clone());
                return Err(match (seen, settings.known()) {
                    (Some((fingerprint, algorithm)), None) => Fail::Unknown {
                        fingerprint,
                        algorithm,
                    },
                    (Some((fingerprint, _)), Some(known)) if known.fingerprint != fingerprint => {
                        Fail::Changed { fingerprint }
                    }
                    _ => Fail::Other(format!("Couldn't connect to {}: {error}", settings.host)),
                });
            }
        };
        let mut handle = handle;
        let signed_in = match settings.sign_in {
            SignIn::Key => {
                let private = russh::keys::load_secret_key(key, None)
                    .map_err(|_| Fail::Configuration("Make the backup key first.".into()))?;
                let hash = handle
                    .best_supported_rsa_hash()
                    .await
                    .ok()
                    .flatten()
                    .flatten();
                handle
                    .authenticate_publickey(
                        &settings.user,
                        PrivateKeyWithHashAlg::new(Arc::new(private), hash),
                    )
                    .await
            }
            SignIn::Password => {
                handle
                    .authenticate_password(
                        &settings.user,
                        settings.password.clone().unwrap_or_default(),
                    )
                    .await
            }
        }
        .map_err(|e| Fail::Other(format!("Signing in failed: {e}")))?;
        if !matches!(signed_in, russh::client::AuthResult::Success) {
            return Err(Fail::Configuration(match settings.sign_in {
                SignIn::Key => format!(
                    "{} refused the backup key for {}. Add the public key to that account's authorized_keys.",
                    settings.host, settings.user
                ),
                SignIn::Password => format!("{} refused the user name or password.", settings.host),
            }));
        }
        let other = |e: russh::Error| Fail::Other(format!("SFTP: {e}"));
        let channel = handle.channel_open_session().await.map_err(other)?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(other)?;
        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| Fail::Other(format!("{} has no SFTP: {e}", settings.host)))?;
        Ok(Self {
            _handle: handle,
            sftp,
            folder: settings.folder.trim_end_matches('/').to_owned(),
        })
    }

    fn path(&self, rel: &str) -> String {
        if rel.is_empty() {
            self.folder.clone()
        } else {
            format!("{}/{rel}", self.folder)
        }
    }

    async fn exists(&self, rel: &str) -> Result<bool> {
        self.sftp
            .try_exists(self.path(rel))
            .await
            .map_err(|e| transfer_error(rel, e))
    }

    async fn make_dirs(&self, rel: Option<&str>) -> Result<()> {
        let full = rel.map_or_else(|| self.folder.clone(), |r| self.path(r));
        let mut sofar = String::new();
        for (i, part) in full.split('/').enumerate() {
            if i > 0 {
                sofar.push('/');
            }
            sofar.push_str(part);
            if part.is_empty() {
                continue;
            }
            if !self
                .sftp
                .try_exists(&sofar)
                .await
                .map_err(|e| transfer_error(&sofar, e))?
            {
                self.sftp
                    .create_dir(&sofar)
                    .await
                    .map_err(|e| transfer_error(&sofar, e))?;
            }
        }
        Ok(())
    }

    /// Publishes a NEW file. It never unlinks or overwrites a committed dependency.
    async fn put_new(&self, rel: &str, bytes: &[u8]) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        if let Some((parent, _)) = rel.rsplit_once('/') {
            self.make_dirs(Some(parent)).await?;
        }
        let part = format!("{rel}.{}.part", crate::keys::random_hex(8));
        let mut file = self
            .sftp
            .create(self.path(&part))
            .await
            .map_err(|e| transfer_error(rel, e))?;
        file.write_all(bytes)
            .await
            .map_err(|e| ApiError::new(502, "backup-unreachable", format!("Writing {rel}: {e}")))?;
        file.shutdown()
            .await
            .map_err(|e| ApiError::new(502, "backup-unreachable", format!("Closing {rel}: {e}")))?;
        self.sftp
            .rename(self.path(&part), self.path(rel))
            .await
            .map_err(|e| transfer_error(rel, e))
    }

    /// A non-authoritative convenience copy, refreshed only after safe publication.
    async fn put(&self, rel: &str, bytes: &[u8]) -> Result<()> {
        if self.exists(rel).await? {
            self.remove(rel).await?;
        }
        self.put_new(rel, bytes).await
    }

    async fn get(&self, rel: &str) -> Result<Vec<u8>> {
        use tokio::io::AsyncReadExt;
        // An authored project is limited to 48 MiB; allow encryption/preview overhead.
        const MAX_FILE: u64 = 64 * 1024 * 1024;
        let file = self
            .sftp
            .open(self.path(rel))
            .await
            .map_err(|e| transfer_error(rel, e))?;
        let mut bytes = Vec::new();
        file.take(MAX_FILE + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|e| ApiError::new(502, "backup-unreachable", format!("Reading {rel}: {e}")))?;
        if bytes.len() as u64 > MAX_FILE {
            return Err(refused(
                "invalid-backup",
                format!("{rel} exceeds the backup file limit."),
            ));
        }
        Ok(bytes)
    }

    async fn remove(&self, rel: &str) -> Result<()> {
        match self.sftp.remove_file(self.path(rel)).await {
            Ok(()) => Ok(()),
            Err(russh_sftp::client::error::Error::Status(s))
                if s.status_code == russh_sftp::protocol::StatusCode::NoSuchFile =>
            {
                Ok(())
            }
            Err(e) => Err(transfer_error(rel, e)),
        }
    }

    async fn list(&self, rel: &str) -> Result<Vec<(String, bool)>> {
        let entries = self
            .sftp
            .read_dir(self.path(rel))
            .await
            .map_err(|e| transfer_error(rel, e))?;
        Ok(entries
            .map(|e| (e.file_name(), e.file_type().is_dir()))
            .filter(|(n, _)| n != "." && n != "..")
            .collect())
    }

    async fn free_bytes(&self) -> Option<u64> {
        let info = self.sftp.fs_info(self.folder.clone()).await.ok()??;
        Some(info.blocks_avail.saturating_mul(info.fragment_size))
    }
}

fn transfer_error(path: &str, error: russh_sftp::client::error::Error) -> ApiError {
    use russh_sftp::{client::error::Error, protocol::StatusCode};
    let permanent = matches!(&error,Error::Status(s) if !matches!(s.status_code,
        StatusCode::NoConnection|StatusCode::ConnectionLost));
    ApiError::new(
        if permanent { 409 } else { 502 },
        if permanent {
            "backup-configuration"
        } else {
            "backup-unreachable"
        },
        format!("{path}: {error}"),
    )
}

// --- Encryption ----------------------------------------------------------------------------------

fn encrypt(recipient: &str, bytes: &[u8]) -> std::result::Result<Vec<u8>, String> {
    let recipient: age::x25519::Recipient = recipient.parse().map_err(|e: &str| e.to_string())?;
    let encryptor =
        age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
            .map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(bytes.len() + 256);
    let mut writer = encryptor.wrap_output(&mut out).map_err(|e| e.to_string())?;
    writer.write_all(bytes).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

fn decrypt(identity: &dyn age::Identity, bytes: &[u8]) -> std::result::Result<Vec<u8>, String> {
    let decryptor = age::Decryptor::new(bytes).map_err(|e| e.to_string())?;
    let reader = decryptor
        .decrypt(std::iter::once(identity))
        .map_err(|_| "The password doesn't open this backup.".to_owned())?;
    let mut out = Vec::new();
    reader
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    if out.len() > 64 * 1024 * 1024 {
        return Err("The decrypted file exceeds the backup limit.".into());
    }
    Ok(out)
}

/// A new key pair, the private half locked with `password`.
fn new_encryption(password: &str) -> std::result::Result<Encryption, String> {
    use base64::Engine;
    let identity = age::x25519::Identity::generate();
    let encryptor = age::Encryptor::with_user_passphrase(SecretString::from(password.to_owned()));
    let mut locked = Vec::new();
    let mut writer = encryptor
        .wrap_output(&mut locked)
        .map_err(|e| e.to_string())?;
    writer
        .write_all(identity.to_string().expose_secret().as_bytes())
        .map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(Encryption {
        recipient: identity.to_public().to_string(),
        locked_key: base64::engine::general_purpose::STANDARD.encode(locked),
    })
}

/// The private key from the backup folder's `key.age`, opened with `password`.
fn unlock(locked: &[u8], password: &str) -> std::result::Result<age::x25519::Identity, String> {
    let scrypt = age::scrypt::Identity::new(SecretString::from(password.to_owned()));
    let text = decrypt(&scrypt, locked)?;
    String::from_utf8(text)
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .ok_or_else(|| "The backup's key is damaged.".into())
}

// --- The service ---------------------------------------------------------------------------------

/// The time of day, `HH:MM`, as minutes.
fn minutes_of(at: &str) -> Option<u32> {
    let (h, m) = at.split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// The latest scheduled time at or before `now` (seconds), in local time.
fn last_scheduled(settings: &Settings, now: u64) -> Option<u64> {
    use chrono::{Datelike, Local, TimeZone, Timelike};
    let minutes = minutes_of(&settings.at)?;
    let local = Local.timestamp_opt(now as i64, 0).single()?;
    let today = local
        .with_hour(minutes / 60)?
        .with_minute(minutes % 60)?
        .with_second(0)?;
    let mut at = if today <= local {
        today
    } else {
        today - chrono::Duration::days(1)
    };
    if settings.every == Every::Weekly {
        while at.weekday().num_days_from_monday() != u32::from(settings.weekday.min(6)) {
            at -= chrono::Duration::days(1);
        }
    }
    Some(at.timestamp() as u64)
}

impl Service {
    fn backup_dir(&self) -> PathBuf {
        self.private.join("backup")
    }

    fn read_json<T: for<'de> Deserialize<'de> + Default>(&self, name: &str) -> T {
        std::fs::read(self.backup_dir().join(name))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn write_json<T: Serialize>(&self, name: &str, value: &T) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(value).map_err(storage)?;
        self.write_private(name, &bytes)
    }

    /// Writes a file in the backup folder readable by cameo-pi alone (0600, in a 0700 folder),
    /// whatever the umask.
    fn write_private(&self, name: &str, bytes: &[u8]) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let dir = self.backup_dir();
        std::fs::create_dir_all(&dir).map_err(storage)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(storage)?;
        let path = dir.join(name);
        cameo_pi_core::journal::write_atomic(&path, bytes).map_err(storage)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(storage)
    }

    fn settings(&self) -> Settings {
        let _files = BACKUP_FILES.lock().unwrap_or_else(|p| p.into_inner());
        self.settings_locked()
    }

    fn settings_locked(&self) -> Settings {
        let mut settings: Settings = self.read_json("settings.json");
        let secrets: Secrets = self.read_json("secrets.json");
        let moved = settings.password.is_some();
        if moved && secrets.password.is_none() {
            // Saved in settings.json by 0.16–0.19: move it to secrets.json.
            let _ = self.save_settings_locked(&settings);
        } else {
            settings.password = secrets.password;
            if moved {
                let _ = self.save_settings_locked(&settings);
            }
        }
        if settings.pi_id.is_empty() {
            settings.pi_id = crate::keys::random_hex(12);
            let _ = self.save_settings_locked(&settings);
        }
        settings
    }

    /// Saves the settings, the password apart from them.
    fn save_settings_locked(&self, settings: &Settings) -> Result<()> {
        self.write_json(
            "secrets.json",
            &Secrets {
                password: settings.password.clone(),
            },
        )?;
        self.write_json("settings.json", settings)
    }

    /// Makes sure the backup folder's files are cameo-pi's alone (files from before 0.20 relied
    /// on the service's umask). Called once at start.
    pub fn backup_tighten(&self) {
        use std::os::unix::fs::PermissionsExt;
        let dir = self.backup_dir();
        if !dir.is_dir() {
            return;
        }
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let _ = std::fs::set_permissions(entry.path(), std::fs::Permissions::from_mode(0o600));
        }
        // Moves a password out of settings.json.
        let _ = self.settings();
    }

    fn state(&self) -> State {
        let _files = BACKUP_FILES.lock().unwrap_or_else(|p| p.into_inner());
        self.read_json("state.json")
    }

    fn commit_configuration_locked(&self, settings: &mut Settings) -> Result<()> {
        settings.revision = settings
            .revision
            .checked_add(1)
            .ok_or_else(|| storage("Settings revision exhausted"))?;
        self.save_settings_locked(settings)?;
        let mut state: State = self.read_json("state.json");
        if state.destination.as_deref() != Some(destination(settings).as_str()) {
            state = State::default();
        }
        state.failing_since = None;
        state.next_retry = None;
        state.retry_delay = 0;
        state.last_attempt = None;
        state.cycle = None;
        self.write_json("state.json", &state)
    }

    fn rearm(&self, revision: u64) -> Result<()> {
        let _files = BACKUP_FILES.lock().unwrap_or_else(|p| p.into_inner());
        if self.settings_locked().revision != revision {
            return Ok(());
        }
        let mut state: State = self.read_json("state.json");
        state.failing_since = None;
        state.next_retry = None;
        state.retry_delay = 0;
        state.last_attempt = None;
        state.cycle = None;
        self.write_json("state.json", &state)
    }

    fn runtime(&self) -> std::sync::MutexGuard<'_, Runtime> {
        self.backup.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn public_key(&self) -> Option<String> {
        std::fs::read_to_string(self.backup_dir().join("key.pub"))
            .ok()
            .map(|k| k.trim().to_owned())
    }

    fn block_on<T>(&self, future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
            .block_on(future)
    }

    fn pi_name() -> String {
        std::fs::read_to_string("/etc/hostname")
            .map(|h| h.trim().to_owned())
            .ok()
            .filter(|h| !h.is_empty())
            .unwrap_or_else(|| "cameo-pi".into())
    }

    /// How the backup stands, for the card, the banner and MCP. No password is ever shown.
    pub fn backup_view(&self) -> Value {
        let settings = self.settings();
        let state = self.state();
        let runtime = self.runtime();
        let health = if !settings.configured() {
            "off"
        } else if state.failing_since.is_some() && state.next_retry.is_none() {
            "failed"
        } else if state.failing_since.is_some() {
            "retrying"
        } else if state.maintenance_warning.is_some() {
            "warning"
        } else if state.last_success.is_none() {
            "never"
        } else {
            "ok"
        };
        json!({
            "host": settings.host,
            "port": settings.port,
            "user": settings.user,
            "folder": settings.folder,
            "sign_in": settings.sign_in,
            "has_password": settings.password.is_some(),
            "every": settings.every,
            "at": settings.at,
            "weekday": settings.weekday,
            "encrypted": settings.encryption.is_some(),
            "public_key": self.public_key(),
            "identity": settings.known(),
            "identities": settings.known_hosts,
            "last_success": state.last_success,
            "last_attempt": state.last_attempt,
            "failing_since": state.failing_since,
            "next_retry": state.next_retry,
            "health": health,
            "maintenance_warning": state.maintenance_warning,
            "running": runtime.running,
            "restore_waiting": runtime.restore.as_ref().map(|r| json!({
                "jobs": r.staged.jobs.len(),
                "folders": r.staged.folders.len(),
                "from": r.marker.pi_name,
                "updated": r.marker.updated,
            })),
        })
    }

    /// Saves where and how; a field left out stays. A new host or port has no accepted identity.
    pub fn backup_configure(&self, change: Value) -> Result<Value> {
        let files = BACKUP_FILES.lock().unwrap_or_else(|p| p.into_inner());
        let mut settings = self.settings_locked();
        let text = |key: &str| {
            change
                .get(key)
                .and_then(Value::as_str)
                .map(|s| s.trim().to_owned())
        };
        if let Some(host) = text("host") {
            if host
                .chars()
                .any(|c| c.is_whitespace() || c == '@' || c == '/')
            {
                return Err(ApiError::bad_request(
                    "Give the computer's name or address only.",
                ));
            }
            settings.host = host;
        }
        if let Some(port) = change.get("port").and_then(Value::as_u64) {
            settings.port = u16::try_from(port)
                .ok()
                .filter(|p| *p > 0)
                .ok_or_else(|| ApiError::bad_request("The port is a number from 1 to 65535."))?;
        }
        if let Some(user) = text("user") {
            settings.user = user;
        }
        if let Some(folder) = text("folder") {
            if folder.chars().any(char::is_control) {
                return Err(ApiError::bad_request(
                    "The folder name has unusual characters.",
                ));
            }
            settings.folder = folder;
        }
        if let Some(sign_in) = change.get("sign_in") {
            settings.sign_in = serde_json::from_value(sign_in.clone())
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
        }
        if let Some(password) = change.get("password") {
            settings.password = password
                .as_str()
                .filter(|p| !p.is_empty())
                .map(str::to_owned);
        }
        if let Some(every) = change.get("every") {
            settings.every = serde_json::from_value(every.clone())
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
        }
        if let Some(at) = text("at") {
            minutes_of(&at).ok_or_else(|| ApiError::bad_request("The time is HH:MM."))?;
            settings.at = at;
        }
        if let Some(day) = change.get("weekday").and_then(Value::as_u64) {
            settings.weekday = (day.min(6)) as u8;
        }
        self.commit_configuration_locked(&mut settings)?;
        drop(files);
        Ok(self.backup_view())
    }

    /// Makes the backup key (or a new one). Only the public half is ever shown.
    pub fn backup_make_key(&self) -> Result<Value> {
        let files = BACKUP_FILES.lock().unwrap_or_else(|p| p.into_inner());
        use russh::keys::ssh_key::{LineEnding, PrivateKey, private::Ed25519Keypair};
        // An Ed25519 key from the system's random source, by the library's own construction.
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(storage)?;
        let key = PrivateKey::from(Ed25519Keypair::from_seed(&seed));
        seed.fill(0);
        let private = key.to_openssh(LineEnding::LF).map_err(storage)?;
        self.write_private("key", private.as_bytes())?;
        let mut public = key.public_key().clone();
        public.set_comment(format!("cameo-pi backup ({})", Self::pi_name()));
        let line = public.to_openssh().map_err(storage)?;
        self.write_private("key.pub", format!("{line}\n").as_bytes())?;
        let mut settings = self.settings_locked();
        self.commit_configuration_locked(&mut settings)?;
        drop(files);
        Ok(self.backup_view())
    }

    /// Accepts the backup computer's identity that Test connection reported.
    pub fn backup_accept(&self, fingerprint: &str, algorithm: &str) -> Result<Value> {
        let files = BACKUP_FILES.lock().unwrap_or_else(|p| p.into_inner());
        let mut settings = self.settings_locked();
        if !settings.configured() {
            return Err(ApiError::bad_request("Set the computer first."));
        }
        let (host, port) = (settings.host.clone(), settings.port);
        settings
            .known_hosts
            .retain(|k| !(k.host.eq_ignore_ascii_case(&host) && k.port == port));
        settings.known_hosts.push(KnownHost {
            host,
            port,
            fingerprint: fingerprint.to_owned(),
            algorithm: algorithm.to_owned(),
            accepted_at: now(),
        });
        self.commit_configuration_locked(&mut settings)?;
        drop(files);
        Ok(self.backup_view())
    }

    /// Forgets an accepted identity.
    pub fn backup_forget(&self, host: &str, port: u16) -> Result<Value> {
        let files = BACKUP_FILES.lock().unwrap_or_else(|p| p.into_inner());
        let mut settings = self.settings_locked();
        settings
            .known_hosts
            .retain(|k| !(k.host.eq_ignore_ascii_case(host) && k.port == port));
        self.commit_configuration_locked(&mut settings)?;
        drop(files);
        Ok(self.backup_view())
    }

    /// Turns encryption on with `password`, or off with `None`. The backup folder must then match:
    /// an existing backup made the other way is refused until the folder changes.
    pub fn backup_encryption(&self, password: Option<&str>) -> Result<Value> {
        let files = BACKUP_FILES.lock().unwrap_or_else(|p| p.into_inner());
        let mut settings = self.settings_locked();
        settings.encryption = match password {
            Some(p) if p.chars().count() < 10 => {
                return Err(ApiError::bad_request(
                    "Use a password of at least 10 characters: it's the only way back into an encrypted backup.",
                ));
            }
            Some(p) => Some(new_encryption(p).map_err(storage)?),
            None => None,
        };
        self.commit_configuration_locked(&mut settings)?;
        drop(files);
        Ok(self.backup_view())
    }

    /// Test connection: signs in, makes the folder if it's missing, checks it can be written, and
    /// whether it's this Pi's backup, an empty folder, or something else (refused).
    pub fn backup_test(&self) -> Result<Value> {
        let settings = self.settings();
        if !settings.configured() {
            return Err(ApiError::bad_request(
                "Fill in the computer, user and folder first.",
            ));
        }
        let key = self.backup_dir().join("key");
        let result=self.block_on(async {
            let remote = match Remote::connect(&settings, &key).await {
                Ok(remote) => remote,
                Err(Fail::Unknown { fingerprint, algorithm }) => {
                    return Ok(json!({
                        "ok": false,
                        "identity_unknown": {"fingerprint": fingerprint, "algorithm": algorithm},
                        "message": format!("First connection to {}. Check this is your computer, then Accept its identity.", settings.host),
                    }));
                }
                Err(fail) => return Err(fail.into_error()),
            };
            let created = !remote.exists("").await?;
            if created {
                remote.make_dirs(None).await?;
            }
            let probe = format!(".cameo-pi-test-{}", crate::keys::random_hex(4));
            remote
                .put(&probe, b"test")
                .await?;
            remote.remove(&probe).await?;
            let marker = remote.check_folder(&settings).await?;
            Ok(json!({
                "ok": true,
                "created": created,
                "free_bytes": remote.free_bytes().await,
                "backup": marker.filter(|m|m.updated>0).map(|m| json!({"pi_name": m.pi_name, "updated": m.updated, "encrypted": m.encrypted})),
                "message": if created { "Connected; the folder was made." } else { "Connected." },
            }))
        });
        match &result {
            Ok(v) if v["ok"] == true => self.rearm(settings.revision)?,
            Err(e) if !transient(e) => {
                let _files = BACKUP_FILES.lock().unwrap_or_else(|p| p.into_inner());
                if self.settings_locked().revision == settings.revision {
                    let mut state: State = self.read_json("state.json");
                    state.failing_since = Some(now());
                    state.next_retry = None;
                    state.last_attempt = Some(Last {
                        at: now(),
                        ok: false,
                        message: e.message.clone(),
                    });
                    self.write_json("state.json", &state)?;
                }
            }
            _ => {}
        }
        result
    }

    /// Whether a backup is due now (the scheduler asks every few seconds).
    pub fn backup_due(&self) -> bool {
        let settings = self.settings();
        // Nothing runs on its own until the person has accepted the computer's identity.
        if !settings.configured() || settings.known().is_none() || {
            let runtime = self.runtime();
            runtime.running.is_some() || runtime.restore.is_some()
        } {
            return false;
        }
        let state = self.state();
        let now = now();
        if let Some(next) = state.next_retry {
            return now >= next;
        }
        if state.failing_since.is_some() {
            // Only explicit repair/Test/Back up now restarts an exhausted or config-failed cycle.
            return false;
        }
        match settings.every {
            Every::Manual => false,
            Every::Change => {
                let (changes, at) = self.library.changes();
                let covered = self.runtime().backed_up_changes;
                let changed = covered != Some(changes) && changes > 0;
                (changed && now.saturating_sub(at) >= QUIET_S)
                    || state
                        .last_success
                        .is_none_or(|t| now.saturating_sub(t) >= AT_LEAST_S)
            }
            Every::Daily | Every::Weekly => last_scheduled(&settings, now).is_some_and(|at| {
                state.cycle.is_none_or(|cycle| cycle < at)
                    && state.last_success.is_none_or(|s| s < at)
            }),
        }
    }

    /// Runs a backup now and records how it went: retries back off on failure, and give up after
    /// a day.
    pub fn backup_now(&self) -> Result<Last> {
        self.backup_attempt(true)
    }

    pub fn backup_scheduled(&self) -> Result<Last> {
        self.backup_attempt(false)
    }

    fn backup_attempt(&self, manual: bool) -> Result<Last> {
        let settings = self.settings();
        if !settings.configured() {
            return Err(ApiError::bad_request("Set where to back up first."));
        }
        {
            let mut runtime = self.runtime();
            if runtime.running.is_some() || runtime.restore.is_some() {
                return Err(refused(
                    "running",
                    "A backup or restore is running or waiting for confirmation.",
                ));
            }
            runtime.running = Some(Progress {
                what: "backup".into(),
                ..Default::default()
            });
        }
        if manual && let Err(error) = self.rearm(settings.revision) {
            self.runtime().running = None;
            return Err(error);
        }
        let (changes, _) = self.library.changes();
        let result = self.block_on(self.run_backup(&settings));
        let at = now();
        let files = BACKUP_FILES.lock().unwrap_or_else(|p| p.into_inner());
        if self.settings_locked().revision != settings.revision {
            drop(files);
            self.runtime().running = None;
            return Ok(Last {at,ok:false,message:"Backup settings changed while this backup was running; its result does not describe the new destination.".into()});
        }
        let mut state: State = self.read_json("state.json");
        if settings.every != Every::Manual
            && let Some(scheduled) = last_scheduled(&settings, at)
        {
            state.cycle = Some(scheduled);
        }
        let last = match result {
            Ok(message) => {
                state.last_success = Some(at);
                state.failing_since = None;
                state.next_retry = None;
                state.retry_delay = 0;
                self.runtime().backed_up_changes = Some(changes);
                Last {
                    at,
                    ok: true,
                    message,
                }
            }
            Err(error) => {
                let since = *state.failing_since.get_or_insert(at);
                state.retry_delay = (state.retry_delay * 2).clamp(RETRY_FIRST_S, RETRY_MOST_S);
                state.next_retry = (transient(&error) && at.saturating_sub(since) < GIVE_UP_S)
                    .then(|| at + state.retry_delay);
                Last {
                    at,
                    ok: false,
                    message: error.message,
                }
            }
        };
        state.last_attempt = Some(last.clone());
        let saved = self.write_json("state.json", &state);
        drop(files);
        self.runtime().running = None;
        saved?;
        Ok(last)
    }

    fn progress(&self, update: impl FnOnce(&mut Progress)) {
        if let Some(p) = self.runtime().running.as_mut() {
            update(p);
        }
    }

    fn store_backup_cache(
        &self,
        settings: &Settings,
        marker: &Marker,
        manifest: &Manifest,
    ) -> Result<()> {
        let _files = BACKUP_FILES.lock().unwrap_or_else(|p| p.into_inner());
        if self.settings_locked().revision != settings.revision {
            return Ok(());
        }
        self.write_json("manifest.json", manifest)?;
        let mut state: State = self.read_json("state.json");
        state.marker_updated = Some(marker.updated);
        state.marker_generation = marker.generation.clone();
        state.destination = Some(destination(settings));
        state.mirror_ok = false;
        self.write_json("state.json", &state)
    }

    async fn run_backup(&self, settings: &Settings) -> Result<String> {
        let remote = Remote::connect(settings, &self.backup_dir().join("key"))
            .await
            .map_err(Fail::into_error)?;
        if !remote.exists("").await? {
            remote.make_dirs(None).await?;
        }
        let mut previous_marker = remote.check_folder(settings).await?;
        if previous_marker.is_none() {
            // Establish ownership before creating any staging files, so an interrupted first
            // run is resumable without mistaking its own staging for someone else's data.
            let owner = Marker {
                format: backup::FORMAT.into(),
                schema_version: backup::SCHEMA,
                pi_id: settings.pi_id.clone(),
                pi_name: Self::pi_name(),
                encrypted: false,
                recipient: None,
                updated: 0,
                made_by: env!("CARGO_PKG_VERSION").into(),
                generation: None,
                manifest: None,
                key: None,
            };
            remote
                .put_new(
                    backup::MARKER,
                    &serde_json::to_vec_pretty(&owner).map_err(storage)?,
                )
                .await?;
            previous_marker = Some(owner);
        }
        let previous_marker = previous_marker.expect("ownership established");
        let recipient = settings.encryption.as_ref().map(|e| e.recipient.clone());
        let converting = previous_marker.updated > 0
            && (previous_marker.encrypted != settings.encryption.is_some()
                || previous_marker.recipient != recipient);
        let state = self.state();
        let cache_matches = state.destination.as_deref() == Some(destination(settings).as_str())
            && state.marker_generation == previous_marker.generation
            && state.marker_updated == Some(previous_marker.updated);
        let mut previous: Manifest = if previous_marker.updated == 0 {
            Manifest::default()
        } else if cache_matches {
            self.read_json("manifest.json")
        } else if !previous_marker.encrypted {
            serde_json::from_slice(&remote.get(manifest_path(&previous_marker)?).await?).map_err(
                |_| refused("invalid-backup", "The previous backup manifest is damaged."),
            )?
        } else {
            // Without the password we cannot safely forget retained files in an encrypted
            // manifest. A replacement Pi learns it during Restore, never by guessing.
            return Err(refused(
                "backup-configuration",
                "Open Restore and enter this backup's password to check it. You can then cancel without replacing your library and retry the backup.",
            ));
        };
        validate_manifest(&previous)?;
        if previous_marker.encrypted {
            for r in &mut previous.removed {
                if r.key.is_none() {
                    r.key = Some(key_path(&previous_marker)?.to_owned());
                }
            }
        }
        if converting && settings.encryption.is_none() {
            previous.removed.retain(|r| r.key.is_none());
        }
        let serial = previous_marker
            .generation
            .as_deref()
            .and_then(|g| g.split('-').next())
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| {
                refused(
                    "invalid-backup",
                    "The backup generation number is exhausted.",
                )
            })?;
        let generation = format!("{serial:020}-{}", crate::keys::random_hex(8));
        let manifest_name = format!(
            "generations/{generation}/manifest.{}",
            if settings.encryption.is_some() {
                "age"
            } else {
                "json"
            }
        );
        let key_name = settings
            .encryption
            .as_ref()
            .map(|_| format!("generations/{generation}/key.age"));
        if let (Some(e), Some(path)) = (&settings.encryption, &key_name) {
            use base64::Engine;
            let locked = base64::engine::general_purpose::STANDARD
                .decode(&e.locked_key)
                .map_err(storage)?;
            remote.put_new(path, &locked).await?;
        }
        let snapshot = self.library.snapshot().map_err(storage)?;
        let result = self
            .send_generation(
                &remote,
                settings,
                &snapshot,
                Generation {
                    old: &previous_marker,
                    previous: &previous,
                    name: &generation,
                    force: converting,
                },
            )
            .await;
        let (manifest, sent) = match result {
            Ok(v) => v,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&snapshot);
                return Err(e);
            }
        };
        let listing = serde_json::to_vec_pretty(&manifest).map_err(storage)?;
        let bytes = match &settings.encryption {
            Some(e) => encrypt(&e.recipient, &listing).map_err(storage)?,
            None => listing.clone(),
        };
        let publication = async {
            remote.put_new(&manifest_name, &bytes).await?;
            let marker = Marker {
                format: backup::FORMAT.into(),
                schema_version: backup::SCHEMA,
                pi_id: settings.pi_id.clone(),
                pi_name: Self::pi_name(),
                encrypted: settings.encryption.is_some(),
                recipient,
                updated: now(),
                made_by: env!("CARGO_PKG_VERSION").into(),
                generation: Some(generation.clone()),
                manifest: Some(manifest_name),
                key: key_name,
            };
            // This NEW filename is the only publication point. Its dependencies are complete
            // and immutable already; no existing pointer or key is removed to publish it.
            remote
                .put_new(
                    &format!("commits/{generation}.json"),
                    &serde_json::to_vec_pretty(&marker).map_err(storage)?,
                )
                .await?;
            Ok::<_, ApiError>(marker)
        }
        .await;
        let marker = match publication {
            Ok(m) => m,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&snapshot);
                return Err(e);
            }
        };
        self.store_backup_cache(settings, &marker, &manifest)?;
        let mirror = remote
            .refresh_mirror(
                settings,
                &manifest,
                &previous,
                !state.mirror_ok || converting,
                &snapshot,
                &listing,
            )
            .await;
        let _ = std::fs::remove_dir_all(&snapshot);
        // Cleanup is post-publication. Its failure leaves extra copies, never missing data.
        // Report it: in particular, plaintext copies may remain after enabling encryption.
        let maintenance = match mirror {
            Err(e) => Err(e),
            Ok(()) => remote.collect_unreferenced(&marker, &manifest).await,
        };
        let warning = maintenance.as_ref().err().map(|e| format!(
            "Backed up safely, but updating or cleaning the browsable copy failed: {}. Earlier copies may remain; repair access and use Back up now.", e.message));
        {
            let _files = BACKUP_FILES.lock().unwrap_or_else(|p| p.into_inner());
            if self.settings_locked().revision == settings.revision {
                let mut mirrored: State = self.read_json("state.json");
                mirrored.mirror_ok = maintenance.is_ok();
                mirrored.maintenance_warning = warning.clone();
                self.write_json("state.json", &mirrored)?;
            }
        }
        Ok(match maintenance {
            Err(_) => warning.expect("maintenance failure has a warning"),
            Ok(()) if converting => format!(
                "Backed up: the backup is now {}.",
                if marker.encrypted {
                    "encrypted"
                } else {
                    "plain folders"
                }
            ),
            Ok(()) if sent == 0 => "Backed up: nothing had changed.".into(),
            Ok(()) => format!(
                "Backed up: {sent} file{} sent.",
                if sent == 1 { "" } else { "s" }
            ),
        })
    }

    async fn send_generation(
        &self,
        remote: &Remote,
        settings: &Settings,
        snapshot: &std::path::Path,
        generation: Generation<'_>,
    ) -> Result<(Manifest, usize)> {
        let Generation {
            old: old_marker,
            previous,
            name: generation,
            force,
        } = generation;
        let current = backup::scan(snapshot, previous).map_err(storage)?;
        let diff = backup::diff(previous, &current, now());
        let mut manifest = Manifest {
            files: BTreeMap::new(),
            removed: previous
                .removed
                .iter()
                .filter(|r| !diff.expired.contains(r))
                .cloned()
                .collect(),
            directories: backup::directories(snapshot).map_err(storage)?,
        };
        self.progress(|p| {
            p.total_files = current.len();
            p.total_bytes = current.values().map(|f| f.size).sum();
        });
        let mut sent = 0;
        for (path, local) in &current {
            let old = previous.files.get(path);
            if !force
                && let Some(e) = old.filter(|e| e.sha256 == local.sha256 && e.object.is_some())
            {
                let mut e = e.clone();
                e.mtime = local.mtime;
                manifest.files.insert(path.clone(), e);
                continue;
            }
            let bytes = std::fs::read(snapshot.join(path)).map_err(storage)?;
            let object = store_file(remote, settings, generation, path, &bytes).await?;
            manifest.files.insert(
                path.clone(),
                Entry {
                    size: local.size,
                    sha256: local.sha256.clone(),
                    mtime: local.mtime,
                    object: Some(object),
                },
            );
            sent += 1;
            self.progress(|p| {
                p.done_files += 1;
                p.done_bytes += local.size;
            });
        }
        // Retirement changes references only. The previous committed source stays in place.
        for path in &diff.retire {
            let old = &previous.files[path];
            if old_marker.encrypted && settings.encryption.is_none() {
                continue;
            }
            let (stored, key) = match &old.object {
                Some(object) => (
                    object.clone(),
                    if old_marker.encrypted {
                        Some(key_path(old_marker)?.to_owned())
                    } else {
                        None
                    },
                ),
                None => {
                    let bytes = remote.get(&format!("library/{path}")).await?;
                    (
                        store_file(
                            remote,
                            settings,
                            generation,
                            &format!("retained/{}/{path}", crate::keys::random_hex(8)),
                            &bytes,
                        )
                        .await?,
                        settings
                            .encryption
                            .as_ref()
                            .map(|_| format!("generations/{generation}/key.age")),
                    )
                }
            };
            manifest.removed.push(Removed {
                path: path.clone(),
                stored,
                at: now(),
                key,
            });
        }
        for r in &mut manifest.removed {
            if r.key.is_none()
                && (settings.encryption.is_some() || !r.stored.starts_with("generations/"))
            {
                let bytes = remote.get(&r.stored).await?;
                r.stored = store_file(
                    remote,
                    settings,
                    generation,
                    &format!("retained/{}/{}", crate::keys::random_hex(8), r.path),
                    &bytes,
                )
                .await?;
                r.key = settings
                    .encryption
                    .as_ref()
                    .map(|_| format!("generations/{generation}/key.age"));
            }
        }
        Ok((manifest, sent))
    }

    /// Restore, step one: fetches the whole backup into staging and checks every job as an upload
    /// (spec §10). Nothing in the library changes until [`Service::backup_restore_apply`].
    pub fn backup_restore_fetch(&self, password: Option<&str>) -> Result<Value> {
        let settings = self.settings();
        if !settings.configured() {
            return Err(ApiError::bad_request("Set the backup computer first."));
        }
        {
            let mut runtime = self.runtime();
            if runtime.running.is_some() {
                return Err(refused(
                    "running",
                    "A backup or restore is running already.",
                ));
            }
            runtime.running = Some(Progress {
                what: "restore".into(),
                ..Default::default()
            });
            if let Some(old) = runtime.restore.take() {
                let _ = std::fs::remove_dir_all(old.dir);
            }
        }
        let dir = self.library.scratch("restore");
        let result = self.block_on(self.fetch(&settings, password, &dir));
        let outcome = result.and_then(|(marker, manifest, encryption)| {
            let staged = backup::inspect(
                &dir.join("library"),
                &CAMEO_1,
                &self.machine,
                &dir.join("scratch"),
            )
            .map_err(|why| ApiError::new(422, "invalid-backup", why))?;
            Ok((marker, manifest, staged, encryption))
        });
        if let Ok((marker, manifest, _, _)) = &outcome
            && marker.pi_id == settings.pi_id
            && let Err(error) = self.store_backup_cache(&settings, marker, manifest)
        {
            self.runtime().running = None;
            let _ = std::fs::remove_dir_all(&dir);
            return Err(error);
        }
        let mut runtime = self.runtime();
        runtime.running = None;
        match outcome {
            Ok((marker, manifest, staged, encryption)) => {
                let summary = json!({
                    "jobs": staged.jobs.len(),
                    "folders": staged.folders.len(),
                    "from": marker.pi_name,
                    "updated": marker.updated,
                    "replace_allowed": !self.cutters_in_use(),
                });
                runtime.restore = Some(StagedRestore {
                    dir,
                    staged,
                    marker,
                    manifest,
                    encryption,
                    settings_revision: settings.revision,
                });
                Ok(summary)
            }
            Err(error) => {
                let _ = std::fs::remove_dir_all(&dir);
                Err(error)
            }
        }
    }

    async fn fetch(
        &self,
        settings: &Settings,
        password: Option<&str>,
        dir: &std::path::Path,
    ) -> Result<(Marker, Manifest, Option<Encryption>)> {
        let remote = Remote::connect(settings, &self.backup_dir().join("key"))
            .await
            .map_err(Fail::into_error)?;
        let marker = remote
            .current_marker()
            .await?
            .ok_or_else(|| refused("no-backup", "There's no backup in that folder."))?;
        let mut encryption = None;
        let identity = if marker.encrypted {
            use base64::Engine;
            let password = password.filter(|p| !p.is_empty()).ok_or_else(|| {
                refused("password", "This backup is encrypted: give its password.")
            })?;
            let locked = remote.get(key_path(&marker)?).await?;
            let id = unlock(&locked, password).map_err(|why| refused("password", why))?;
            let recipient = id.to_public().to_string();
            if marker.recipient.as_deref() != Some(recipient.as_str()) {
                return Err(refused(
                    "invalid-backup",
                    "The backup key does not match its recipient.",
                ));
            }
            encryption = Some(Encryption {
                recipient,
                locked_key: base64::engine::general_purpose::STANDARD.encode(&locked),
            });
            Some(id)
        } else {
            None
        };
        let stored = remote.get(manifest_path(&marker)?).await?;
        let listing = match &identity {
            Some(id) => decrypt(id, &stored).map_err(|e| refused("invalid-backup", e))?,
            None => stored,
        };
        let manifest: Manifest = serde_json::from_slice(&listing)
            .map_err(|_| refused("invalid-backup", "The backup manifest is damaged."))?;
        validate_manifest(&manifest)?;
        let total: u64 = manifest.files.values().map(|e| e.size).sum();
        self.progress(|p| {
            p.total_files = manifest.files.len();
            p.total_bytes = total;
        });
        let library = dir.join("library");
        std::fs::create_dir_all(&library).map_err(storage)?;
        for folder in &manifest.directories {
            std::fs::create_dir_all(
                library
                    .join(backup::safe_path(folder).map_err(|why| refused("invalid-backup", why))?),
            )
            .map_err(storage)?;
        }
        for (path, entry) in &manifest.files {
            let rel =
                backup::safe_path(path).map_err(|why| ApiError::new(422, "invalid-backup", why))?;
            let stored = entry
                .object
                .clone()
                .unwrap_or_else(|| format!("library/{path}"));
            let raw = remote.get(&stored).await?;
            let bytes = match &identity {
                Some(id) => decrypt(id, &raw).map_err(|e| refused("invalid-backup", e))?,
                None => raw,
            };
            if bytes.len() as u64 != entry.size {
                return Err(refused(
                    "invalid-backup",
                    format!("{path} has the wrong size."),
                ));
            }
            if format!("{:x}", Sha256::digest(&bytes)) != entry.sha256 {
                return Err(ApiError::new(
                    422,
                    "invalid-backup",
                    format!("{path} in the backup is damaged: it doesn't match the manifest."),
                ));
            }
            let target = library.join(rel);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(storage)?;
            }
            std::fs::write(&target, &bytes).map_err(storage)?;
            self.progress(|p| {
                p.done_files += 1;
                p.done_bytes += entry.size;
            });
        }
        Ok((marker, manifest, encryption))
    }

    /// Restore, step two: the fetched library replaces this one (the current library goes to
    /// Recently deleted), and this Pi takes over that backup folder.
    pub fn backup_restore_apply(&self) -> Result<Value> {
        let _cutters = self.revisions.lock().unwrap_or_else(|p| p.into_inner());
        if self.cutters_in_use() {
            return Err(refused(
                "busy",
                "A cutter has a turn or a queue. Restore waits until no cutter is in use.",
            ));
        }
        let restore = {
            let mut runtime = self.runtime();
            if runtime.running.is_some() {
                return Err(refused(
                    "running",
                    "A backup or restore is running already.",
                ));
            }
            let restore = runtime
                .restore
                .take()
                .ok_or_else(|| ApiError::new(404, "not-found", "Fetch the backup again first."))?;
            runtime.running = Some(Progress {
                what: "restore".into(),
                ..Default::default()
            });
            restore
        };
        let _applying = ApplyingRestore(self);
        let files = BACKUP_FILES.lock().unwrap_or_else(|p| p.into_inner());
        if self.settings_locked().revision != restore.settings_revision {
            let _ = std::fs::remove_dir_all(&restore.dir);
            return Err(refused(
                "stale-settings",
                "Backup settings changed. Fetch the backup again before replacing the library.",
            ));
        }
        let result = self.library.replace_with(&restore.dir.join("library"));
        let _ = std::fs::remove_dir_all(&restore.dir);
        result.map_err(storage)?;
        // This Pi carries on that backup: its id, and the manifest as fetched. Files the checks
        // rewrote (plans, previews) go again at the next backup.
        let mut settings = self.settings_locked();
        settings.pi_id = restore.marker.pi_id.clone();
        settings.encryption = restore.encryption;
        settings.revision = settings
            .revision
            .checked_add(1)
            .ok_or_else(|| storage("Settings revision exhausted"))?;
        self.save_settings_locked(&settings)?;
        self.write_json("manifest.json", &restore.manifest)?;
        let mut state: State = self.read_json("state.json");
        state.marker_updated = Some(restore.marker.updated);
        state.marker_generation = restore.marker.generation.clone();
        state.destination = Some(destination(&settings));
        state.failing_since = None;
        state.next_retry = None;
        state.retry_delay = 0;
        self.write_json("state.json", &state)?;
        drop(files);
        Ok(json!({"restored": true, "jobs": restore.staged.jobs.len()}))
    }

    pub fn backup_restore_cancel(&self) {
        if let Some(old) = self.runtime().restore.take() {
            let _ = std::fs::remove_dir_all(old.dir);
        }
    }

    /// Whether any cutter has a turn, a queue or an execution: Replace waits for none.
    fn cutters_in_use(&self) -> bool {
        self.cutters.iter().any(|c| {
            let view = lock(&c.shared).view();
            view.turn.is_some() || !view.queue.is_empty() || view.execution.is_some()
        })
    }
}

fn ownership_part(name: &str) -> bool {
    name.strip_prefix(&format!("{}.", backup::MARKER))
        .and_then(|n| n.strip_suffix(".part"))
        .is_some_and(|nonce| nonce.len() == 16 && nonce.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn transient(error: &ApiError) -> bool {
    error.error == "backup-unreachable"
}

fn destination(settings: &Settings) -> String {
    // Cache identity includes the actual destination, not a second-resolution timestamp.
    serde_json::to_string(&(
        &settings.host,
        settings.port,
        &settings.user,
        &settings.folder,
    ))
    .expect("destination serializes")
}

fn validate_marker(marker: &Marker) -> Result<()> {
    if marker.format != backup::FORMAT || !(1..=backup::SCHEMA).contains(&marker.schema_version) {
        return Err(refused(
            "invalid-backup",
            "This backup format isn't supported.",
        ));
    }
    if let Some(g) = &marker.generation {
        if g.is_empty() || !g.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(refused(
                "invalid-backup",
                "The backup generation name is unsafe.",
            ));
        }
        let expected = format!(
            "generations/{g}/manifest.{}",
            if marker.encrypted { "age" } else { "json" }
        );
        if marker.manifest.as_deref() != Some(expected.as_str())
            || (marker.encrypted
                && marker.key.as_deref() != Some(format!("generations/{g}/key.age").as_str()))
        {
            return Err(refused(
                "invalid-backup",
                "The generation's manifest or key path is invalid.",
            ));
        }
    } else if marker.schema_version > 1 && marker.updated != 0 {
        return Err(refused(
            "invalid-backup",
            "The backup has no committed generation.",
        ));
    }
    Ok(())
}
fn manifest_path(marker: &Marker) -> Result<&str> {
    validate_marker(marker)?;
    if marker.schema_version > 1 && marker.generation.is_none() {
        return Err(refused(
            "no-backup",
            "No backup has finished in this folder yet.",
        ));
    }
    Ok(marker.manifest.as_deref().unwrap_or(if marker.encrypted {
        "manifest.age"
    } else {
        "manifest.json"
    }))
}
fn key_path(marker: &Marker) -> Result<&str> {
    validate_marker(marker)?;
    Ok(marker.key.as_deref().unwrap_or("key.age"))
}
fn checked_path(path: &str) -> Result<()> {
    backup::safe_path(path)
        .map(|_| ())
        .map_err(|why| refused("invalid-backup", why))
}
fn validate_manifest(manifest: &Manifest) -> Result<()> {
    for (p, e) in &manifest.files {
        checked_path(p)?;
        if e.size > 64 * 1024 * 1024
            || e.sha256.len() != 64
            || !e.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(refused(
                "invalid-backup",
                "A backup file has invalid size or hash metadata.",
            ));
        }
        if let Some(object) = &e.object {
            checked_path(object)?;
            if !object.starts_with("objects/") && !object.starts_with("generations/") {
                return Err(refused(
                    "invalid-backup",
                    "An immutable backup file is outside its storage area.",
                ));
            }
        }
    }
    for r in &manifest.removed {
        checked_path(&r.path)?;
        checked_path(&r.stored)?;
        if let Some(key) = &r.key {
            checked_path(key)?;
        }
    }
    for d in &manifest.directories {
        checked_path(d)?;
    }
    Ok(())
}
async fn store_file(
    remote: &Remote,
    settings: &Settings,
    generation: &str,
    path: &str,
    bytes: &[u8],
) -> Result<String> {
    let (object, bytes) = match &settings.encryption {
        Some(e) => (
            backup::object_name(),
            encrypt(&e.recipient, bytes).map_err(storage)?,
        ),
        None => (
            format!("generations/{generation}/library/{path}"),
            bytes.to_vec(),
        ),
    };
    remote.put_new(&object, &bytes).await?;
    Ok(object)
}

impl Remote {
    async fn current_marker(&self) -> Result<Option<Marker>> {
        if self.exists("commits").await? {
            let mut names = self
                .list("commits")
                .await?
                .into_iter()
                .filter(|(n, d)| {
                    !d && n.ends_with(".json")
                        && n.trim_end_matches(".json")
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                })
                .map(|(n, _)| n)
                .collect::<Vec<_>>();
            names.sort();
            if let Some(name) = names.last() {
                let marker: Marker =
                    serde_json::from_slice(&self.get(&format!("commits/{name}")).await?).map_err(
                        |_| refused("invalid-backup", "The committed backup record is damaged."),
                    )?;
                validate_marker(&marker)?;
                if marker.generation.as_deref() != name.strip_suffix(".json") {
                    return Err(refused(
                        "invalid-backup",
                        "The backup commit names another generation.",
                    ));
                }
                return Ok(Some(marker));
            }
        }
        if !self.exists(backup::MARKER).await? {
            return Ok(None);
        }
        let marker: Marker = serde_json::from_slice(&self.get(backup::MARKER).await?)
            .map_err(|_| refused("not-a-backup", "The backup ownership marker is damaged."))?;
        validate_marker(&marker)?;
        Ok(Some(marker))
    }

    async fn check_folder(&self, settings: &Settings) -> Result<Option<Marker>> {
        if self.exists(backup::MARKER).await? {
            let owner: Marker =
                serde_json::from_slice(&self.get(backup::MARKER).await?).map_err(|_| {
                    refused("not-a-backup", "The folder has a damaged cameo-pi marker.")
                })?;
            validate_marker(&owner)?;
            if owner.pi_id != settings.pi_id {
                return Err(refused(
                    "other-pi",
                    format!(
                        "That folder holds the backup of another cameo-pi ({}). Choose another folder, or Restore from it.",
                        owner.pi_name
                    ),
                ));
            }
        }
        let marker = self.current_marker().await?;
        match marker {
            Some(m) if m.pi_id != settings.pi_id => Err(refused(
                "other-pi",
                "That folder holds another Pi's backup. Restore from it first.",
            )),
            Some(m) => Ok(Some(m)),
            None => {
                let others = self
                    .list("")
                    .await?
                    .into_iter()
                    .filter(|(n, dir)| {
                        *dir || (!n.starts_with(".cameo-pi-test-") && !ownership_part(n))
                    })
                    .count();
                if others == 0 {
                    Ok(None)
                } else {
                    Err(refused(
                        "folder-not-empty",
                        "That folder has other things in it. Choose an empty folder, or a new one.",
                    ))
                }
            }
        }
    }

    async fn remove_tree(&self, rel: &str) -> Result<()> {
        if !self.exists(rel).await? {
            return Ok(());
        }
        let mut stack = vec![(rel.to_owned(), false)];
        while let Some((dir, listed)) = stack.pop() {
            if listed {
                self.sftp
                    .remove_dir(self.path(&dir))
                    .await
                    .map_err(|e| transfer_error(&dir, e))?;
                continue;
            }
            stack.push((dir.clone(), true));
            for (name, is_dir) in self.list(&dir).await? {
                let child = format!("{dir}/{name}");
                if is_dir {
                    stack.push((child, false));
                } else {
                    self.remove(&child).await?;
                }
            }
        }
        Ok(())
    }

    async fn refresh_mirror(
        &self,
        settings: &Settings,
        manifest: &Manifest,
        previous: &Manifest,
        full: bool,
        snapshot: &std::path::Path,
        listing: &[u8],
    ) -> Result<()> {
        if settings.encryption.is_some() {
            self.remove_tree("library").await?;
            self.remove_tree("removed").await?;
            self.remove("manifest.json").await?;
            return Ok(());
        }
        self.make_dirs(Some("library")).await?;
        for dir in &manifest.directories {
            self.make_dirs(Some(&format!("library/{dir}"))).await?;
        }
        for (path, e) in &manifest.files {
            let target = format!("library/{path}");
            if full
                || previous
                    .files
                    .get(path)
                    .is_none_or(|old| old.sha256 != e.sha256)
                || !self.exists(&target).await?
            {
                self.put(
                    &target,
                    &std::fs::read(snapshot.join(path)).map_err(storage)?,
                )
                .await?;
            }
        }
        let date = |at| {
            chrono::DateTime::from_timestamp(at as i64, 0)
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default()
        };
        for r in &manifest.removed {
            if r.key.is_some() {
                continue;
            }
            let target = format!("removed/{}/library/{}", date(r.at), r.path);
            if !self.exists(&target).await? {
                self.put(&target, &self.get(&r.stored).await?).await?;
            }
        }
        // Only the convenience mirror is changed here; committed source files are untouched.
        self.prune_mirror(
            "library",
            &manifest
                .files
                .keys()
                .map(|p| format!("library/{p}"))
                .collect(),
            &manifest
                .directories
                .iter()
                .map(|d| format!("library/{d}"))
                .collect(),
        )
        .await?;
        let retained = manifest
            .removed
            .iter()
            .filter(|r| r.key.is_none())
            .map(|r| format!("removed/{}/library/{}", date(r.at), r.path))
            .collect();
        self.prune_mirror("removed", &retained, &std::collections::HashSet::new())
            .await?;
        self.put("manifest.json", listing).await?;
        Ok(())
    }

    async fn prune_mirror(
        &self,
        root: &str,
        keep: &std::collections::HashSet<String>,
        dirs: &std::collections::HashSet<String>,
    ) -> Result<()> {
        if !self.exists(root).await? {
            return Ok(());
        }
        let mut stack = vec![(root.to_owned(), false)];
        while let Some((dir, listed)) = stack.pop() {
            if listed {
                if dir != root && !dirs.contains(&dir) {
                    let _ = self.sftp.remove_dir(self.path(&dir)).await;
                }
                continue;
            }
            stack.push((dir.clone(), true));
            for (name, is_dir) in self.list(&dir).await? {
                let path = format!("{dir}/{name}");
                if is_dir {
                    stack.push((path, false));
                } else if !keep.contains(&path) {
                    self.remove(&path).await?;
                }
            }
        }
        Ok(())
    }

    async fn collect_unreferenced(&self, marker: &Marker, manifest: &Manifest) -> Result<()> {
        let mut keep = std::collections::HashSet::new();
        keep.extend(manifest.files.values().filter_map(|e| e.object.clone()));
        keep.extend(manifest.removed.iter().map(|r| r.stored.clone()));
        keep.extend(manifest.removed.iter().filter_map(|r| r.key.clone()));
        keep.insert(manifest_path(marker)?.to_owned());
        if marker.encrypted {
            keep.insert(key_path(marker)?.to_owned());
        }
        keep.insert(format!(
            "commits/{}.json",
            marker.generation.as_deref().expect("published generation")
        ));
        // Commit first, then retire old pointers before their unreferenced dependencies.
        for root in ["commits", "generations", "objects"] {
            self.prune_mirror(root, &keep, &std::collections::HashSet::new())
                .await?;
        }
        for (name, directory) in self.list("").await? {
            if !directory && ownership_part(&name) {
                self.remove(&name).await?;
            }
        }
        if !keep.contains("key.age") {
            self.remove("key.age").await?;
        }
        self.remove("manifest.age").await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> (tempfile::TempDir, Service) {
        let dir = tempfile::tempdir().unwrap();
        let service = Service::new("test".into(), dir.path(), vec![]).unwrap();
        (dir, service)
    }

    #[test]
    fn saving_settings_keeps_only_the_matching_destinations_cache() {
        let (_dir, service) = service();
        service
            .backup_configure(json!({"host":"localhost", "user":"test", "folder":"/backup"}))
            .unwrap();
        let mut settings = service.settings();
        service
            .write_json(
                "state.json",
                &State {
                    destination: Some(destination(&settings)),
                    marker_updated: Some(42),
                    marker_generation: Some("one".into()),
                    failing_since: Some(1),
                    next_retry: Some(2),
                    ..Default::default()
                },
            )
            .unwrap();
        {
            let _files = BACKUP_FILES.lock().unwrap();
            // Encryption and scheduling changes must retain the previous generation's cache.
            settings.every = Every::Manual;
            service.commit_configuration_locked(&mut settings).unwrap();
        }
        let state = service.state();
        assert_eq!(state.marker_updated, Some(42));
        assert_eq!(state.marker_generation.as_deref(), Some("one"));
        assert!(state.failing_since.is_none() && state.next_retry.is_none());
        service
            .backup_configure(json!({"folder":"/other"}))
            .unwrap();
        assert_eq!(service.state(), State::default());
    }

    #[test]
    fn failed_backups_stay_stopped_for_every_schedule_until_explicit_repair() {
        let (_dir, service) = service();
        service
            .backup_configure(json!({"host":"localhost", "user":"test", "folder":"/backup"}))
            .unwrap();
        service.backup_accept("fingerprint", "ssh-ed25519").unwrap();
        for every in [Every::Change, Every::Daily, Every::Weekly] {
            {
                let _files = BACKUP_FILES.lock().unwrap();
                let mut settings = service.settings_locked();
                settings.every = every;
                service.save_settings_locked(&settings).unwrap();
                service
                    .write_json(
                        "state.json",
                        &State {
                            failing_since: Some(1),
                            ..Default::default()
                        },
                    )
                    .unwrap();
            }
            assert!(!service.backup_due());
        }
        service.create_folder(&[], "New local folder").unwrap();
        assert!(
            !service.backup_due(),
            "new library work does not restart failures"
        );
        let restarted = Service::new("test".into(), _dir.path(), vec![]).unwrap();
        assert!(
            !restarted.backup_due(),
            "a restart does not restart failures"
        );
        service.backup_configure(json!({"every":"change"})).unwrap();
        assert!(service.backup_due());
    }

    #[test]
    fn simultaneous_settings_edits_do_not_lose_each_other() {
        let (_dir, service) = service();
        std::thread::scope(|scope| {
            for patch in [
                json!({"host":"localhost"}),
                json!({"user":"test"}),
                json!({"folder":"/backup"}),
                json!({"port":2222}),
            ] {
                let service = &service;
                scope.spawn(move || {
                    service.backup_configure(patch).unwrap();
                });
            }
        });
        let settings = service.settings();
        assert_eq!(settings.host, "localhost");
        assert_eq!(settings.user, "test");
        assert_eq!(settings.folder, "/backup");
        assert_eq!(settings.port, 2222);
        assert_eq!(settings.revision, 4);
    }

    #[test]
    fn an_old_destinations_result_cannot_replace_the_current_cache() {
        let (_dir, service) = service();
        let old = service.settings();
        service
            .backup_configure(json!({"host":"new-host"}))
            .unwrap();
        let marker: Marker = serde_json::from_value(json!({
            "format":backup::FORMAT, "schema_version":1, "pi_id":"old", "pi_name":"Old Pi",
            "encrypted":false, "updated":42, "made_by":"test"
        }))
        .unwrap();
        service
            .store_backup_cache(&old, &marker, &Manifest::default())
            .unwrap();
        assert_eq!(service.state(), State::default());
        assert!(!service.backup_dir().join("manifest.json").exists());
    }

    #[test]
    fn settings_changes_refuse_a_staged_restore_without_replacing_the_library() {
        let (_dir, service) = service();
        service.create_folder(&[], "Keep me").unwrap();
        let settings = service.settings();
        let restore = service.library.scratch("restore-test");
        std::fs::create_dir_all(restore.join("library")).unwrap();
        let marker: Marker = serde_json::from_value(json!({
            "format":backup::FORMAT, "schema_version":1, "pi_id":"old", "pi_name":"Old Pi",
            "encrypted":false, "updated":42, "made_by":"test"
        }))
        .unwrap();
        service.runtime().restore = Some(StagedRestore {
            dir: restore,
            staged: Staged::default(),
            marker,
            manifest: Manifest::default(),
            encryption: None,
            settings_revision: settings.revision,
        });
        service
            .backup_configure(json!({"host":"new-host"}))
            .unwrap();
        assert_eq!(
            service.backup_restore_apply().unwrap_err().error,
            "stale-settings"
        );
        assert!(service.runtime().running.is_none());
        assert!(
            service
                .library
                .list(&[])
                .unwrap()
                .folders
                .iter()
                .any(|folder| folder == "Keep me")
        );
    }

    #[test]
    fn the_password_is_never_written_with_the_settings() {
        let settings = Settings {
            password: Some("hunter2hunter2".into()),
            ..Settings::default()
        };
        let text = serde_json::to_string(&settings).unwrap();
        assert!(!text.contains("hunter2"), "{text}");
        // A file from 0.16–0.19 still reads, so the password can be moved.
        let old: Settings = serde_json::from_str(r#"{"password": "p"}"#).unwrap();
        assert_eq!(old.password.as_deref(), Some("p"));
    }

    #[test]
    fn times_and_schedules() {
        assert_eq!(minutes_of("03:30"), Some(210));
        assert_eq!(minutes_of("24:00"), None);
        let settings = Settings {
            every: Every::Daily,
            at: "03:00".into(),
            ..Settings::default()
        };
        let now = now();
        let last = last_scheduled(&settings, now).unwrap();
        assert!(last <= now && now - last < 86_400 + 3600);
        let weekly = Settings {
            every: Every::Weekly,
            ..settings
        };
        let last = last_scheduled(&weekly, now).unwrap();
        assert!(last <= now && now - last < 8 * 86_400);
    }

    #[test]
    fn encryption_round_trips_and_the_password_unlocks_only_its_key() {
        let e = new_encryption("correct horse battery").unwrap();
        let sealed = encrypt(&e.recipient, b"a design").unwrap();
        assert_ne!(&sealed[..], b"a design");
        use base64::Engine;
        let locked = base64::engine::general_purpose::STANDARD
            .decode(&e.locked_key)
            .unwrap();
        let identity = unlock(&locked, "correct horse battery").unwrap();
        assert_eq!(decrypt(&identity, &sealed).unwrap(), b"a design");
        assert!(unlock(&locked, "wrong password").is_err());
    }
}
