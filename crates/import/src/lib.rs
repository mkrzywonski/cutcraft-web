//! VectorCraft integration boundary: smoke check and M1 import inspection.

mod dxf;
mod inspect;
mod picture;
mod prepare;
mod source;
pub use cutcraft_job as job;
pub use inspect::{
    Diagnostic, Font, FontFace, FontStatus, Inspection, Object, Page, inspect_bytes, inspect_file,
    read_source,
};
pub use prepare::{inspect_and_prepare, open_file, prepare_bytes, prepare_file};
pub use source::{SourceView, source_png};

/// An original fixture, inlined so the smoke check needs no external assets or fonts.
const SMOKE_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg"
    width="25.4mm" height="25.4mm" viewBox="0 0 72 72">
    <path d="M 9 9 H 63 V 63 H 9 Z" fill="black"/>
</svg>"#;

/// Exercise the actual pinned importer without opening any user document.
pub fn smoke_check() -> Result<(), String> {
    let loaded = vectorcraft_engine::cmd::fileio::load("cutcraft-smoke.svg", SMOKE_SVG)
        .map_err(|error| error.to_string())?;
    if loaded.preview_only || !loaded.warnings.is_empty() {
        return Err(format!(
            "SVG smoke import was incomplete: {:?}",
            loaded.warnings
        ));
    }
    let page = loaded
        .doc
        .artboards
        .first()
        .ok_or("SVG smoke import has no page")?;
    if (page.rect.width() - 72.0).abs() > 1e-6 || (page.rect.height() - 72.0).abs() > 1e-6 {
        return Err("SVG smoke import did not preserve a 25.4 mm page".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn pinned_engine_imports_physical_size_svg() {
        super::smoke_check().expect("authored SVG must import with its physical dimensions");
    }
}
