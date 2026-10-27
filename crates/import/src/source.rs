//! The original design as the engine draws it (spec §5.2's before/after view): every page of a
//! vector source rendered on white by VectorCraft's renderer, or the picture itself. It shows
//! what was opened, before Make cut-ready; nothing here is cut.

use std::io::Cursor;
use std::path::Path;
use std::sync::Mutex;

use cutcraft_job::{Decision, PreparedDesign};
use image::{ImageFormat, RgbaImage};

use crate::inspect::MM_PER_PT;

/// The original design, kept ready to draw any region at any zoom (the main window's artwork
/// view). Regions are in prepared-design mm, the same coordinates as the cut lines, so the two
/// views line up exactly.
pub struct SourceView {
    kind: Kind,
}

enum Kind {
    Vector {
        doc: Box<vectorcraft_doc::Document>,
        /// Where prepared (0, 0) is in the document, in points.
        origin_pt: [f64; 2],
        renderer: Box<Mutex<vectorcraft_render::Renderer>>,
    },
    /// A picture at its physical size: `mm_per_px`, and where prepared (0, 0) falls on it, in mm.
    Picture {
        image: RgbaImage,
        mm_per_px: f64,
        origin_mm: [f64; 2],
    },
}

impl SourceView {
    /// Import the original once for drawing, lined up with `prepared`.
    pub fn new(path: &Path, bytes: &[u8], prepared: &PreparedDesign) -> Result<Self, String> {
        let page = prepared.page.as_ref();
        if crate::picture::is_picture(path) {
            let image = crate::picture::rgba(bytes)?;
            // Its width as they gave it; else the page's when the page is the whole picture;
            // else 96 pixels per inch until they answer.
            let answered = prepared
                .recipe
                .decisions
                .iter()
                .rev()
                .find_map(|d| match d {
                    Decision::PictureSize { width_mm } => Some(*width_mm),
                    _ => None,
                });
            let whole_page = page.filter(|p| p.id != "artwork").map(|p| p.width_mm);
            let width_mm = answered
                .or(whole_page)
                .unwrap_or(f64::from(image.width()) * 25.4 / 96.0);
            let mm_per_px = width_mm / f64::from(image.width().max(1));
            // A page fitted to the artwork starts part-way into the picture.
            let origin_mm = page
                .and_then(|p| p.origin_pt)
                .map_or([0.0, 0.0], |[x, y]| [x * MM_PER_PT, y * MM_PER_PT]);
            return Ok(Self {
                kind: Kind::Picture {
                    image,
                    mm_per_px,
                    origin_mm,
                },
            });
        }
        let unit = prepared.recipe.decisions.iter().find_map(|d| match d {
            Decision::DxfUnits { unit } => Some(*unit),
            _ => None,
        });
        let from_file = prepared
            .recipe
            .decisions
            .iter()
            .any(|d| matches!(d, Decision::FontFromFile { .. }));
        let pdf_appearance = prepared
            .recipe
            .decisions
            .iter()
            .rev()
            .find_map(|d| match d {
                Decision::ImportRoute { route } => {
                    Some(*route == cutcraft_job::ImportRoute::PdfAppearance)
                }
                _ => None,
            })
            .unwrap_or(false);
        let imported = crate::inspect::import_route(
            path,
            bytes,
            !pdf_appearance && !from_file,
            from_file,
            unit,
        )?;
        let doc = imported.doc;
        // Older projects did not record the page origin: use the page's artboard.
        let origin_pt = page.and_then(|p| p.origin_pt).unwrap_or_else(|| {
            let index = page
                .and_then(|p| p.id.strip_prefix("page/"))
                .and_then(|i| i.parse::<usize>().ok())
                .unwrap_or(0);
            doc.artboards
                .get(index)
                .or(doc.artboards.first())
                .map_or([0.0, 0.0], |a| [a.rect.x0, a.rect.y0])
        });
        Ok(Self {
            kind: Kind::Vector {
                doc: Box::new(doc),
                origin_pt,
                renderer: Box::new(Mutex::new(vectorcraft_render::Renderer::new())),
            },
        })
    }

