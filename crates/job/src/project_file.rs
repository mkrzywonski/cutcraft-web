//! `.cutcraft` project files (spec §5.7): a versioned, self-contained snapshot. The prepared
//! geometry is stored with text already outlined, so a repeat cut needs neither the original
//! file nor its fonts; the original bytes are embedded so preparation can be regenerated.

use std::{io::Write, path::Path};

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::project::{CutRecord, Project};

pub const PROJECT_FILE_SCHEMA_VERSION: u32 = 1;
const FORMAT: &str = "cutcraft-project";

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectFile {
    pub format: String,
    pub schema_version: u32,
    /// The CutCraft version that saved it.
    pub app_version: String,
    pub source: EmbeddedSource,
    pub project: Project,
}

/// The original design file, as opened.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddedSource {
    pub file_name: String,
    /// Where it was when the project was made; it may since have moved or changed.
    pub original_path: String,
    pub sha256: String,
    pub bytes_base64: String,
}

impl EmbeddedSource {
    pub fn new(path: &Path, bytes: &[u8]) -> Self {
        Self {
            file_name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            original_path: path.to_string_lossy().into_owned(),
            sha256: format!("{:x}", Sha256::digest(bytes)),
            bytes_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    /// The embedded bytes, checked against their recorded hash.
    pub fn bytes(&self) -> Result<Vec<u8>, String> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&self.bytes_base64)
            .map_err(|e| format!("The project's copy of the design is damaged: {e}"))?;
        if format!("{:x}", Sha256::digest(&bytes)) != self.sha256 {
            return Err(
                "The project's copy of the design does not match its recorded fingerprint.".into(),
            );
        }
        Ok(bytes)
    }
}

impl ProjectFile {
    pub fn new(
        project: Project,
        source_path: &Path,
        source_bytes: &[u8],
        app_version: &str,
    ) -> Self {
        Self {
            format: FORMAT.into(),
            schema_version: PROJECT_FILE_SCHEMA_VERSION,
            app_version: app_version.into(),
            source: EmbeddedSource::new(source_path, source_bytes),
            project,
        }
    }

    /// Save atomically: an interrupted save leaves the previous file intact. Never overwrites
    /// the design file itself (spec §1: the original is never overwritten).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let original = Path::new(&self.source.original_path);
        if path == original
            || std::fs::canonicalize(path).ok().as_deref()
                == std::fs::canonicalize(original).ok().as_deref()
                && path.exists()
        {
            return Err("Choose a project file different from the design file.".into());
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .map_err(|e| format!("Cannot create the project file: {e}"))?;
        temporary
            .write_all(&bytes)
            .and_then(|_| temporary.as_file().sync_all())
            .map_err(|e| format!("Cannot save the project: {e}"))?;
        temporary
            .persist(path)
            .map_err(|e| format!("Cannot replace the project file: {}", e.error))?;
        Ok(())
    }

    /// Record what has been cut in a saved project, changing nothing else in it: unsaved edits
    /// in the window stay unsaved. Atomic, like [`ProjectFile::save`].
    pub fn record_cut(path: &Path, record: &CutRecord) -> Result<(), String> {
        let mut file = Self::open(path)?;
        file.project.cut_record = record.clone();
        file.save(path)
    }

