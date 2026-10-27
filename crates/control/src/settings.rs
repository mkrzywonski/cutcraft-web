use std::{
    io::Write,
    path::{Path, PathBuf},
};

use cutcraft_import::job::{MatProfile, Material};
use serde::{Deserialize, Serialize};

pub const DEFAULT_MCP_PORT: u16 = 47833;
/// Recent designs and projects kept for the start screen.
pub const MAX_RECENT: usize = 10;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct McpSettings {
    pub enabled: bool,
    pub port: u16,
    /// The address the service listens on: 127.0.0.1 (this computer only, the default) or one
    /// of this computer's network addresses.
    pub address: String,
    /// AI tools must send one of the access keys (`Authorization: Bearer …`). Always so beyond
    /// this computer.
    pub require_key: bool,
    /// Named access keys, one per AI tool. Only their fingerprints are kept.
    pub keys: Vec<AccessKey>,
}

/// An access key as it is kept: its name, a SHA-256 fingerprint of the key (never the key), the
/// key's last four characters to tell keys apart, and when it was made.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AccessKey {
    pub name: String,
    pub fingerprint: String,
    pub ends_with: String,
    pub created_unix: u64,
}

impl Default for McpSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            port: DEFAULT_MCP_PORT,
            address: LOCAL_ADDRESS.into(),
            require_key: false,
            keys: Vec::new(),
        }
    }
}

/// This computer only.
pub const LOCAL_ADDRESS: &str = "127.0.0.1";

impl McpSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.port == 0 {
            return Err("Choose an MCP port between 1 and 65535.".into());
        }
        let ip: std::net::IpAddr = self
            .address
            .trim()
            .parse()
            .map_err(|_| format!("{} is not an IP address.", self.address))?;
        if ip.is_unspecified() {
            return Err("Choose one of this computer's addresses rather than all of them (0.0.0.0), so connections can be checked.".into());
        }
        if self.keys_required() && self.keys.is_empty() {
            return Err(if ip.is_loopback() {
                "Make an access key before requiring one, or no AI tool can connect.".into()
            } else {
                "Listening beyond this computer needs an access key: make one first.".into()
            });
        }
        Ok(())
    }

    /// The address listened on (this computer only if it does not parse).
    pub fn ip(&self) -> std::net::IpAddr {
        self.address
            .trim()
            .parse()
            .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST))
    }

    /// Only programs on this computer can connect.
    pub fn local_only(&self) -> bool {
        self.ip().is_loopback()
    }

    /// AI tools must send an access key.
    pub fn keys_required(&self) -> bool {
        self.require_key || !self.local_only()
    }

    /// Whether a request with this `Authorization` header may connect: always when no key is
    /// required, else only with a current key.
    pub fn accepts(&self, authorization: Option<&str>) -> bool {
        if !self.keys_required() {
            return true;
        }
        let Some(key) = authorization.and_then(|h| h.strip_prefix("Bearer ")) else {
            return false;
        };
        let fingerprint = fingerprint(key.trim());
        self.keys.iter().any(|k| k.fingerprint == fingerprint)
    }

    pub fn url(&self) -> String {
        match self.ip() {
            std::net::IpAddr::V6(ip) => format!("http://[{ip}]:{}/mcp", self.port),
            ip => format!("http://{ip}:{}/mcp", self.port),
        }
    }
}

/// A new access key: `cutcraft_` and 32 random hex digits.
pub fn new_key() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| format!("No random source: {e}"))?;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("cutcraft_{hex}"))
}

/// The SHA-256 of a key, as kept in the settings.
pub fn fingerprint(key: &str) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(key.as_bytes()))
}

impl AccessKey {
    /// A key named `name` made now, with the key itself (shown to them once).
    pub fn create(name: &str) -> Result<(Self, String), String> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 40 {
            return Err(
                "Give the key a name of up to 40 characters, such as claude or codex.".into(),
            );
        }
        let key = new_key()?;
        let created_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        Ok((
            Self {
                name: name.to_owned(),
                fingerprint: fingerprint(&key),
                ends_with: key[key.len() - 4..].to_owned(),
                created_unix,
            },
            key,
        ))
    }
}

