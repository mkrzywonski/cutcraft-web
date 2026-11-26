//! Font parsing and file extraction, called only inside a preparation worker.
use base64::Engine;
use cameo_pi_core::fonts::{Archive, Face, MAX_FONT_BYTES, digest, read_archive};
use serde_json::{Value, json};
use std::{path::Path, sync::OnceLock};
use vectorcraft_text::FontDb;

fn face_id(family: &str, style: &str) -> String {
    digest(
        serde_json::to_string(&(family, style))
            .expect("font names serialize")
            .as_bytes(),
    )
}
fn bundled() -> &'static FontDb {
    static DB: OnceLock<FontDb> = OnceLock::new();
    DB.get_or_init(|| FontDb::with_font_dirs(vec![]))
}
fn library_face<'a>(
    archive: &'a Archive,
    family: &str,
    style: &str,
) -> Option<&'a cameo_pi_core::fonts::Font> {
    archive.fonts.iter().find(|font| {
        font.faces.iter().any(|face| {
            face.family.eq_ignore_ascii_case(family) && face.style.eq_ignore_ascii_case(style)
        })
    })
}
/// The catalog remains metadata-only. Loading every system font's bytes just to list it
/// would exceed a small worker's memory on machines with many large CJK font files.
pub fn catalog(library: &Path, inspection: &Value) -> Result<Value, String> {
    let archive = read_archive(library)?;
    let db = FontDb::global();
    let mut fonts = vec![];
    for family in db.family_list().iter() {
        for style in db.styles(family) {
            if fonts.len() >= 65536 {
                return Err("Too many font faces to list.".into());
            }
            let installed = library_face(&archive, family, &style);
            let origin = if bundled().has_family(family)
                && bundled().styles(family).iter().any(|s| s == &style)
            {
                "bundled"
            } else if installed.is_some() {
                "library"
            } else {
                "system"
            };
            fonts.push(
                json!({"id":face_id(family,&style),"family":family,"style":style,"origin":origin,
                "library_id":installed.map(|font|&font.id)}),
            );
        }
    }
    let project:Vec<_>=inspection["fonts"].as_array().into_iter().flatten().map(|font|{
        let family=font["family"].as_str().unwrap_or("");let style=font["style"].as_str().unwrap_or("Regular");
        let available=fonts.iter().find(|entry|entry["family"].as_str().is_some_and(|f|f.eq_ignore_ascii_case(family)) && entry["style"].as_str().is_some_and(|s|s.eq_ignore_ascii_case(style)));
        json!({"family":family,"style":style,"available":available.is_some(),"download_id":available.map(|entry|entry["id"].clone())})
    }).collect();
    Ok(json!({"fonts":fonts,"installed":archive.fonts,"project_fonts":project}))
}
pub fn inspect(area: &Path, data_base64: &str) -> Result<Vec<Face>, String> {
    if data_base64.len() > MAX_FONT_BYTES.div_ceil(3) * 4 {
        return Err("The font exceeds 16 MiB.".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_base64)
        .map_err(|_| "Invalid font bytes.")?;
    if bytes.is_empty() || bytes.len() > MAX_FONT_BYTES {
        return Err("The font is empty or too large.".into());
    }
    let dir = area.join(format!("font-check-{}", crate::keys::random_hex(8)));
    std::fs::create_dir(&dir).map_err(|e| e.to_string())?;
    let result = (|| {
        std::fs::write(dir.join("candidate.ttf"), &bytes).map_err(|e| e.to_string())?;
        let db = FontDb::with_font_dirs(vec![dir.clone()]);
        let mut faces = vec![];
        for family in db.family_list().iter() {
            for style in db.styles(family) {
                if let Some(face) = db.face(family, &style) {
                    for version in db.versions(&face) {
                        if version.path().is_some_and(|path| path.starts_with(&dir)) {
                            let entry = Face {
                                family: version.family.clone(),
                                style: version.style.clone(),
                                version: version.version.clone(),
                            };
                            if !faces.contains(&entry) {
                                faces.push(entry);
                            }
                        }
                    }
                }
            }
        }
        if faces.is_empty() {
            return Err(
                "No supported font faces found, or this font is already bundled with CutCraft."
                    .into(),
            );
        }
        if faces.len() > 64 {
            return Err("The font contains more than 64 faces.".into());
        }
        Ok(faces)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}
pub fn download(id: &str) -> Result<(String, String), String> {
    let db = FontDb::global();
    for family in db.family_list().iter() {
        for style in db.styles(family) {
            if face_id(family, &style) != id {
                continue;
            }
            let face = db
                .face(family, &style)
                .ok_or("This font is no longer available.")?;
            if face.file_data().len() > 40 * 1024 * 1024 {
                return Err("This font is too large to download.".into());
            }
            return Ok((
                if face.file_data().starts_with(b"OTTO") {
                    "font.otf"
                } else if face.file_data().starts_with(b"ttcf") {
                    "font.ttc"
                } else {
                    "font.ttf"
                }
                .into(),
                base64::engine::general_purpose::STANDARD.encode(face.file_data()),
            ));
        }
    }
    Err("Font not found. Refresh the font list.".into())
}