    /// Open a project. A file from a newer CutCraft is refused (and left untouched) rather than
    /// misread; a damaged or mismatched embedded design is an error.
    pub fn open(path: &Path) -> Result<Self, String> {
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("Cannot open the project: {e}"))?;
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| format!("This is not a readable CutCraft project: {e}"))?;
        if value.get("format").and_then(|f| f.as_str()) != Some(FORMAT) {
            return Err("This is not a CutCraft project file.".into());
        }
        let version = value
            .get("schema_version")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if version > u64::from(PROJECT_FILE_SCHEMA_VERSION) {
            return Err(format!(
                "This project was saved by a newer CutCraft (project format {version}). Update CutCraft to open it; the file has not been changed."
            ));
        }
        let mut file: ProjectFile = serde_json::from_value(value)
            .map_err(|e| format!("This project file is damaged: {e}"))?;
        file.project.merge_sheets();
        file.source.bytes()?;
        if file.source.sha256 != file.project.prepared.source_sha256 {
            return Err("The project's design copy does not match its prepared geometry.".into());
        }
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MachineProfile, PreparedDesign, Recipe};

    fn project(sha: &str) -> Project {
        Project::new(
            PreparedDesign {
                schema_version: 1,
                stage: "preparation".into(),
                cut_ready: true,
                engine_revision: "test".into(),
                source_sha256: sha.into(),
                recipe: Recipe::new(),
                fonts: vec![],
                page: None,
                pieces: vec![],
                cut_lines: vec![],
                changes: vec![],
                pending: vec![],
                diagnostics: vec![],
                stroked_fills: 0,
            },
            MachineProfile::cameo_1(),
        )
    }

    #[test]
    fn a_saved_project_reopens_identically_without_its_design_file() {
        let dir = tempfile::tempdir().unwrap();
        let design = dir.path().join("design.svg");
        let bytes = b"<svg authored/>";
        std::fs::write(&design, bytes).unwrap();
        let sha = format!("{:x}", Sha256::digest(bytes));
        let file = ProjectFile::new(project(&sha), &design, bytes, "0.1.0");
        let saved = dir.path().join("design.cutcraft");
        file.save(&saved).unwrap();
        std::fs::remove_file(&design).unwrap();
        let reopened = ProjectFile::open(&saved).unwrap();
        assert_eq!(reopened, file);
        assert_eq!(reopened.source.bytes().unwrap(), bytes);
    }

    #[test]
    fn newer_damaged_and_foreign_files_are_refused_and_left_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let design = dir.path().join("design.svg");
        let bytes = b"<svg authored/>";
        let sha = format!("{:x}", Sha256::digest(bytes));
        let file = ProjectFile::new(project(&sha), &design, bytes, "0.1.0");
        let path = dir.path().join("p.cutcraft");
        file.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();

        let newer = text.replacen("\"schema_version\": 1", "\"schema_version\": 99", 1);
        std::fs::write(&path, &newer).unwrap();
        let error = ProjectFile::open(&path).unwrap_err();
        assert!(error.contains("newer CutCraft"), "{error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);

        let tampered = text.replace(
            &file.source.bytes_base64,
            &base64::engine::general_purpose::STANDARD.encode(b"other"),
        );
        std::fs::write(&path, tampered).unwrap();
        assert!(
            ProjectFile::open(&path)
                .unwrap_err()
                .contains("fingerprint")
        );

        std::fs::write(&path, "{\"format\":\"something-else\"}").unwrap();
        assert!(
            ProjectFile::open(&path)
                .unwrap_err()
                .contains("not a CutCraft project")
        );
    }

    #[test]
    fn the_design_file_itself_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let design = dir.path().join("design.svg");
        let bytes = b"<svg authored/>";
        std::fs::write(&design, bytes).unwrap();
        let sha = format!("{:x}", Sha256::digest(bytes));
        let file = ProjectFile::new(project(&sha), &design, bytes, "0.1.0");
        assert!(file.save(&design).is_err());
        assert_eq!(std::fs::read(&design).unwrap(), bytes);
    }

    #[test]
    fn recording_a_cut_changes_only_the_cut_record() {
        let dir = tempfile::tempdir().unwrap();
        let design = dir.path().join("design.svg");
        let bytes = b"<svg authored/>";
        let sha = format!("{:x}", Sha256::digest(bytes));
        let file = ProjectFile::new(project(&sha), &design, bytes, "0.1.0");
        let path = dir.path().join("p.cutcraft");
        file.save(&path).unwrap();
        assert!(
            !std::fs::read_to_string(&path)
                .unwrap()
                .contains("cut_record")
        );
        let mut record = CutRecord::default();
        record.complete("sheet/1", "plan-a");
        record.interrupt("sheet/2");
        ProjectFile::record_cut(&path, &record).unwrap();
        let reopened = ProjectFile::open(&path).unwrap();
        assert_eq!(reopened.project.cut_record, record);
        let mut expected = file.clone();
        expected.project.cut_record = record;
        assert_eq!(reopened, expected);
    }
}