    /// The bounds of the source objects `ids` (`source/<layer>/<child>/…`, as diagnostics and
    /// questions name them) in prepared mm, or none when none of them are drawn. A picture is
    /// one object, the whole picture.
    pub fn bounds_mm(&self, ids: &[String]) -> Option<[f64; 4]> {
        match &self.kind {
            Kind::Vector { doc, origin_pt, .. } => {
                let symbols = doc.symbol_boxes();
                ids.iter()
                    .filter_map(|id| {
                        let mut path = id.strip_prefix("source/")?.split('/');
                        let first: usize = path.next()?.parse().ok()?;
                        let mut node = doc.layers.get(first)?;
                        for index in path {
                            node = node.children()?.get(index.parse::<usize>().ok()?)?;
                        }
                        node.visual_bounds_with(&symbols)
                    })
                    .reduce(|a, b| a.union(b))
                    .map(|r| {
                        [
                            (r.x0 - origin_pt[0]) * MM_PER_PT,
                            (r.y0 - origin_pt[1]) * MM_PER_PT,
                            (r.x1 - origin_pt[0]) * MM_PER_PT,
                            (r.y1 - origin_pt[1]) * MM_PER_PT,
                        ]
                    })
            }
            Kind::Picture {
                image,
                mm_per_px,
                origin_mm,
            } => (!ids.is_empty()).then(|| {
                [
                    -origin_mm[0],
                    -origin_mm[1],
                    f64::from(image.width()) * mm_per_px - origin_mm[0],
                    f64::from(image.height()) * mm_per_px - origin_mm[1],
                ]
            }),
        }
    }

    /// Draw `region_mm` (`[x0, y0, x1, y1]`, prepared mm) into `width` × `height` straight RGBA
    /// pixels on white.
    pub fn render(&self, region_mm: [f64; 4], width: u32, height: u32) -> Result<Vec<u8>, String> {
        let (width, height) = (width.clamp(1, 4096), height.clamp(1, 4096));
        let [x0, y0, x1, y1] = region_mm;
        if !(x1 > x0 && y1 > y0) {
            return Err("Nothing to draw.".into());
        }
        match &self.kind {
            Kind::Vector {
                doc,
                origin_pt,
                renderer,
            } => {
                let scale = f64::from(width) / ((x1 - x0) / MM_PER_PT);
                let view = kurbo::Affine::scale(scale)
                    * kurbo::Affine::translate((
                        -(origin_pt[0] + x0 / MM_PER_PT),
                        -(origin_pt[1] + y0 / MM_PER_PT),
                    ));
                let options = vectorcraft_render::RenderOptions {
                    background: Some([255, 255, 255, 255]),
                    skip_templates: true,
                    ..Default::default()
                };
                let mut renderer = renderer
                    .lock()
                    .map_err(|_| "The renderer is unavailable.".to_string())?;
                Ok(renderer
                    .render(doc, width, height, view, &options)
                    .to_straight())
            }
            Kind::Picture {
                image,
                mm_per_px,
                origin_mm,
            } => {
                let (iw, ih) = (f64::from(image.width()), f64::from(image.height()));
                let mut out = vec![255u8; (width * height * 4) as usize];
                for py in 0..height {
                    let y = y0 + (f64::from(py) + 0.5) / f64::from(height) * (y1 - y0);
                    let sy = ((y + origin_mm[1]) / mm_per_px).floor();
                    if !(0.0..ih).contains(&sy) {
                        continue;
                    }
                    for px in 0..width {
                        let x = x0 + (f64::from(px) + 0.5) / f64::from(width) * (x1 - x0);
                        let sx = ((x + origin_mm[0]) / mm_per_px).floor();
                        if !(0.0..iw).contains(&sx) {
                            continue;
                        }
                        let p = image.get_pixel(sx as u32, sy as u32).0;
                        let a = u32::from(p[3]);
                        let i = ((py * width + px) * 4) as usize;
                        for c in 0..3 {
                            // Over white.
                            out[i + c] = ((u32::from(p[c]) * a + 255 * (255 - a)) / 255) as u8;
                        }
                    }
                }
                Ok(out)
            }
        }
    }
}

