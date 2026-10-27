//! Program access keys and pairing (spec §14.2), OctoPrint "application keys" style: a program
//! asks with its name, someone allows it on the web interface, and the program collects its own
//! key with the request secret only it holds. Keys are stored hashed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use cameo_pi_core::journal::write_atomic;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// How long a request may wait for approval.
pub const REQUEST_LIFETIME: Duration = Duration::from_secs(600);
/// The most requests waiting at once, so the bootstrap route can't be flooded.
pub const MAX_PENDING: usize = 8;

pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("the system's random number source works");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn hash(secret: &str) -> String {
    format!("{:x}", Sha256::digest(secret.as_bytes()))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Key {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub hash: String,
    pub created: u64,
    pub approved_by: String,
    /// The user it belongs to and acts as; `None` for keys approved in open mode.
    #[serde(default)]
    pub user: Option<String>,
    /// That user's credentials value when the key was approved or adopted: the key works only
    /// while it is still theirs (`Users::for_key`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub credentials: String,
}

#[derive(Debug)]
enum Pending {
    Waiting,
    Approved(String),
    Denied,
    Collected,
}

#[derive(Debug)]
struct Request {
    name: String,
    code: String,
    at: Instant,
    state: Pending,
}

/// A waiting request, as the web interface lists it.
#[derive(Clone, Debug, Serialize)]
pub struct Waiting {
    pub code: String,
    pub name: String,
    pub waited_s: u64,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "state")]
pub enum Collect {
    Waiting,
    Denied,
    Key { key: String },
}

pub struct Keys {
    path: PathBuf,
    /// The keys, and the file's modification time when they were read: the file is read again
    /// when it changes, so a command-line change shows here too.
    keys: Mutex<(Vec<Key>, Option<SystemTime>)>,
    /// Requests by the hash of their secret.
    requests: Mutex<HashMap<String, Request>>,
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn read(path: &Path) -> std::io::Result<(Vec<Key>, Option<SystemTime>)> {
    let stamp = modified(path);
    match std::fs::read(path) {
        Ok(bytes) => Ok((
            serde_json::from_slice(&bytes).map_err(std::io::Error::other)?,
            stamp,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Vec::new(), None)),
        Err(e) => Err(e),
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Keys {
    pub fn open(private: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(private)?;
        let path = private.join("keys.json");
        let keys = read(&path)?;
        Ok(Self {
            path,
            keys: Mutex::new(keys),
            requests: Mutex::new(HashMap::new()),
        })
    }

    /// The current keys, read again if the file changed. A file that can't be read leaves no
    /// key working.
    fn current(&self) -> MutexGuard<'_, (Vec<Key>, Option<SystemTime>)> {
        let mut guard = self.keys.lock().unwrap_or_else(|p| p.into_inner());
        if modified(&self.path) != guard.1 {
            *guard = read(&self.path).unwrap_or((Vec::new(), None));
        }
        guard
    }

    fn save(&self, keys: &mut (Vec<Key>, Option<SystemTime>)) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(&keys.0).map_err(std::io::Error::other)?;
        write_atomic(&self.path, &bytes)?;
        keys.1 = modified(&self.path);
        Ok(())
    }

    fn expire(requests: &mut HashMap<String, Request>) {
        requests.retain(|_, r| r.at.elapsed() < REQUEST_LIFETIME);
    }

    /// A program asks for access. Returns its request secret and the code both sides show.
    pub fn request(&self, name: &str) -> Result<(String, String), &'static str> {
        let name: String = name.chars().filter(|c| !c.is_control()).take(80).collect();
        if name.trim().is_empty() {
            return Err("A program must say what it is.");
        }
        let mut requests = self.requests.lock().unwrap_or_else(|p| p.into_inner());
        Self::expire(&mut requests);
        let waiting = requests
            .values()
            .filter(|r| matches!(r.state, Pending::Waiting))
            .count();
        if waiting >= MAX_PENDING {
            return Err("Too many programs are waiting for approval; try again later.");
        }
        let secret = random_hex(16);
        // A code no other waiting request has, so approving one can't approve another.
        let code = loop {
            let mut code_bytes = [0u8; 2];
            getrandom::fill(&mut code_bytes).expect("the system's random number source works");
            let code = format!("{:04}", u16::from_le_bytes(code_bytes) % 10_000);
            if !requests
                .values()
                .any(|r| r.code == code && matches!(r.state, Pending::Waiting))
            {
                break code;
            }
        };
        requests.insert(
            hash(&secret),
            Request {
                name,
                code: code.clone(),
                at: Instant::now(),
                state: Pending::Waiting,
            },
        );
        Ok((secret, code))
    }

    pub fn waiting(&self) -> Vec<Waiting> {
        let mut requests = self.requests.lock().unwrap_or_else(|p| p.into_inner());
        Self::expire(&mut requests);
        requests
            .values()
            .filter(|r| matches!(r.state, Pending::Waiting))
            .map(|r| Waiting {
                code: r.code.clone(),
                name: r.name.clone(),
                waited_s: r.at.elapsed().as_secs(),
            })
            .collect()
    }

