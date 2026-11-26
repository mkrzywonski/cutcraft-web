//! The household font archive. Font parsing belongs to a preparation worker; this module
//! stores checked bytes and metadata under the library's snapshot/write lock.
use crate::{
    journal::write_atomic,
    library::{Library, check_name},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fs, path::Path};

pub const DIRECTORY: &str = ".fonts";
pub const MAX_FONT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_FONTS: usize = 128;
const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_INDEX_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Face {
    pub family: String,
    pub style: String,
    pub version: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Font {
    pub id: String,
    pub name: String,
    pub bytes: u64,
    pub faces: Vec<Face>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Archive {
    pub schema: u32,
    pub fonts: Vec<Font>,
}
impl Default for Archive {
    fn default() -> Self {
        Self {
            schema: 1,
            fonts: vec![],
        }
    }
}
pub fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn regular(path: &Path) -> Result<u64, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.file_type().is_file() {
        return Err("Font archive entries must be regular files.".into());
    }
    Ok(meta.len())
}
fn check(archive: &Archive) -> Result<(), String> {
    if archive.schema != 1 || archive.fonts.len() > MAX_FONTS {
        return Err("Unsupported or oversized font archive.".into());
    }
    let mut ids = BTreeSet::new();
    let mut faces = BTreeSet::new();
    let mut total = 0;
    for font in &archive.fonts {
        check_name(&font.name).map_err(|e| e.to_string())?;
        if !valid_id(&font.id)
            || !ids.insert(&font.id)
            || font.bytes == 0
            || font.bytes > MAX_FONT_BYTES as u64
            || font.faces.is_empty()
            || font.faces.len() > 64
        {
            return Err("Invalid font archive entry.".into());
        }
        total += font.bytes;
        for face in &font.faces {
            if [&face.family, &face.style, &face.version]
                .iter()
                .any(|s| s.len() > 512 || s.chars().any(char::is_control))
                || face.family.trim().is_empty()
            {
                return Err("Invalid font face metadata.".into());
            }
            if !faces.insert((
                face.family.to_lowercase(),
                face.style.to_lowercase(),
                face.version.clone(),
            )) {
                return Err(
                    "A font face is already installed. Replace that font explicitly.".into(),
                );
            }
        }
    }
    if total > MAX_ARCHIVE_BYTES {
        return Err("The font archive exceeds 256 MiB.".into());
    }
    Ok(())
}
pub fn read_archive(dir: &Path) -> Result<Archive, String> {
    if !dir.exists() {
        return Ok(Archive::default());
    }
    if !fs::symlink_metadata(dir)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_dir()
    {
        return Err("Invalid font archive directory.".into());
    }
    let index = dir.join("index.json");
    if fs::symlink_metadata(&index).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return Ok(Archive::default());
    }
    if regular(&index)? > MAX_INDEX_BYTES {
        return Err("Font index is too large.".into());
    }
    let archive: Archive = serde_json::from_slice(&fs::read(index).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    check(&archive)?;
    Ok(archive)
}
/// Integrity/shape check only. The server also validates restored font programs in a worker.
pub fn inspect_archive(dir: &Path) -> Result<Archive, String> {
    let archive = read_archive(dir)?;
    if !dir.exists() {
        return Ok(archive);
    }
    regular(&dir.join("index.json"))?;
    let mut expected = BTreeSet::from(["index.json".to_owned()]);
    for font in &archive.fonts {
        let name = format!("{}.ttf", font.id);
        let file = dir.join(&name);
        if regular(&file)? != font.bytes
            || digest(&fs::read(file).map_err(|e| e.to_string())?) != font.id
        {
            return Err("A font archive file is missing or changed.".into());
        }
        expected.insert(name);
    }
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !expected.remove(&entry.file_name().to_string_lossy().into_owned()) {
            return Err("Unexpected file in the font archive.".into());
        }
    }
    if !expected.is_empty() {
        return Err("Incomplete font archive.".into());
    }
    Ok(archive)
}
/// Recover an interrupted manifest publication. Only this managed directory is touched.
pub fn recover_archive(dir: &Path) -> Result<(), String> {
    if !dir.exists() {
        return Ok(());
    }
    let archive = read_archive(dir)?;
    let expected: BTreeSet<_> = archive
        .fonts
        .iter()
        .map(|font| format!("{}.ttf", font.id))
        .chain(["index.json".into()])
        .collect();
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !expected.contains(&entry.file_name().to_string_lossy().into_owned()) {
            if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                return Err("Unexpected directory in the font archive.".into());
            }
            fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
        }
    }
    if !dir.join("index.json").exists() {
        write_atomic(
            &dir.join("index.json"),
            &serde_json::to_vec(&archive).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    inspect_archive(dir).map(|_| ())
}
impl Library {
    pub fn fonts(&self) -> Result<Archive, String> {
        let _guard = self.write.lock().unwrap_or_else(|p| p.into_inner());
        read_archive(&self.root.join(DIRECTORY))
    }
    pub fn font_bytes(&self, id: &str) -> Result<Vec<u8>, String> {
        let _guard = self.write.lock().unwrap_or_else(|p| p.into_inner());
        let dir = self.root.join(DIRECTORY);
        let archive = read_archive(&dir)?;
        let font = archive
            .fonts
            .iter()
            .find(|f| f.id == id)
            .ok_or("Font not found.")?;
        let path = dir.join(format!("{id}.ttf"));
        if regular(&path)? != font.bytes {
            return Err("The stored font changed.".into());
        }
        let bytes = fs::read(path).map_err(|e| e.to_string())?;
        if digest(&bytes) != id {
            return Err("The stored font changed.".into());
        }
        Ok(bytes)
    }
    /// Caller has parsed every face in a bounded preparation worker first.
    pub fn install_font(
        &self,
        name: String,
        bytes: &[u8],
        faces: Vec<Face>,
        replace: Option<&str>,
    ) -> Result<Font, String> {
        if bytes.is_empty() || bytes.len() > MAX_FONT_BYTES {
            return Err("Fonts must be between 1 byte and 16 MiB.".into());
        }
        let _guard = self.lock_write();
        let dir = self.root.join(DIRECTORY);
        let mut archive = read_archive(&dir)?;
        let font = Font {
            id: digest(bytes),
            name,
            bytes: bytes.len() as u64,
            faces,
        };
        if let Some(existing) = archive.fonts.iter().find(|f| f.id == font.id) {
            return Ok(existing.clone());
        }
        if let Some(id) = replace {
            if !archive.fonts.iter().any(|f| f.id == id) {
                return Err("The font to replace changed. Refresh the font list.".into());
            }
            archive.fonts.retain(|f| f.id != id);
        }
        // A changed version of an existing family/style still requires explicit replacement.
        if archive.fonts.iter().any(|f| {
            f.faces.iter().any(|old| {
                font.faces.iter().any(|new| {
                    old.family.eq_ignore_ascii_case(&new.family)
                        && old.style.eq_ignore_ascii_case(&new.style)
                })
            })
        }) {
            return Err("This family and style is already installed. Choose Replace first.".into());
        }
        archive.fonts.push(font.clone());
        archive.fonts.sort_by(|a, b| a.id.cmp(&b.id));
        check(&archive)?;
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        write_atomic(&dir.join(format!("{}.ttf", font.id)), bytes).map_err(|e| e.to_string())?;
        write_atomic(
            &dir.join("index.json"),
            &serde_json::to_vec(&archive).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if let Some(id) = replace {
            fs::remove_file(dir.join(format!("{id}.ttf"))).map_err(|e| e.to_string())?;
        }
        fs::File::open(&dir)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(font)
    }
    pub fn remove_font(&self, id: &str) -> Result<(), String> {
        let _guard = self.lock_write();
        let dir = self.root.join(DIRECTORY);
        let mut archive = read_archive(&dir)?;
        if !archive.fonts.iter().any(|f| f.id == id) {
            return Err("Font not found.".into());
        }
        archive.fonts.retain(|f| f.id != id);
        write_atomic(
            &dir.join("index.json"),
            &serde_json::to_vec(&archive).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        fs::remove_file(dir.join(format!("{id}.ttf"))).map_err(|e| e.to_string())?;
        fs::File::open(&dir)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn face() -> Vec<Face> {
        vec![Face {
            family: "Authored fixture".into(),
            style: "Regular".into(),
            version: "1".into(),
        }]
    }
    #[test]
    fn archive_replacement_is_explicit_and_snapshots_keep_old_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::open(dir.path()).unwrap();
        // Storage tests use opaque bytes; parsing is deliberately a worker responsibility.
        let first = library
            .install_font("fixture.ttf".into(), b"checked-one", face(), None)
            .unwrap();
        let snap = library.snapshot().unwrap();
        assert!(
            library
                .install_font("new.ttf".into(), b"checked-two", face(), None)
                .is_err()
        );
        let next = library
            .install_font("new.ttf".into(), b"checked-two", face(), Some(&first.id))
            .unwrap();
        assert_eq!(library.font_bytes(&next.id).unwrap(), b"checked-two");
        assert_eq!(
            fs::read(snap.join(DIRECTORY).join(format!("{}.ttf", first.id))).unwrap(),
            b"checked-one"
        );
        let staged = crate::backup::inspect(
            &snap,
            &cutcraft_device::gpgl::CAMEO_1,
            &cutcraft_job::MachineProfile::cameo_1(),
            &dir.path().join("inspect"),
        )
        .unwrap();
        assert_eq!(staged.fonts, 1);
        let copy = Library::open(&dir.path().join("restored")).unwrap();
        copy.replace_with(&snap).unwrap();
        assert_eq!(copy.font_bytes(&first.id).unwrap(), b"checked-one");
        assert!(library.list(&[]).unwrap().folders.is_empty());
        assert!(library.font_bytes("../index.json").is_err());
        assert_eq!(
            inspect_archive(&dir.path().join("library").join(DIRECTORY))
                .unwrap()
                .fonts
                .len(),
            1
        );
        library.remove_font(&next.id).unwrap();
        assert!(library.fonts().unwrap().fonts.is_empty());
        assert!(library.changes().0 >= 3);
    }
    #[test]
    fn crash_recovery_removes_only_unpublished_fonts_and_checks_integrity() {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::open(dir.path()).unwrap();
        let font = library
            .install_font("fixture.ttf".into(), b"checked", face(), None)
            .unwrap();
        let fonts = dir.path().join("library").join(DIRECTORY);
        fs::write(fonts.join("orphan.ttf"), b"unpublished").unwrap();
        recover_archive(&fonts).unwrap();
        assert!(!fonts.join("orphan.ttf").exists());
        fs::write(fonts.join(format!("{}.ttf", font.id)), b"changed").unwrap();
        assert!(inspect_archive(&fonts).is_err());
    }
}

#[cfg(test)]
mod path_tests {
    #[test]
    fn only_the_reserved_font_archive_paths_pass_backup_validation() {
        use crate::backup::safe_path;
        assert!(safe_path(".fonts").is_ok());
        assert!(safe_path(".fonts/index.json").is_ok());
        assert!(safe_path(&format!(".fonts/{}.ttf", "a".repeat(64))).is_ok());
        for path in [
            ".fonts/../secret",
            ".fonts/.hidden",
            ".fonts/index.json/child",
            "elsewhere/.fonts/index.json",
            ".fonts/arbitrary.ttf",
        ] {
            assert!(safe_path(path).is_err(), "{path}");
        }
    }
}