/// Who is using a port, for the AI connection settings (spec §6: several windows each need their
/// own port).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum PortState {
    /// Nothing listens there.
    Free,
    /// This window's own AI connection.
    ThisWindow,
    /// Another CutCraft window.
    AnotherCutcraft { pid: Option<u32> },
    /// Some other program.
    OtherProgram,
    /// It cannot be used (not this computer's address, or not allowed).
    Unavailable { reason: String },
}

/// Who uses `port` on `address`; `own` is where this window's service listens, if running.
pub fn port_state(
    address: std::net::IpAddr,
    port: u16,
    own: Option<(std::net::IpAddr, u16)>,
) -> PortState {
    if own == Some((address, port)) {
        return PortState::ThisWindow;
    }
    match std::net::TcpListener::bind((address, port)) {
        Ok(_) => PortState::Free,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            identify(address, port).unwrap_or(PortState::OtherProgram)
        }
        Err(error) => PortState::Unavailable {
            reason: error.to_string(),
        },
    }
}

/// Ask whoever listens there whether it is CutCraft (`GET /cutcraft`).
fn identify(address: std::net::IpAddr, port: u16) -> Option<PortState> {
    use std::io::Read;
    let timeout = std::time::Duration::from_millis(400);
    let mut stream =
        std::net::TcpStream::connect_timeout(&std::net::SocketAddr::new(address, port), timeout)
            .ok()?;
    stream.set_read_timeout(Some(timeout)).ok()?;
    stream.set_write_timeout(Some(timeout)).ok()?;
    stream
        .write_all(b"GET /cutcraft HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")
        .ok()?;
    let mut reply = Vec::new();
    let _ = stream.take(2048).read_to_end(&mut reply);
    let reply = String::from_utf8_lossy(&reply);
    let body = reply.split("\r\n\r\n").nth(1)?;
    body.starts_with("CutCraft")
        .then(|| PortState::AnotherCutcraft {
            pid: body
                .split_whitespace()
                .skip_while(|w| *w != "pid")
                .nth(1)
                .and_then(|p| p.parse().ok()),
        })
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsFile {
    schema_version: u32,
    mcp: McpSettings,
}

#[derive(Clone)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            path: directory.join("settings.json"),
        }
    }

    pub fn from_environment() -> Result<Self, String> {
        let directory = match std::env::var_os("CUTCRAFT_CONFIG_DIR") {
            Some(directory) => PathBuf::from(directory),
            None => directories::BaseDirs::new()
                .ok_or("Cannot locate your settings folder.")?
                .config_dir()
                .join("cutcraft"),
        };
        Ok(Self::new(directory))
    }
    /// A store with no file, for headless sessions: it loads the defaults and never saves.
    pub fn detached() -> Self {
        Self {
            path: PathBuf::new(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<McpSettings, String> {
        if self.path.as_os_str().is_empty() {
            return Ok(McpSettings::default());
        }
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(McpSettings::default());
            }
            Err(error) => return Err(format!("Cannot read MCP settings: {error}")),
        };
        let file: SettingsFile =
            serde_json::from_slice(&bytes).map_err(|e| format!("Cannot read MCP settings: {e}"))?;
        if file.schema_version != 1 {
            return Err("MCP settings were written by an unsupported version of CutCraft.".into());
        }
        file.mcp.validate()?;
        Ok(file.mcp)
    }

    pub fn save(&self, settings: &McpSettings) -> Result<(), String> {
        settings.validate()?;
        let directory = self
            .path
            .parent()
            .ok_or("The settings path has no folder.")?;
        std::fs::create_dir_all(directory)
            .map_err(|e| format!("Cannot create the settings folder: {e}"))?;
        let bytes = serde_json::to_vec_pretty(&SettingsFile {
            schema_version: 1,
            mcp: settings.clone(),
        })
        .map_err(|e| e.to_string())?;
        write_atomic(&self.path, &bytes)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MaterialsFile {
    schema_version: u32,
    materials: Vec<Material>,
}

/// How lengths are shown and typed in the window (stored values are always mm).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Units {
    #[default]
    Inches,
    Millimetres,
}

/// Snap to grid on the Cut preview: drags land on multiples of `step_mm` on the mat's rulers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Snap {
    pub enabled: bool,
    /// The grid step; none until they set one (¼ in, or 5 mm when lengths are in mm).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_mm: Option<f64>,
}

