//! Users, sessions and logins (spec §14.1).
//!
//! While no user exists the Pi is in open mode: anyone who can reach it is an admin, so a fresh
//! Pi can be set up. Creating the first user ends open mode for good. After that every request
//! needs a session or a key a user approved, checked against the current state on each request,
//! never a role remembered from login. Account files that can't be read fail closed: nothing is
//! allowed until `cameo-pi reset-admin` repairs them from the command line.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use cameo_pi_core::journal::write_atomic;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::keys::random_hex;

/// Phones stay logged in this long (spec §14.1, mobile first).
pub const SESSION_DAYS: u64 = 90;
pub const MIN_PASSWORD: usize = 8;
/// Argon2id cost: OWASP's minimum (19 MiB, 2 passes). Measured on the Pi before release.
const ARGON_MEMORY_KIB: u32 = 19 * 1024;
const ARGON_PASSES: u32 = 2;
/// Password hashes at once, so a login flood can't use up the Pi's memory or CPU.
pub const MAX_HASHING: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    Admin,
    User,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub name: String,
    pub role: Role,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hash: String,
    /// Set by an admin reset: until changed, only the password can be changed.
    pub must_change: bool,
    pub created: u64,
    /// New at every password set or reset. A program key carries the value from its approval
    /// and stops working when it changes, even in a server that didn't make the change (the
    /// command-line reset).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub credentials: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Session {
    /// SHA-256 of the cookie value.
    hash: String,
    user: String,
    created: u64,
    expires: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Store {
    users: Vec<User>,
    sessions: Vec<Session>,
}

/// Who a request is, after authentication.
#[derive(Clone, Debug, PartialEq)]
pub enum Identity {
    /// No users exist yet: everyone is an admin.
    Open,
    User {
        name: String,
        role: Role,
        must_change: bool,
    },
    /// Users exist and this request has no valid session.
    Nobody,
    /// The account files are damaged: nothing is allowed (fail closed).
    Locked,
}

#[derive(Debug, PartialEq)]
pub enum AuthError {
    BadLogin,
    /// Too many failed attempts; try again after this long.
    Wait(Duration),
    Busy,
    Weak,
    BadName,
    Exists,
    NotFound,
    LastAdmin,
    Locked,
    Forbidden,
    Storage(String),
}