/// A PNG of the original design, its longest side at most `max_side` pixels, with its size.
pub fn source_png(path: &Path, bytes: &[u8], max_side: u32) -> Result<(Vec<u8>, u32, u32), String> {
    let max_side = max_side.clamp(64, 2048);
    let picture = if crate::picture::is_picture(path) {
        let image = image::DynamicImage::from(crate::picture::rgba(bytes)?);
        image.thumbnail(max_side, max_side).to_rgba8()
    } else {
        let imported = crate::inspect::import_bytes(path, bytes)?;
        let doc = &imported.doc;
        let region = doc
            .artboards
            .iter()
            .map(|a| a.rect)
            .reduce(|a, b| a.union(b))
            .ok_or("The design has no pages to show.")?;
        if !(region.width() > 0.0 && region.height() > 0.0) {
            return Err("The design's page is empty.".into());
        }
        let scale = f64::from(max_side) / region.width().max(region.height());
        let rendered = vectorcraft_render::Renderer::new().render_region(doc, region, scale, true);
        RgbaImage::from_raw(rendered.width, rendered.height, rendered.to_straight())
            .ok_or("The renderer returned an image of the wrong size.")?
    };
    let (width, height) = picture.dimensions();
    let mut png = Vec::new();
    picture
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|e| format!("The picture could not be encoded: {e}"))?;
    Ok((png, width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_original_is_drawn_on_white_with_its_artwork() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50mm" height="25mm" viewBox="0 0 100 50">
            <rect x="0" y="0" width="50" height="50" fill="#ff0000"/>
            <rect x="60" y="10" width="30" height="30" fill="none" stroke="#0000ff" stroke-width="4"/>
        </svg>"##;
        let (png, width, height) = source_png(Path::new("design.svg"), svg, 200).unwrap();
        assert_eq!((width, height), (200, 100));
        let image = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(
            image.get_pixel(50, 50).0,
            [255, 0, 0, 255],
            "the red square"
        );
        assert_eq!(
            image.get_pixel(190, 90).0,
            [255, 255, 255, 255],
            "white page"
        );
        let blue = image
            .pixels()
            .filter(|p| p.0[2] > 200 && p.0[0] < 60)
            .count();
        assert!(
            blue > 100,
            "the stroke is drawn as it appears: {blue} blue pixels"
        );
    }

    #[test]
    fn a_region_of_the_original_lines_up_with_prepared_millimetres() {
        // A 50 × 25 mm page whose left half is red: prepared x 0–25 mm.
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50mm" height="25mm" viewBox="0 0 100 50">
            <rect x="0" y="0" width="50" height="50" fill="#ff0000"/>
        </svg>"##;
        let path = Path::new("design.svg");
        let prepared = crate::prepare_bytes(path, svg, &cutcraft_job::Recipe::new()).unwrap();
        let view = SourceView::new(path, svg, &prepared).unwrap();
        // 20–30 mm across: red on the left half of the picture, white on the right.
        let pixels = view.render([20.0, 0.0, 30.0, 10.0], 100, 100).unwrap();
        let at = |x: usize, y: usize| &pixels[(y * 100 + x) * 4..][..3];
        assert_eq!(at(10, 50), [255, 0, 0]);
        assert_eq!(at(90, 50), [255, 255, 255]);
    }

    #[test]
    fn show_me_finds_an_object_by_its_source_id() {
        // A 50 × 25 mm page with a 10 × 5 mm box 20 mm in, and a second box.
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="50mm" height="25mm" viewBox="0 0 100 50">
            <rect x="40" y="10" width="20" height="10" fill="#ff0000"/>
            <rect x="0" y="30" width="10" height="10" fill="#00ff00"/>
        </svg>"##;
        let path = Path::new("design.svg");
        let prepared = crate::prepare_bytes(path, svg, &cutcraft_job::Recipe::new()).unwrap();
        let view = SourceView::new(path, svg, &prepared).unwrap();
        let near = |a: [f64; 4], b: [f64; 4]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.01);
        let box_ = view.bounds_mm(&["source/0/0".into()]).unwrap();
        assert!(near(box_, [20.0, 5.0, 30.0, 10.0]), "{box_:?}");
        let both = view
            .bounds_mm(&["source/0/0".into(), "source/0/1".into()])
            .unwrap();
        assert!(near(both, [0.0, 5.0, 30.0, 20.0]), "{both:?}");
        assert_eq!(view.bounds_mm(&["source/0/9".into()]), None);
    }

    #[test]
    fn a_picture_is_drawn_at_the_size_she_gave_it() {
        // 200 × 100 px, left half black; they say it is 100 mm wide.
        let mut image = RgbaImage::from_pixel(200, 100, image::Rgba([255, 255, 255, 255]));
        for y in 0..100 {
            for x in 0..100 {
                image.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        let mut png = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        let mut recipe = cutcraft_job::Recipe::new();
        recipe
            .decisions
            .push(Decision::PictureSize { width_mm: 100.0 });
        let path = Path::new("picture.png");
        let prepared = crate::prepare_bytes(path, &png, &recipe).unwrap();
        let view = SourceView::new(path, &png, &prepared).unwrap();
        // 45–55 mm across: black, then white, at the picture's real size.
        let pixels = view.render([45.0, 10.0, 55.0, 20.0], 10, 10).unwrap();
        assert_eq!(&pixels[(5 * 10 + 1) * 4..][..3], [0, 0, 0]);
        assert_eq!(&pixels[(5 * 10 + 8) * 4..][..3], [255, 255, 255]);
    }
}