impl Snap {
    /// The step in use.
    pub fn step(&self, units: Units) -> f64 {
        self.step_mm.unwrap_or(match units {
            Units::Inches => 6.35,
            Units::Millimetres => 5.0,
        })
    }
}

/// The groups of the window's right panel, by title.
pub const PANEL_GROUPS: &[&str] = &[
    "Issues",
    "Info",
    "Artwork size",
    "What to cut",
    "Material",
    "Mat",
    "Media",
    "Cut lines",
    "Cutting aids",
    "Estimate",
];

/// How they arranged the right panel: the groups in their order, the ones locked above the
/// scrolling part so they stay in view, and the ones they opened or closed. Groups not listed
/// keep their usual place and their usual open state.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PanelLayout {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locked: Vec<String>,
    /// Groups they opened (true) or closed (false).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub open: std::collections::BTreeMap<String, bool>,
}

impl PanelLayout {
    /// Every group, in their order, then the rest in their usual order.
    pub fn arranged(&self) -> Vec<&'static str> {
        let mut groups: Vec<&'static str> = Vec::new();
        let named = self
            .order
            .iter()
            .filter_map(|name| PANEL_GROUPS.iter().copied().find(|g| g == name));
        for g in named.chain(PANEL_GROUPS.iter().copied()) {
            if !groups.contains(&g) {
                groups.push(g);
            }
        }
        groups
    }

    pub fn is_locked(&self, group: &str) -> bool {
        self.locked.iter().any(|g| g == group)
    }
}

/// What a newly opened design (not a CutCraft project) starts with: a material and a mat from
/// their libraries, by name. None, or a name no longer in the library, keeps the usual choice.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NewDesign {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mat: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreferencesFile {
    schema_version: u32,
    units: Units,
    #[serde(default)]
    snap: Snap,
    #[serde(default = "default_font_size")]
    font_size: f32,
    #[serde(default)]
    panel: PanelLayout,
    #[serde(default)]
    new_design: NewDesign,
}

fn default_font_size() -> f32 {
    14.0
}

impl Default for PreferencesFile {
    fn default() -> Self {
        Self {
            schema_version: 1,
            units: Units::default(),
            snap: Snap::default(),
            font_size: default_font_size(),
            panel: PanelLayout::default(),
            new_design: NewDesign::default(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MatsFile {
    schema_version: u32,
    mats: Vec<MatProfile>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecentFile {
    schema_version: u32,
    paths: Vec<String>,
}

/// Paired cameo-pi stations (spec §5.11). The keys are secrets: the file is written readable by
/// its owner only, and never shown.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StationsFile {
    schema_version: u32,
    stations: Vec<cutcraft_station::Credentials>,
}

impl SettingsStore {
    pub fn load_stations(&self) -> Result<Vec<cutcraft_station::Credentials>, String> {
        match self.read::<StationsFile>("stations.json")? {
            Some(file) if file.schema_version == 1 => Ok(file.stations),
            Some(_) => {
                Err("stations.json was written by an unsupported version of CutCraft.".into())
            }
            None => Ok(Vec::new()),
        }
    }

    /// Saved through a temporary file, which is created readable by its owner only.
    pub fn save_stations(&self, stations: &[cutcraft_station::Credentials]) -> Result<(), String> {
        self.write(
            "stations.json",
            &StationsFile {
                schema_version: 1,
                stations: stations.to_vec(),
            },
        )
    }

    /// A file beside `settings.json`, or none for a detached store.
    fn sibling(&self, name: &str) -> Option<PathBuf> {
        (!self.path.as_os_str().is_empty()).then(|| self.path.with_file_name(name))
    }

    fn read<T: for<'de> Deserialize<'de>>(&self, name: &str) -> Result<Option<T>, String> {
        let Some(path) = self.sibling(name) else {
            return Ok(None);
        };
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| format!("Cannot read {name}: {e}")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("Cannot read {name}: {error}")),
        }
    }

    fn write<T: Serialize>(&self, name: &str, value: &T) -> Result<(), String> {
        let Some(path) = self.sibling(name) else {
            return Ok(());
        };
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)
                .map_err(|e| format!("Cannot create the settings folder: {e}"))?;
        }
        let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
        write_atomic(&path, &bytes)
    }