impl AuthError {
    pub fn message(&self) -> String {
        match self {
            Self::BadLogin => "That user name and password don't match.".into(),
            Self::Wait(d) => format!("Too many tries. Wait {} seconds.", d.as_secs().max(1)),
            Self::Busy => "cameo-pi is busy; try again in a moment.".into(),
            Self::Weak => format!("Passwords need at least {MIN_PASSWORD} characters."),
            Self::BadName => {
                "User names are letters, numbers, spaces, . - and _ (up to 40).".into()
            }
            Self::Exists => "There's already a user with that name.".into(),
            Self::NotFound => "No such user.".into(),
            Self::LastAdmin => "The last admin can't be removed or made a user.".into(),
            Self::Locked => {
                "cameo-pi's account files are damaged. Run `sudo cameo-pi reset-admin` on the Pi."
                    .into()
            }
            Self::Forbidden => "Only an admin can do that.".into(),
            Self::Storage(e) => format!("cameo-pi couldn't save the change: {e}"),
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn sha(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn argon() -> Argon2<'static> {
    let params = Params::new(ARGON_MEMORY_KIB, ARGON_PASSES, 1, None).expect("valid parameters");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

pub fn hash_password(password: &str) -> String {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).expect("the system's random number source works");
    let salt = SaltString::encode_b64(&salt).expect("16 bytes encode");
    argon()
        .hash_password(password.as_bytes(), &salt)
        .expect("hashing a password")
        .to_string()
}

fn verify(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|parsed| {
            argon()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

fn check_name(name: &str) -> Result<(), AuthError> {
    let ok = !name.is_empty()
        && name.chars().count() <= 40
        && name.trim() == name
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || " .-_".contains(c));
    if ok { Ok(()) } else { Err(AuthError::BadName) }
}

/// Failed-login backoff, per user name and per network address.
#[derive(Default)]
struct Backoff {
    failures: HashMap<String, (u32, Instant)>,
}

impl Backoff {
    fn delay(failures: u32) -> Duration {
        if failures < 3 {
            Duration::ZERO
        } else {
            Duration::from_secs((1u64 << (failures - 3).min(6)).min(60))
        }
    }
    fn wait(&self, keys: &[String]) -> Option<Duration> {
        keys.iter()
            .filter_map(|k| self.failures.get(k))
            .filter_map(|(n, at)| Self::delay(*n).checked_sub(at.elapsed()))
            .filter(|d| !d.is_zero())
            .max()
    }
    fn fail(&mut self, keys: &[String]) {
        for k in keys {
            let entry = self
                .failures
                .entry(k.clone())
                .or_insert((0, Instant::now()));
            entry.0 += 1;
            entry.1 = Instant::now();
        }
        if self.failures.len() > 1000 {
            self.failures
                .retain(|_, (_, at)| at.elapsed() < Duration::from_secs(3600));
        }
    }
    fn succeed(&mut self, keys: &[String]) {
        for k in keys {
            self.failures.remove(k);
        }
    }
}

pub struct Users {
    path: PathBuf,
    state: Mutex<(Result<Store, String>, Option<SystemTime>)>,
    backoff: Mutex<Backoff>,
    hashing: Mutex<usize>,
}

/// Written (and synced) when the first user is created, and never removed: from then on a
/// missing account file means damage, not a fresh Pi, so it fails closed instead of reopening
/// open mode.
const INITIALISED: &str = "accounts-initialised";

fn load(path: &Path) -> (Result<Store, String>, Option<SystemTime>) {
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let initialised = path
        .parent()
        .is_some_and(|dir| dir.join(INITIALISED).exists());
    let store = match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !initialised => Ok(Store::default()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err("the account file is missing".into())
        }
        Err(e) => Err(e.to_string()),
    };
    (store, modified)
}

/// Marks the accounts as set up, for good.
fn mark_initialised(dir: &Path) -> Result<(), String> {
    let marker = dir.join(INITIALISED);
    if !marker.exists() {
        write_atomic(
            &marker,
            b"Users exist; a missing users.json means damage.\n",
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

impl Users {
    pub fn open(private: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(private)?;
        let path = private.join("users.json");
        Ok(Self {
            state: Mutex::new(load(&path)),
            path,
            backoff: Mutex::new(Backoff::default()),
            hashing: Mutex::new(0),
        })
    }

    /// The store, reloaded if the file changed (the command line may have reset an admin).
    fn store(&self) -> MutexGuard<'_, (Result<Store, String>, Option<SystemTime>)> {
        let mut guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let modified = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok();
        if modified != guard.1 {
            *guard = load(&self.path);
        }
        guard
    }

    /// Runs a password hash or check within the bound on concurrent hashing.
    fn hashing<T>(&self, f: impl FnOnce() -> T) -> Result<T, AuthError> {
        {
            let mut busy = self.hashing.lock().unwrap_or_else(|p| p.into_inner());
            if *busy >= MAX_HASHING {
                return Err(AuthError::Busy);
            }
            *busy += 1;
        }
        let result = f();
        *self.hashing.lock().unwrap_or_else(|p| p.into_inner()) -= 1;
        Ok(result)
    }

    #[cfg(test)]
    pub fn open_mode(&self) -> bool {
        matches!(&self.store().0, Ok(store) if store.users.is_empty())
    }

    /// Who presents this session cookie, checked against the current users and sessions.
    pub fn identify(&self, session: Option<&str>) -> Identity {
        let guard = self.store();
        let store = match &guard.0 {
            Err(_) => return Identity::Locked,
            Ok(store) => store,
        };
        if store.users.is_empty() {
            return Identity::Open;
        }
        let Some(cookie) = session else {
            return Identity::Nobody;
        };
        let hashed = sha(cookie);
        let now = now();
        store
            .sessions
            .iter()
            .find(|s| s.hash == hashed && s.expires > now)
            .and_then(|s| store.users.iter().find(|u| u.name == s.user))
            .map_or(Identity::Nobody, |u| Identity::User {
                name: u.name.clone(),
                role: u.role,
                must_change: u.must_change,
            })
    }

    fn new_session(store: &mut Store, user: &str) -> String {
        let cookie = random_hex(32);
        let now = now();
        store.sessions.retain(|s| s.expires > now);
        store.sessions.push(Session {
            hash: sha(&cookie),
            user: user.to_owned(),
            created: now,
            expires: now + SESSION_DAYS * 86_400,
        });
        cookie
    }

    /// Logs in, returning a new session cookie value.
    pub fn login(&self, name: &str, password: &str, address: &str) -> Result<String, AuthError> {
        let keys = [format!("user:{name}"), format!("addr:{address}")];
        if let Some(wait) = self
            .backoff
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .wait(&keys)
        {
            return Err(AuthError::Wait(wait));
        }
        let hash = {
            let guard = self.store();
            let store = guard.0.as_ref().map_err(|_| AuthError::Locked)?;
            store
                .users
                .iter()
                .find(|u| u.name == name)
                .map(|u| u.hash.clone())
        };
        // Hash even for an unknown name, so timing doesn't reveal which names exist.
        let ok = self.hashing(|| match &hash {
            Some(hash) => verify(password, hash),
            None => {
                let _ = verify(password, &hash_password("x"));
                false
            }
        })?;
        let mut backoff = self.backoff.lock().unwrap_or_else(|p| p.into_inner());
        if !ok {
            backoff.fail(&keys);
            return Err(AuthError::BadLogin);
        }
        backoff.succeed(&keys);
        drop(backoff);
        self.change(|store| {
            if !store.users.iter().any(|u| u.name == name) {
                return Err(AuthError::BadLogin);
            }
            Ok(Self::new_session(store, name))
        })
    }

    pub fn logout(&self, session: &str) -> Result<(), AuthError> {
        self.change(|store| {
            let hashed = sha(session);
            store.sessions.retain(|s| s.hash != hashed);
            Ok(())
        })
    }

    /// Applies `f` to a copy of the store, saves the copy, and only then makes it the live
    /// state, all under the lock. A failed save changes nothing, in memory or on disk.
    fn change<T>(
        &self,
        f: impl FnOnce(&mut Store) -> Result<T, AuthError>,
    ) -> Result<T, AuthError> {
        let mut guard = self.store();
        let current = guard.0.as_ref().map_err(|_| AuthError::Locked)?;
        let mut candidate = Store {
            users: current.users.clone(),
            sessions: current.sessions.clone(),
        };
        let value = f(&mut candidate)?;
        let bytes =
            serde_json::to_vec_pretty(&candidate).map_err(|e| AuthError::Storage(e.to_string()))?;
        if !candidate.users.is_empty()
            && let Some(dir) = self.path.parent()
        {
            mark_initialised(dir).map_err(AuthError::Storage)?;
        }
        write_atomic(&self.path, &bytes).map_err(|e| AuthError::Storage(e.to_string()))?;
        guard.1 = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok();
        guard.0 = Ok(candidate);
        Ok(value)
    }

    /// Creates the first user, ending open mode, in one step: it is always an admin, every
    /// open-mode session ends, and the creator gets a new session (spec §14.1). A second request
    /// racing it finds users already exist and is refused.
    pub fn create_first(&self, name: &str, password: &str) -> Result<String, AuthError> {
        check_name(name)?;
        if password.chars().count() < MIN_PASSWORD {
            return Err(AuthError::Weak);
        }
        let hash = self.hashing(|| hash_password(password))?;
        self.change(|store| {
            if !store.users.is_empty() {
                return Err(AuthError::Forbidden);
            }
            store.users.push(User {
                name: name.to_owned(),
                role: Role::Admin,
                hash,
                must_change: false,
                created: now(),
                credentials: random_hex(8),
            });
            store.sessions.clear();
            Ok(Self::new_session(store, name))
        })
    }

    /// An admin adds a user with a first password, which they change at their first login.
    pub fn create(&self, name: &str, password: &str, role: Role) -> Result<(), AuthError> {
        check_name(name)?;
        if password.chars().count() < MIN_PASSWORD {
            return Err(AuthError::Weak);
        }
        let hash = self.hashing(|| hash_password(password))?;
        self.change(|store| {
            if store.users.iter().any(|u| u.name == name) {
                return Err(AuthError::Exists);
            }
            store.users.push(User {
                name: name.to_owned(),
                role,
                hash,
                must_change: true,
                created: now(),
                credentials: random_hex(8),
            });
            Ok(())
        })
    }

    pub fn list(&self) -> Result<Vec<User>, AuthError> {
        let guard = self.store();
        let store = guard.0.as_ref().map_err(|_| AuthError::Locked)?;
        Ok(store
            .users
            .iter()
            .map(|u| User {
                hash: String::new(),
                credentials: String::new(),
                ..u.clone()
            })
            .collect())
    }

    /// A user's current credentials value, for the keys they approve or adopt.
    pub fn credentials(&self, name: &str) -> Option<String> {
        let guard = self.store();
        let store = guard.0.as_ref().ok()?;
        store
            .users
            .iter()
            .find(|u| u.name == name)
            .map(|u| u.credentials.clone())
    }

    /// The user a program key acts as: only while the key's credentials are the user's current
    /// ones (spec §14.1: a password change or reset ends that user's keys).
    pub fn for_key(&self, name: &str, credentials: &str) -> Option<User> {
        self.list()
            .ok()?
            .into_iter()
            .find(|u| u.name == name && self.credentials(name).as_deref() == Some(credentials))
    }

    fn admins(store: &Store) -> usize {
        store.users.iter().filter(|u| u.role == Role::Admin).count()
    }

    pub fn set_role(&self, name: &str, role: Role) -> Result<(), AuthError> {
        self.change(|store| {
            let admins = Self::admins(store);
            let user = store
                .users
                .iter_mut()
                .find(|u| u.name == name)
                .ok_or(AuthError::NotFound)?;
            if user.role == Role::Admin && role == Role::User && admins == 1 {
                return Err(AuthError::LastAdmin);
            }
            user.role = role;
            Ok(())
        })
    }

    /// Removes a user and ends their sessions. Their program keys stop working at once (keys
    /// check that their user exists) and are removed by the caller.
    pub fn remove(&self, name: &str) -> Result<(), AuthError> {
        self.change(|store| {
            let admins = Self::admins(store);
            let index = store
                .users
                .iter()
                .position(|u| u.name == name)
                .ok_or(AuthError::NotFound)?;
            if store.users[index].role == Role::Admin && admins == 1 {
                return Err(AuthError::LastAdmin);
            }
            store.users.remove(index);
            store.sessions.retain(|s| s.user != name);
            Ok(())
        })
    }

    /// A new password. By an admin for someone else (`must_change`), or by the user themself,
    /// keeping only the session they changed it from. Either way every other session ends.
    pub fn set_password(
        &self,
        name: &str,
        password: &str,
        must_change: bool,
        keep_session: Option<&str>,
    ) -> Result<(), AuthError> {
        if password.chars().count() < MIN_PASSWORD {
            return Err(AuthError::Weak);
        }
        let hash = self.hashing(|| hash_password(password))?;
        let keep = keep_session.map(sha);
        self.change(|store| {
            let user = store
                .users
                .iter_mut()
                .find(|u| u.name == name)
                .ok_or(AuthError::NotFound)?;
            user.hash = hash;
            user.must_change = must_change;
            user.credentials = random_hex(8);
            store
                .sessions
                .retain(|s| s.user != name || Some(&s.hash) == keep.as_ref());
            Ok(())
        })
    }

    /// Checks a user's current password (for "change my password").
    pub fn check_password(&self, name: &str, password: &str) -> Result<bool, AuthError> {
        let hash = {
            let guard = self.store();
            let store = guard.0.as_ref().map_err(|_| AuthError::Locked)?;
            store
                .users
                .iter()
                .find(|u| u.name == name)
                .map(|u| u.hash.clone())
        };
        let hash = hash.ok_or(AuthError::NotFound)?;
        self.hashing(|| verify(password, &hash))
    }
}

/// `cameo-pi reset-admin`: sets a temporary password for an admin (creating one if there is
/// none, or if the account file is damaged, after moving it aside), ends that admin's sessions,
/// and returns the user name and the temporary password. Command line only (spec §14.1).
pub fn reset_admin(private: &Path, name: Option<&str>) -> Result<(String, String), String> {
    std::fs::create_dir_all(private).map_err(|e| e.to_string())?;
    let path = private.join("users.json");
    let (store, _) = load(&path);
    let mut store = match store {
        Ok(store) => store,
        Err(_) => {
            if path.exists() {
                let aside = private.join(format!("users.damaged-{}.json", now()));
                std::fs::rename(&path, &aside).map_err(|e| e.to_string())?;
            }
            Store::default()
        }
    };
    let password = random_hex(6);
    let hash = hash_password(&password);
    let target = match name {
        Some(name) => name.to_owned(),
        None => store
            .users
            .iter()
            .find(|u| u.role == Role::Admin)
            .map(|u| u.name.clone())
            .unwrap_or_else(|| "admin".into()),
    };
    check_name(&target).map_err(|e| e.message())?;
    match store.users.iter_mut().find(|u| u.name == target) {
        Some(user) => {
            user.role = Role::Admin;
            user.hash = hash;
            user.must_change = true;
            user.credentials = random_hex(8);
        }
        None => store.users.push(User {
            name: target.clone(),
            role: Role::Admin,
            hash,
            must_change: true,
            created: now(),
            credentials: random_hex(8),
        }),
    }
    store.sessions.retain(|s| s.user != target);
    let bytes = serde_json::to_vec_pretty(&store).map_err(|e| e.to_string())?;
    mark_initialised(private)?;
    write_atomic(&path, &bytes).map_err(|e| e.to_string())?;
    Ok((target, password))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn users() -> (Users, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (Users::open(dir.path()).unwrap(), dir)
    }

    #[test]
    fn open_mode_until_the_first_user_then_logins_only() {
        let (u, _dir) = users();
        assert_eq!(u.identify(None), Identity::Open);
        let cookie = u.create_first("Sam", "correct horse").unwrap();
        assert_eq!(u.identify(None), Identity::Nobody);
        assert_eq!(
            u.identify(Some(&cookie)),
            Identity::User {
                name: "Sam".into(),
                role: Role::Admin,
                must_change: false
            }
        );
        // The race: a second "first user" is refused.
        assert_eq!(
            u.create_first("Alex", "another pass"),
            Err(AuthError::Forbidden)
        );
    }

    #[test]
    fn logins_back_off_after_failures_and_reveal_nothing() {
        let (u, _dir) = users();
        u.create_first("Sam", "correct horse").unwrap();
        for _ in 0..3 {
            assert_eq!(
                u.login("Sam", "wrong", "10.0.0.5"),
                Err(AuthError::BadLogin)
            );
        }
        assert!(matches!(
            u.login("Sam", "correct horse", "10.0.0.5"),
            Err(AuthError::Wait(_))
        ));
        assert_eq!(u.login("Nobody", "x", "10.0.0.9"), Err(AuthError::BadLogin));
    }

    #[test]
    fn an_admin_reset_limits_the_user_until_they_change_it_and_ends_sessions() {
        let (u, _dir) = users();
        let admin = u.create_first("Sam", "correct horse").unwrap();
        u.create("Alex", "first password", Role::User).unwrap();
        let alex = u.login("Alex", "first password", "a").unwrap();
        assert!(matches!(
            u.identify(Some(&alex)),
            Identity::User {
                must_change: true,
                ..
            }
        ));
        u.set_password("Alex", "my own password", false, Some(&alex))
            .unwrap();
        assert!(matches!(
            u.identify(Some(&alex)),
            Identity::User {
                must_change: false,
                ..
            }
        ));
        u.set_password("Alex", "reset by admin", true, None)
            .unwrap();
        assert_eq!(u.identify(Some(&alex)), Identity::Nobody, "sessions end");
        assert!(matches!(u.identify(Some(&admin)), Identity::User { .. }));
    }

    #[test]
    fn the_last_admin_stays_and_removing_a_user_ends_their_sessions() {
        let (u, _dir) = users();
        u.create_first("Sam", "correct horse").unwrap();
        assert_eq!(u.remove("Sam"), Err(AuthError::LastAdmin));
        assert_eq!(u.set_role("Sam", Role::User), Err(AuthError::LastAdmin));
        u.create("Alex", "first password", Role::Admin).unwrap();
        let alex = u.login("Alex", "first password", "a").unwrap();
        u.remove("Alex").unwrap();
        assert_eq!(u.identify(Some(&alex)), Identity::Nobody);
    }

    #[test]
    fn damaged_account_files_fail_closed_until_reset_from_the_command_line() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("users.json"), b"{broken").unwrap();
        let u = Users::open(dir.path()).unwrap();
        assert_eq!(u.identify(None), Identity::Locked, "not open mode");
        assert!(!u.open_mode());

        let (name, password) = reset_admin(dir.path(), None).unwrap();
        assert_eq!(name, "admin");
        let cookie = u.login(&name, &password, "local").unwrap();
        assert!(matches!(
            u.identify(Some(&cookie)),
            Identity::User {
                role: Role::Admin,
                must_change: true,
                ..
            }
        ));
    }

    #[test]
    fn reset_admin_ends_that_admins_sessions_without_reopening_open_mode() {
        let dir = tempfile::tempdir().unwrap();
        let u = Users::open(dir.path()).unwrap();
        let old = u.create_first("Sam", "correct horse").unwrap();
        let (name, _) = reset_admin(dir.path(), Some("Sam")).unwrap();
        assert_eq!(name, "Sam");
        assert_eq!(u.identify(Some(&old)), Identity::Nobody);
        assert!(!u.open_mode());
    }

    #[test]
    fn review_missing_initialized_users_file_must_fail_closed() {
        let (u, dir) = users();
        u.create_first("Sam", "correct horse").unwrap();
        std::fs::remove_file(dir.path().join("users.json")).unwrap();
        assert_eq!(u.identify(None), Identity::Locked);
    }
    #[test]
    fn review_failed_persistence_must_not_change_live_accounts() {
        use std::os::unix::fs::PermissionsExt;
        let (u, dir) = users();
        u.create_first("Sam", "correct horse").unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let result = u.create("Other", "correct horse", Role::Admin);
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(result.is_err());
        assert!(
            !u.list().unwrap().iter().any(|a| a.name == "Other"),
            "failed save created a live admin account"
        );
    }

    #[test]
    fn a_restart_with_the_account_file_missing_stays_locked_until_reset() {
        let dir = tempfile::tempdir().unwrap();
        Users::open(dir.path())
            .unwrap()
            .create_first("Sam", "correct horse")
            .unwrap();
        std::fs::remove_file(dir.path().join("users.json")).unwrap();
        let restarted = Users::open(dir.path()).unwrap();
        assert_eq!(restarted.identify(None), Identity::Locked);
        assert!(restarted.create_first("Eve", "12345678").is_err());
        let (name, password) = reset_admin(dir.path(), None).unwrap();
        assert!(restarted.login(&name, &password, "local").is_ok());
        assert!(!restarted.open_mode());
    }

    #[test]
    fn failed_saves_change_nothing_for_roles_passwords_removals_or_the_first_user() {
        use std::os::unix::fs::PermissionsExt;
        let (u, dir) = users();
        let read_only = |on: bool| {
            let mode = if on { 0o500 } else { 0o700 };
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(mode)).unwrap();
        };
        // The first user, refused by a read-only folder: still open mode.
        read_only(true);
        assert!(u.create_first("Sam", "correct horse").is_err());
        read_only(false);
        assert_eq!(u.identify(None), Identity::Open);

        let sam = u.create_first("Sam", "correct horse").unwrap();
        u.create("Alex", "first password", Role::Admin).unwrap();
        read_only(true);
        assert!(u.set_role("Alex", Role::User).is_err());
        assert!(u.set_password("Sam", "new password", false, None).is_err());
        assert!(u.remove("Alex").is_err());
        read_only(false);
        let list = u.list().unwrap();
        let alex = list.iter().find(|x| x.name == "Alex").unwrap();
        assert_eq!(alex.role, Role::Admin);
        assert!(
            matches!(u.identify(Some(&sam)), Identity::User { .. }),
            "Sam's session survives"
        );
        // And after a restart the same.
        let restarted = Users::open(dir.path()).unwrap();
        assert_eq!(restarted.list().unwrap().len(), 2);
    }
}