    /// Allow or deny the waiting request with this code. Codes only match requests; they are
    /// not a security check of the connection (spec §14.2).
    pub fn decide(
        &self,
        code: &str,
        allow: bool,
        by: &str,
        user: Option<(&str, &str)>,
    ) -> std::io::Result<bool> {
        let mut requests = self.requests.lock().unwrap_or_else(|p| p.into_inner());
        Self::expire(&mut requests);
        let Some(request) = requests
            .values_mut()
            .find(|r| r.code == code && matches!(r.state, Pending::Waiting))
        else {
            return Ok(false);
        };
        if !allow {
            request.state = Pending::Denied;
            return Ok(true);
        }
        let secret = random_hex(32);
        let mut keys = self.current();
        keys.0.push(Key {
            id: random_hex(6),
            name: request.name.clone(),
            hash: hash(&secret),
            created: now(),
            approved_by: by.to_owned(),
            user: user.map(|(name, _)| name.to_owned()),
            credentials: user.map(|(_, c)| c.to_owned()).unwrap_or_default(),
        });
        self.save(&mut keys)?;
        request.state = Pending::Approved(secret);
        Ok(true)
    }

    /// A key made on the web interface and shown there once (spec §14.2's fallback), for a
    /// program that can't ask for access itself, such as an AI tool. Returns the key.
    pub fn create(
        &self,
        name: &str,
        by: &str,
        user: Option<(&str, &str)>,
    ) -> std::io::Result<String> {
        let secret = random_hex(32);
        let mut keys = self.current();
        keys.0.push(Key {
            id: random_hex(6),
            name: name.to_owned(),
            hash: hash(&secret),
            created: now(),
            approved_by: by.to_owned(),
            user: user.map(|(name, _)| name.to_owned()),
            credentials: user.map(|(_, c)| c.to_owned()).unwrap_or_default(),
        });
        self.save(&mut keys)?;
        Ok(secret)
    }

    /// The program collects its key, once, with its request secret.
    pub fn collect(&self, secret: &str) -> Option<Collect> {
        let mut requests = self.requests.lock().unwrap_or_else(|p| p.into_inner());
        Self::expire(&mut requests);
        let request = requests.get_mut(&hash(secret))?;
        match std::mem::replace(&mut request.state, Pending::Collected) {
            Pending::Waiting => {
                request.state = Pending::Waiting;
                Some(Collect::Waiting)
            }
            Pending::Approved(key) => Some(Collect::Key { key }),
            Pending::Denied => {
                request.state = Pending::Denied;
                Some(Collect::Denied)
            }
            Pending::Collected => None,
        }
    }

    /// The key a program presented, if it is a current one.
    pub fn check(&self, presented: &str) -> Option<Key> {
        let hashed = hash(presented);
        self.current().0.iter().find(|k| k.hash == hashed).cloned()
    }

    pub fn list(&self) -> Vec<Key> {
        self.current()
            .0
            .iter()
            .map(|k| Key {
                hash: String::new(),
                credentials: String::new(),
                ..k.clone()
            })
            .collect()
    }

    /// Removes every key of `user` (removed, or their password changed: spec §14.1).
    pub fn remove_user(&self, user: &str) -> std::io::Result<()> {
        let mut keys = self.current();
        keys.0.retain(|k| k.user.as_deref() != Some(user));
        self.save(&mut keys)
    }

    /// When the first user is created: open-mode keys to keep now belong to that user, and the
    /// rest are removed.
    pub fn adopt(&self, keep: &[String], user: &str, credentials: &str) -> std::io::Result<()> {
        let mut keys = self.current();
        keys.0.retain(|k| keep.contains(&k.id));
        for key in keys.0.iter_mut() {
            key.user = Some(user.to_owned());
            key.credentials = credentials.to_owned();
        }
        self.save(&mut keys)
    }

    pub fn remove(&self, id: &str) -> std::io::Result<bool> {
        let mut keys = self.current();
        let before = keys.0.len();
        keys.0.retain(|k| k.id != id);
        if keys.0.len() == before {
            return Ok(false);
        }
        self.save(&mut keys)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_is_paired_once_and_only_its_secret_collects_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let keys = Keys::open(dir.path()).unwrap();
        let (secret, code) = keys.request("CutCraft on the kitchen MacBook").unwrap();
        assert_eq!(keys.collect(&secret), Some(Collect::Waiting));
        assert_eq!(keys.collect("someone else's guess"), None);
        assert_eq!(keys.waiting()[0].code, code);
        assert!(keys.decide(&code, true, "browser", None).unwrap());
        let Some(Collect::Key { key }) = keys.collect(&secret) else {
            panic!()
        };
        assert_eq!(keys.collect(&secret), None, "handed out once");
        assert_eq!(
            keys.check(&key).unwrap().name,
            "CutCraft on the kitchen MacBook"
        );
        assert!(keys.check("not a key").is_none());

        // Keys survive a restart, stored hashed.
        let stored = std::fs::read_to_string(dir.path().join("keys.json")).unwrap();
        assert!(!stored.contains(&key));
        let reopened = Keys::open(dir.path()).unwrap();
        let id = reopened.check(&key).unwrap().id;
        assert!(reopened.remove(&id).unwrap());
        assert!(reopened.check(&key).is_none());
    }

    #[test]
    fn denied_requests_get_no_key_and_waiting_requests_are_capped() {
        let dir = tempfile::tempdir().unwrap();
        let keys = Keys::open(dir.path()).unwrap();
        let (secret, code) = keys.request("x").unwrap();
        keys.decide(&code, false, "b", None).unwrap();
        assert_eq!(keys.collect(&secret), Some(Collect::Denied));
        for i in 0..MAX_PENDING {
            keys.request(&format!("p{i}")).unwrap();
        }
        assert!(keys.request("one too many").is_err());
    }
}