    /// Their material library (spec §5.6): the v1 defaults until they change it.
    pub fn load_materials(&self) -> Result<Vec<Material>, String> {
        match self.read::<MaterialsFile>("materials.json")? {
            Some(file) if file.schema_version == 1 => Ok(file.materials),
            Some(_) => {
                Err("materials.json was written by an unsupported version of CutCraft.".into())
            }
            None => Ok(Material::defaults()),
        }
    }

    pub fn save_materials(&self, materials: &[Material]) -> Result<(), String> {
        self.write(
            "materials.json",
            &MaterialsFile {
                schema_version: 1,
                materials: materials.to_vec(),
            },
        )
    }

    fn preferences(&self) -> PreferencesFile {
        self.read::<PreferencesFile>("preferences.json")
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    /// Inches or millimetres (spec §3: inches by default).
    pub fn load_units(&self) -> Units {
        self.preferences().units
    }

    pub fn save_units(&self, units: Units) -> Result<(), String> {
        let preferences = PreferencesFile {
            schema_version: 1,
            units,
            ..self.preferences()
        };
        self.write("preferences.json", &preferences)
    }

    /// Snap to grid on the Cut preview (off by default).
    pub fn load_snap(&self) -> Snap {
        self.preferences().snap
    }

    pub fn save_snap(&self, snap: Snap) -> Result<(), String> {
        let preferences = PreferencesFile {
            schema_version: 1,
            snap,
            ..self.preferences()
        };
        self.write("preferences.json", &preferences)
    }

    pub fn load_font_size(&self) -> f32 {
        let size = self.preferences().font_size;
        if (10.0..=24.0).contains(&size) {
            size
        } else {
            default_font_size()
        }
    }

    pub fn save_font_size(&self, font_size: f32) -> Result<(), String> {
        self.write(
            "preferences.json",
            &PreferencesFile {
                schema_version: 1,
                font_size,
                ..self.preferences()
            },
        )
    }

    /// How they arranged the right panel.
    pub fn load_panel(&self) -> PanelLayout {
        self.preferences().panel
    }

    pub fn save_panel(&self, panel: PanelLayout) -> Result<(), String> {
        self.write(
            "preferences.json",
            &PreferencesFile {
                schema_version: 1,
                panel,
                ..self.preferences()
            },
        )
    }

    /// The material and mat a newly opened design starts with.
    pub fn load_new_design(&self) -> NewDesign {
        self.preferences().new_design
    }

    pub fn save_new_design(&self, new_design: NewDesign) -> Result<(), String> {
        self.write(
            "preferences.json",
            &PreferencesFile {
                schema_version: 1,
                new_design,
                ..self.preferences()
            },
        )
    }

    /// Their mats (spec §4.1): the presets until they change them.
    pub fn load_mats(&self) -> Result<Vec<MatProfile>, String> {
        match self.read::<MatsFile>("mats.json")? {
            Some(file) if file.schema_version == 1 => Ok(file.mats),
            Some(_) => Err("mats.json was written by an unsupported version of CutCraft.".into()),
            None => Ok(MatProfile::presets()),
        }
    }

    pub fn save_mats(&self, mats: &[MatProfile]) -> Result<(), String> {
        self.write(
            "mats.json",
            &MatsFile {
                schema_version: 1,
                mats: mats.to_vec(),
            },
        )
    }

    /// Recent designs and projects, newest first.
    pub fn load_recent(&self) -> Vec<String> {
        self.read::<RecentFile>("recent.json")
            .ok()
            .flatten()
            .map(|f| f.paths)
            .unwrap_or_default()
    }

    pub fn save_recent(&self, paths: &[String]) -> Result<(), String> {
        self.write(
            "recent.json",
            &RecentFile {
                schema_version: 1,
                paths: paths.to_vec(),
            },
        )
    }
}

pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| format!("Cannot create an output file: {e}"))?;
    temporary
        .write_all(bytes)
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|e| format!("Cannot save the output: {e}"))?;
    temporary
        .persist(path)
        .map_err(|e| format!("Cannot replace the output file: {}", e.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_keys_are_kept_as_fingerprints_and_checked() {
        let mut local = McpSettings::default();
        assert!(local.validate().is_ok() && local.local_only());
        assert_eq!(local.url(), "http://127.0.0.1:47833/mcp");
        assert!(
            local.accepts(None),
            "no key needed by default on this computer"
        );
        local.require_key = true;
        assert!(local.validate().unwrap_err().contains("Make an access key"));
        let (claude, key) = AccessKey::create(" claude ").unwrap();
        assert_eq!(claude.name, "claude");
        assert!(key.starts_with("cutcraft_") && key.len() == 41);
        assert_ne!(claude.fingerprint, key, "the key itself is never kept");
        assert!(key.ends_with(&claude.ends_with));
        local.keys.push(claude);
        assert!(local.validate().is_ok());
        assert!(local.accepts(Some(&format!("Bearer {key}"))));
        assert!(!local.accepts(Some("Bearer cutcraft_wrong")));
        assert!(!local.accepts(None));
        assert!(!local.accepts(Some(&key)), "only as a bearer key");
        assert!(AccessKey::create("  ").is_err());

        let mut network = McpSettings {
            address: "192.168.1.20".into(),
            ..McpSettings::default()
        };
        assert!(network.keys_required());
        assert!(network.validate().unwrap_err().contains("access key"));
        network.keys = local.keys.clone();
        assert!(network.validate().is_ok());
        assert_eq!(network.url(), "http://192.168.1.20:47833/mcp");
        let everywhere = McpSettings {
            address: "0.0.0.0".into(),
            ..network.clone()
        };
        assert!(everywhere.validate().is_err());
        let nonsense = McpSettings {
            address: "my laptop".into(),
            ..McpSettings::default()
        };
        assert!(nonsense.validate().is_err());
    }

    #[test]
    fn a_free_port_is_free_and_a_taken_one_is_not() {
        let local = std::net::IpAddr::from(std::net::Ipv4Addr::LOCALHOST);
        let taken = std::net::TcpListener::bind((local, 0)).unwrap();
        let port = taken.local_addr().unwrap().port();
        assert_eq!(port_state(local, port, None), PortState::OtherProgram);
        drop(taken);
        assert_eq!(port_state(local, port, None), PortState::Free);
    }
    #[test]
    fn defaults_and_disabled_custom_port_survive_restart() {
        let directory = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(directory.path().to_owned());
        assert_eq!(store.load().unwrap(), McpSettings::default());
        let settings = McpSettings {
            enabled: false,
            port: 49123,
            ..McpSettings::default()
        };
        store.save(&settings).unwrap();
        assert_eq!(
            SettingsStore::new(directory.path().to_owned())
                .load()
                .unwrap(),
            settings
        );
        assert!(
            store
                .save(&McpSettings {
                    enabled: true,
                    port: 0,
                    ..McpSettings::default()
                })
                .is_err()
        );
        assert_eq!(store.load().unwrap(), settings);
    }
    #[test]
    fn materials_and_recent_files_live_beside_the_settings() {
        let directory = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(directory.path().to_owned());
        assert_eq!(store.load_materials().unwrap(), Material::defaults());
        let mut materials = Material::defaults();
        materials[0].force = 14;
        store.save_materials(&materials).unwrap();
        assert_eq!(store.load_materials().unwrap()[0].force, 14);
        store.save_recent(&["a.svg".into()]).unwrap();
        assert_eq!(store.load_recent(), ["a.svg"]);
        let detached = SettingsStore::detached();
        detached.save_materials(&materials).unwrap();
        assert_eq!(detached.load_materials().unwrap(), Material::defaults());
    }
    #[test]
    fn malformed_settings_are_reported_and_not_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let store = SettingsStore::new(directory.path().to_owned());
        std::fs::write(store.path(), b"broken").unwrap();
        assert!(store.load().is_err());
        assert_eq!(std::fs::read(store.path()).unwrap(), b"broken");
    }
}
