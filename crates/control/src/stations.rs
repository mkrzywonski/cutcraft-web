//! Paired cameo-pi stations (spec §5.11): pairing, the stored keys, and each Pi's cutters and
//! library. The keys stay here and in `stations.json`; views and command results carry only a
//! Pi's address and its certificate's fingerprint.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cutcraft_station::{Collected, Credentials, Error as StationError, Pairing, Station};
use serde::Serialize;
use serde_json::json;

use crate::commands::CommandError;
use crate::{ActionOutput, Controller};

/// The Pis as the window and automation see them.
#[derive(Clone, Debug, Default, Serialize)]
pub struct StationsView {
    pub paired: Vec<StationView>,
    /// The latest pairing request, until another starts.
    pub pairing: Option<PairingView>,
    /// The Pis found on the network at the last look (`station.discover`).
    pub found: Vec<FoundStation>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FoundStation {
    /// What it calls itself: "cameo-pi on cameopi".
    pub name: String,
    /// The address to pair with.
    pub address: String,
    pub ips: Vec<String>,
    /// Already paired.
    pub paired: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct StationView {
    /// As the user gave it.
    pub address: String,
    /// The certificate pinned at pairing, as `cameo-pi fingerprint` prints it.
    pub fingerprint: String,
    /// Its cutters, as last seen.
    pub cutters: Vec<RemoteCutter>,
    pub version: Option<String>,
    /// Why the last look at it failed.
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RemoteCutter {
    pub id: String,
    /// The name its user gave it on the Pi.
    pub name: String,
    pub online: bool,
    /// The Pi's practice cutter, a Simulated Cameo.
    pub practice: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct PairingView {
    pub address: String,
    /// The code the Pi's web interface shows beside the request.
    pub code: String,
    pub fingerprint: String,
    pub state: PairingState,
    pub message: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PairingState {
    Waiting,
    Paired,
    Denied,
    Expired,
    Failed,
}

/// The keys, and which pairing is current.
#[derive(Default)]
pub(crate) struct Stations {
    pub credentials: Vec<Credentials>,
    /// Projects opened from a Pi's library: path → (the Pi's address, the job's id). Cutting or
    /// sending them adds a revision of that job.
    pub origins: std::collections::HashMap<String, (String, String)>,
    /// What was last sent to each Pi's material and mat lists (address → fingerprint), so they
    /// go again only when they change.
    pub synced: std::collections::HashMap<String, String>,
    /// Bumped by each pairing and cancel: an older pairing's collector stops.
    pub pairing: u64,
}

pub(crate) fn fail(error: impl std::fmt::Display) -> CommandError {
    CommandError::new("failed", error.to_string())
}

/// How CutCraft names itself to the Pi: "CutCraft on <this computer>".
fn program_name() -> String {
    let from_env = ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .find_map(|name| std::env::var(name).ok());
    #[cfg(target_os = "macos")]
    let from_env = from_env.or_else(|| {
        std::process::Command::new("scutil")
            .args(["--get", "ComputerName"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
    });
    let name = from_env
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "this computer".into());
    format!("CutCraft on {name}")
}

/// The Pi's own name from an address: `cameopi` for `cameopi.local` or `cameopi.local:443`.
pub(crate) fn host_name(address: &str) -> String {
    let host = address.trim().trim_start_matches("https://");
    let host = host.split(':').next().unwrap_or(host);
    host.strip_suffix(".local").unwrap_or(host).to_owned()
}

fn same_address(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

impl Controller {
    pub(crate) fn stations(&self) -> Result<std::sync::MutexGuard<'_, Stations>, CommandError> {
        self.0
            .stations
            .lock()
            .map_err(|_| fail("The paired cameo-pi list is unavailable."))
    }

    /// The paired Pi at `address`, or the only one when no address is given.
    pub(crate) fn station(&self, address: Option<&str>) -> Result<Arc<Station>, CommandError> {
        let stations = self.stations()?;
        let credentials = match address {
            Some(address) => stations
                .credentials
                .iter()
                .find(|c| same_address(&c.address, address))
                .ok_or_else(|| {
                    CommandError::new(
                        "invalid-params",
                        format!("CutCraft isn't paired with a cameo-pi at {address:?}."),
                    )
                })?,
            None => match stations.credentials.as_slice() {
                [only] => only,
                [] => {
                    return Err(CommandError::new(
                        "disabled",
                        "No cameo-pi is paired. Add one first.",
                    ));
                }
                _ => {
                    return Err(CommandError::new(
                        "invalid-params",
                        "Several cameo-pis are paired: say which (station).",
                    ));
                }
            },
        };
        Station::connect(credentials.clone())
            .map(Arc::new)
            .map_err(fail)
    }

    /// Asks the Pi at `address` for access. The answer is collected in the background; the
    /// pairing view follows it.
    pub(crate) fn station_pair(&self, address: &str) -> Result<ActionOutput, CommandError> {
        let pairing = Pairing::begin(address, &program_name()).map_err(fail)?;
        let generation = {
            let mut stations = self.stations()?;
            stations.pairing += 1;
            stations.pairing
        };
        let view = PairingView {
            address: address.trim().to_owned(),
            code: pairing.code.clone(),
            fingerprint: pairing.fingerprint.to_string(),
            state: PairingState::Waiting,
            message: Some(format!(
                "On cameo-pi's web page, allow \"{}\" with code {}.",
                program_name(),
                pairing.code
            )),
        };
        self.change(|s| s.stations.pairing = Some(view.clone()))
            .map_err(fail)?;
        let controller = self.clone();
        std::thread::spawn(move || controller.collect_pairing(pairing, generation));
        Ok(ActionOutput::json(json!(view)))
    }

    fn collect_pairing(&self, pairing: Pairing, generation: u64) {
        // The Pi forgets a request after 10 minutes.
        let deadline = Instant::now() + Duration::from_secs(11 * 60);
        loop {
            std::thread::sleep(Duration::from_millis(1500));
            if self.stations().map_or(true, |s| s.pairing != generation) {
                return;
            }
            let (state, message) = match pairing.collect() {
                Ok(Collected::Waiting) | Err(StationError::Unreachable(_))
                    if Instant::now() < deadline =>
                {
                    continue;
                }
                Ok(Collected::Waiting | Collected::Expired) => (
                    PairingState::Expired,
                    "The request ran out before anyone allowed it. Try again.".to_owned(),
                ),
                Ok(Collected::Denied) => (
                    PairingState::Denied,
                    "cameo-pi's web page said Deny.".to_owned(),
                ),
                Ok(Collected::Paired(credentials)) => match self.remember(credentials) {
                    Ok(()) => (PairingState::Paired, "Paired.".to_owned()),
                    Err(error) => (PairingState::Failed, error),
                },
                Err(error) => (PairingState::Failed, error.to_string()),
            };
            if self.stations().map_or(true, |s| s.pairing != generation) {
                return;
            }
            let _ = self.change(|s| {
                if let Some(view) = &mut s.stations.pairing {
                    view.state = state;
                    view.message = Some(message);
                }
            });
            if state == PairingState::Paired {
                let _ = self.station_list();
            }
            return;
        }
    }

    /// Sends the material and mat lists to every paired Pi that doesn't have these ones yet, in
    /// the background (cameo-pi spec §11). Automatic: pairing, looking at the Pis, connecting,
    /// sending and saving a material or mat all call it.
    pub(crate) fn sync_catalog(&self) {
        let controller = self.clone();
        std::thread::spawn(move || controller.sync_catalog_now());
    }

    fn sync_catalog_now(&self) {
        use sha2::{Digest, Sha256};
        let Ok(snapshot) = self.snapshot() else {
            return;
        };
        let materials = json!(snapshot.materials);
        let mats = json!(snapshot.mats);
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(format!("{materials}\n{mats}").as_bytes())
        );
        let Ok(list) = self.stations().map(|s| {
            s.credentials
                .iter()
                .filter(|c| s.synced.get(&c.address) != Some(&fingerprint))
                .cloned()
                .collect::<Vec<_>>()
        }) else {
            return;
        };
        for credentials in list {
            let address = credentials.address.clone();
            let sent = Station::connect(credentials)
                .and_then(|station| station.put_catalog(&materials, &mats, &program_name()));
            if sent.is_ok()
                && let Ok(mut stations) = self.stations()
            {
                stations.synced.insert(address, fingerprint.clone());
            }
        }
    }

    /// Keeps a new pairing, replacing an older one for the same address or the same Pi.
    fn remember(&self, credentials: Credentials) -> Result<(), String> {
        let list = {
            let mut stations = self.stations().map_err(|e| e.message)?;
            stations.credentials.retain(|c| {
                !same_address(&c.address, &credentials.address)
                    && c.fingerprint != credentials.fingerprint
            });
            // A new key starts with no lists on the Pi: send them again.
            stations.synced.remove(&credentials.address);
            stations.credentials.push(credentials);
            stations.credentials.clone()
        };
        self.0
            .store
            .save_stations(&list)
            .map_err(|e| format!("Paired, but CutCraft couldn't save it: {e}"))?;
        self.show_stations(&list, None);
        Ok(())
    }

    /// Updates the paired list in the view: every Pi, with what `looked` found for each.
    fn show_stations(&self, list: &[Credentials], looked: Option<Vec<StationView>>) {
        let _ = self.change(|s| {
            let before = std::mem::take(&mut s.stations.paired);
            s.stations.paired = list
                .iter()
                .map(|c| {
                    let fresh = looked
                        .as_ref()
                        .and_then(|l| l.iter().find(|v| same_address(&v.address, &c.address)));
                    let old = before.iter().find(|v| same_address(&v.address, &c.address));
                    fresh.or(old).cloned().unwrap_or_else(|| StationView {
                        address: c.address.clone(),
                        fingerprint: c.fingerprint.to_string(),
                        cutters: Vec::new(),
                        version: None,
                        error: None,
                    })
                })
                .collect();
        });
    }

    /// Looks at every paired Pi: its version and cutters, or why it can't be reached.
    pub(crate) fn station_list(&self) -> Result<ActionOutput, CommandError> {
        let list = self.stations()?.credentials.clone();
        let looked: Vec<StationView> = list
            .iter()
            .map(|credentials| {
                let state = Station::connect(credentials.clone()).and_then(|s| s.state());
                let (cutters, version, error) = match state {
                    Ok(state) => (
                        state
                            .cutters
                            .iter()
                            .map(|c| RemoteCutter {
                                id: c.id.clone(),
                                name: c.view.name.clone(),
                                online: c.view.online,
                                practice: c.simulated,
                            })
                            .collect(),
                        Some(state.version),
                        None,
                    ),
                    Err(error) => (Vec::new(), None, Some(error.to_string())),
                };
                StationView {
                    address: credentials.address.clone(),
                    fingerprint: credentials.fingerprint.to_string(),
                    cutters,
                    version,
                    error,
                }
            })
            .collect();
        self.show_stations(&list, Some(looked.clone()));
        self.sync_catalog();
        Ok(ActionOutput::json(json!({ "stations": looked })))
    }

    /// Looks for cameo-pis on the network (Bonjour) for about three seconds.
    pub(crate) fn station_discover(&self) -> Result<ActionOutput, CommandError> {
        let found = cutcraft_station::discover(Duration::from_secs(3)).map_err(fail)?;
        let paired = self.stations()?.credentials.clone();
        let found: Vec<FoundStation> = found
            .into_iter()
            .map(|f| FoundStation {
                paired: paired.iter().any(|c| same_address(&c.address, &f.address)),
                name: f.name,
                address: f.address,
                ips: f.ips,
            })
            .collect();
        self.change(|s| s.stations.found.clone_from(&found))
            .map_err(fail)?;
        Ok(ActionOutput::json(json!({ "found": found })))
    }

    /// Forgets a paired Pi, or cancels a pairing request to it.
    pub(crate) fn station_forget(&self, address: &str) -> Result<ActionOutput, CommandError> {
        let pairing = self
            .snapshot()
            .ok()
            .and_then(|s| s.stations.pairing)
            .is_some_and(|p| same_address(&p.address, address) && p.state == PairingState::Waiting);
        let (list, cancelled) = {
            let mut stations = self.stations()?;
            let before = stations.credentials.len();
            stations
                .credentials
                .retain(|c| !same_address(&c.address, address));
            let forgot = stations.credentials.len() != before;
            stations.synced.retain(|a, _| !same_address(a, address));
            if pairing {
                stations.pairing += 1;
            }
            if !forgot && !pairing {
                return Err(CommandError::new(
                    "invalid-params",
                    format!("CutCraft isn't paired with a cameo-pi at {address:?}."),
                ));
            }
            (stations.credentials.clone(), pairing)
        };
        if self.cutting_on_station(address) {
            let _ = self.device_disconnect();
        }
        self.0.store.save_stations(&list).map_err(fail)?;
        self.show_stations(&list, None);
        if cancelled {
            let _ = self.change(|s| s.stations.pairing = None);
        }
        Ok(ActionOutput::json(
            json!({ "forgotten": address, "pairing_cancelled": cancelled }),
        ))
    }

    /// The job a project opened from `address`'s library came from.
    pub(crate) fn origin(&self, path: &str, address: &str) -> Option<String> {
        let stations = self.stations().ok()?;
        let (from, job) = stations.origins.get(path)?;
        same_address(from, address).then(|| job.clone())
    }

    /// File ▸ Open From cameo-pi: a job's project, saved in the settings folder's
    /// `cameo-pi-projects` and opened as a project.
    pub(crate) fn station_open(
        &self,
        address: Option<&str>,
        job_id: &str,
        revision: Option<u32>,
    ) -> Result<ActionOutput, CommandError> {
        self.check_not_cutting()?;
        let station = self.station(address)?;
        let job = station.job(job_id).map_err(fail)?;
        let revision = revision.unwrap_or(job.current_revision);
        let bytes = station.file(job_id, revision, "project").map_err(fail)?;
        let settings = self.0.store.path();
        let dir = if settings.as_os_str().is_empty() {
            std::env::temp_dir().join("cutcraft-cameo-pi-projects")
        } else {
            settings.with_file_name("cameo-pi-projects")
        };
        std::fs::create_dir_all(&dir).map_err(fail)?;
        let safe: String = job
            .name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || " -_().".contains(c) {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let path = dir.join(format!("{}.cutcraft", safe.trim().trim_start_matches('.')));
        crate::settings::write_atomic(&path, &bytes).map_err(fail)?;
        self.open_project(&path).map_err(fail)?;
        let key = path.to_string_lossy().into_owned();
        self.stations()?.origins.insert(
            key.clone(),
            (station.credentials().address.clone(), job_id.to_owned()),
        );
        Ok(ActionOutput::json(json!({
            "path": key,
            "job_id": job_id,
            "revision": revision,
        })))
    }

    /// One folder of a Pi's library.
    pub(crate) fn station_library(
        &self,
        address: Option<&str>,
        folder: &[String],
    ) -> Result<ActionOutput, CommandError> {
        let listing = self.station(address)?.library(folder).map_err(fail)?;
        Ok(ActionOutput::json(json!({
            "folder": folder,
            "folders": listing.folders,
            "jobs": listing.jobs.iter().map(|j| json!({
                "id": j.id, "name": j.name, "notes": j.notes,
                "revision": j.current_revision, "times_cut": j.times_cut,
            })).collect::<Vec<_>>(),
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pi_is_named_after_its_host() {
        assert_eq!(host_name("cameopi.local"), "cameopi");
        assert_eq!(host_name("https://cameopi.local:8443"), "cameopi");
        assert_eq!(host_name("10.0.0.5"), "10.0.0.5");
    }

    #[test]
    fn cutcraft_names_itself_after_this_computer() {
        assert!(program_name().starts_with("CutCraft on "));
    }
}
