//! Picture files (spec §5.1.2): decode, size and trace a PNG, JPEG, GIF, BMP or PSD design into an
//! imported document, which Make cut-ready then prepares like any vector source.

use std::{path::Path, sync::Arc};

use cutcraft_job::{Diagnostic, TraceColours, TraceSettings};
use kurbo::Affine;
use sha2::{Digest, Sha256};
use vectorcraft_color::{Color, Paint};
use vectorcraft_doc::{Appearance, AppearanceItem, Document, Node, appearance::FillLayer};
use vectorcraft_engine::cmd::fileio::psdread;
use vectorcraft_trace::{Raster, TraceParams};

use crate::inspect::{ENGINE_REVISION, Imported, Inspection, MM_PER_PT, Object, Page};

/// Decoding and tracing budget: larger pictures are refused with a message, not traced slowly.
const MAX_PIXELS: u64 = 40_000_000;
/// A trace with more anchors than this is refused (spec §5.9: bounded preparation work).
const MAX_ANCHORS: usize = 250_000;
/// Below this many pixels per millimetre of cut size, edges become visibly stepped.
const LOW_PIXELS_PER_MM: f64 = 4.0;
/// Flat artwork: its four commonest (coarsely quantized) colours cover at least this share of
/// its opaque pixels. Photos and shirt mockups fall below it.
const FLAT_SHARE: f64 = 0.9;
/// More distinct RGB colours than this also asks for confirmation before tracing.
const MAX_FLAT_COLOURS: usize = 20;
/// Pixels at least this opaque count towards a picture's colours.
const OPAQUE: u8 = 250;
/// Light art on transparency: at least this share of the visible pixels is lighter than the
/// one-colour threshold, so a trace of the dark pixels would find almost nothing.
const LIGHT_ART_SHARE: f64 = 0.9;

pub(crate) fn is_picture(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "psd" | "psb"
        )
    })
}

/// Animated pictures with more frames than this are refused rather than decoded.
const MAX_FRAMES: usize = 500;

pub(crate) struct Picture {
    pub raster: Raster,
    /// How many frames the picture has (more than 1 only for an animated GIF).
    pub frames: usize,
    /// Stated resolution in pixels per inch, when the file states a usable one.
    pub ppi: Option<f64>,
    /// Share of opaque pixels in the four commonest colours.
    pub flat_share: f64,
    /// Exact RGB count exceeds the artwork limit, ignoring mostly transparent pixels.
    pub many_colours: bool,
}

/// Decode one frame (the first unless `frame` names another) of a PNG, JPEG, GIF or BMP, or a
/// Photoshop document's merged image (its layers flattened, as Photoshop saves it).
pub(crate) fn decode(bytes: &[u8], frame: Option<usize>) -> Result<Picture, String> {
    if psdread::is_psd(bytes) {
        return decode_psd(bytes, frame);
    }
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("Cannot read this picture: {e}"))?;
    let format = reader.format();
    if !matches!(
        format,
        Some(
            image::ImageFormat::Png
                | image::ImageFormat::Jpeg
                | image::ImageFormat::Gif
                | image::ImageFormat::Bmp
        )
    ) {
        return Err("This picture is not a PNG, JPEG, GIF or BMP file.".into());
    }
    let (width, height) = reader
        .into_dimensions()
        .map_err(|e| format!("Cannot read this picture: {e}"))?;
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(format!(
            "This picture is {width} × {height} pixels, more than the {} megapixels supported. Save a smaller copy.",
            MAX_PIXELS / 1_000_000
        ));
    }
    let (rgba, frames) = if format == Some(image::ImageFormat::Gif) {
        gif_frame(
            bytes,
            frame.unwrap_or(0),
            u64::from(width) * u64::from(height),
        )?
    } else {
        if frame.is_some_and(|f| f > 0) {
            return Err("This picture has only one frame.".into());
        }
        let image =
            image::load_from_memory(bytes).map_err(|e| format!("Cannot read this picture: {e}"))?;
        (image.to_rgba8(), 1)
    };
    let ppi = match format {
        Some(image::ImageFormat::Png) => png_ppi(bytes),
        Some(image::ImageFormat::Jpeg) => jfif_ppi(bytes),
        Some(image::ImageFormat::Bmp) => bmp_ppi(bytes),
        // GIF states no resolution.
        _ => None,
    };
    let flat_share = flat_share(&rgba);
    let many_colours = has_many_colours(&rgba);
    Ok(Picture {
        raster: Raster::new(width, height, rgba.into_raw()),
        frames,
        ppi,
        flat_share,
        many_colours,
    })
}

/// The whole picture for drawing (a GIF's first frame, a Photoshop document's merged image).
pub(crate) fn rgba(bytes: &[u8]) -> Result<image::RgbaImage, String> {
    if psdread::is_psd(bytes) {
        return psd_rgba(bytes);
    }
    image::load_from_memory(bytes)
        .map(|image| image.to_rgba8())
        .map_err(|e| format!("The picture could not be read: {e}"))
}

fn psd_rgba(bytes: &[u8]) -> Result<image::RgbaImage, String> {
    let cms = vectorcraft_color::cms::active();
    psdread::decode(bytes, MAX_PIXELS * 16, |c| cms.cmyk_to_srgb(c, false))
        .map_err(|e| format!("Cannot read this Photoshop file: {e}. Save it as PNG in Photoshop."))
}

/// A PSD or PSB through the engine's reader: its merged image, CMYK documents in the active
/// colour settings, at the resolution the document states.
fn decode_psd(bytes: &[u8], frame: Option<usize>) -> Result<Picture, String> {
    if frame.is_some_and(|f| f > 0) {
        return Err("This picture has only one frame.".into());
    }
    let rgba = psd_rgba(bytes)?;
    let (width, height) = rgba.dimensions();
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(format!(
            "This picture is {width} × {height} pixels, more than the {} megapixels supported. Save a smaller copy.",
            MAX_PIXELS / 1_000_000
        ));
    }
    let ppi = psdread::resolution(bytes)
        .map(|(x, _)| x)
        .filter(|ppi| ppi.is_finite() && *ppi > 1.0);
    let flat_share = flat_share(&rgba);
    let many_colours = has_many_colours(&rgba);
    Ok(Picture {
        raster: Raster::new(width, height, rgba.into_raw()),
        frames: 1,
        ppi,
        flat_share,
        many_colours,
    })
}

/// One composited frame of a GIF, and how many frames it has, within the decoding budget.
fn gif_frame(
    bytes: &[u8],
    wanted: usize,
    pixels: u64,
) -> Result<(image::RgbaImage, usize), String> {
    use image::AnimationDecoder;
    let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("Cannot read this picture: {e}"))?;
    let mut chosen = None;
    let mut count = 0;
    for frame in decoder.into_frames() {
        let frame = frame.map_err(|e| format!("Cannot read this picture: {e}"))?;
        if count == wanted {
            chosen = Some(frame.into_buffer());
        }
        count += 1;
        if count > MAX_FRAMES || count as u64 * pixels > 4 * MAX_PIXELS {
            return Err(format!(
                "This animated picture has too many frames (more than {count}) to read. Save the frame you want as its own picture."
            ));
        }
    }
    let chosen = chosen.ok_or(format!(
        "This picture has {count} frame(s); frame {wanted} does not exist."
    ))?;
    Ok((chosen, count))
}

/// BMP header resolution (pixels per metre) when both axes agree.
fn bmp_ppi(bytes: &[u8]) -> Option<f64> {
    let field = |at: usize| {
        bytes
            .get(at..at + 4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let (x, y) = (field(38)?, field(42)?);
    (x == y && x > 0).then(|| f64::from(x) * 0.0254)
}

/// PNG `pHYs`: pixels per metre when its unit is the metre and both axes agree.
fn png_ppi(bytes: &[u8]) -> Option<f64> {
    let mut at = 8;
    while at + 12 <= bytes.len() {
        let length = u32::from_be_bytes(bytes[at..at + 4].try_into().ok()?) as usize;
        let kind = &bytes[at + 4..at + 8];
        let data = bytes.get(at + 8..at + 8 + length)?;
        if kind == b"pHYs" && length == 9 {
            let x = u32::from_be_bytes(data[0..4].try_into().ok()?);
            let y = u32::from_be_bytes(data[4..8].try_into().ok()?);
            return (data[8] == 1 && x == y && x > 0).then(|| f64::from(x) * 0.0254);
        }
        if kind == b"IDAT" {
            return None;
        }
        at += 12 + length;
    }
    None
}

/// JPEG JFIF density (`APP0`): dots per inch or per centimetre when both axes agree.
fn jfif_ppi(bytes: &[u8]) -> Option<f64> {
    let app0 = bytes.windows(5).position(|w| w == b"JFIF\0")?;
    let d = bytes.get(app0 + 7..app0 + 12)?;
    let (x, y) = (
        u16::from_be_bytes([d[1], d[2]]),
        u16::from_be_bytes([d[3], d[4]]),
    );
    if x != y || x == 0 {
        return None;
    }
    match d[0] {
        1 => Some(f64::from(x)),
        2 => Some(f64::from(x) * 2.54),
        _ => None,
    }
}

/// Scan every opaque pixel so even rare colours count, stopping once the limit is exceeded.
/// Alpha is not a colour: transparent RGB values and alpha variations do not add shades, and
/// the colours of partly transparent edge pixels are left out too: a Photoshop document's
/// merged image is stored on white, and taking the white back out of its soft edges gives
/// black art a spread of greys it doesn't have (2026-10-23).
fn has_many_colours(rgba: &image::RgbaImage) -> bool {
    let mut colours = std::collections::HashSet::new();
    for p in rgba.pixels().filter(|p| p[3] >= OPAQUE) {
        colours.insert([p[0], p[1], p[2]]);
        if colours.len() > MAX_FLAT_COLOURS {
            return true;
        }
    }
    false
}

fn flat_share(rgba: &image::RgbaImage) -> f64 {
    let mut counts = std::collections::HashMap::<u16, usize>::new();
    let step = ((rgba.width() as usize * rgba.height() as usize) / 250_000).max(1);
    let mut opaque = 0;
    for p in rgba.pixels().step_by(step) {
        if p[3] < 128 {
            continue;
        }
        opaque += 1;
        let key = (u16::from(p[0] >> 4) << 8) | (u16::from(p[1] >> 4) << 4) | u16::from(p[2] >> 4);
        *counts.entry(key).or_default() += 1;
    }
    let mut top: Vec<usize> = counts.into_values().collect();
    top.sort_unstable_by(|a, b| b.cmp(a));
    top.iter().take(4).sum::<usize>() as f64 / opaque.max(1) as f64
}

/// Automatic picture size, preserving proportions and leaving room for the default border.
/// Explicit recipe sizes bypass these starting-size limits.
pub(crate) fn initial_width_mm(picture: &Picture) -> (f64, &'static str) {
    use cutcraft_job::{
        MachineProfile, Mat,
        project::{PLACEMENT_CLEARANCE_MM, WEEDING_MARGIN_MM},
    };

    let width = f64::from(picture.raster.width);
    let height = f64::from(picture.raster.height);
    let ppi = picture.ppi.unwrap_or(96.0);
    let longest_mm = width.max(height) / ppi * 25.4;
    let machine = MachineProfile::cameo_1();
    let available = machine
        .cutting_width_mm
        .min(machine.mat_length_mm(Mat::Mat12x12))
        - 2.0 * (WEEDING_MARGIN_MM + PLACEMENT_CLEARANCE_MM);
    let chosen = longest_mm.clamp(25.4, available);
    let reason = if longest_mm < 25.4 {
        "enlarged to 1 inch; resize in Cut Preview"
    } else if longest_mm > available {
        "fitted to the mat; resize in Cut Preview"
    } else if picture.ppi.is_some() {
        "from the picture's resolution; resize in Cut Preview"
    } else {
        "using 96 DPI; resize in Cut Preview"
    };
    (chosen * width / width.max(height), reason)
}

/// The inspection of a picture file: its pixels and what it needs before preparation.
pub(crate) fn inspection(path: &Path, bytes: &[u8], picture: &Picture) -> Inspection {
    let format = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .map_or("png", |e| match e.as_str() {
            "gif" => "gif",
            "bmp" => "bmp",
            "jpg" | "jpeg" => "jpeg",
            _ => "png",
        });
    Inspection {
        schema_version: 1,
        engine_revision: ENGINE_REVISION,
        source_sha256: format!("{:x}", Sha256::digest(bytes)),
        format: format.into(),
        stage: "import_inspection",
        cut_ready: false,
        restored_editing_data: false,
        preview_only: false,
        declared_svg_size_mm: None,
        pages: vec![],
        objects: vec![Object {
            source_id: "source/0".into(),
            kind: "image",
            excluded: None,
        }],
        fonts: vec![],
        diagnostics: picture_diagnostics(picture, None),
    }
}

/// A photo or product mockup rather than flat artwork: Make cut-ready asks before tracing it.
pub(crate) fn looks_like_photo(picture: &Picture) -> bool {
    picture.many_colours || picture.flat_share < FLAT_SHARE
}

/// Warnings about the picture's resolution at its chosen size.
pub(crate) fn picture_diagnostics(picture: &Picture, width_mm: Option<f64>) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if let Some(width_mm) = width_mm {
        let per_mm = f64::from(picture.raster.width) / width_mm;
        if per_mm < LOW_PIXELS_PER_MM {
            out.push(Diagnostic::info(
                "picture.low_resolution",
                format!(
                    "At this size the picture has only {:.0} pixels per inch; its edges may look stepped. A larger original traces more smoothly.",
                    per_mm * 25.4
                ),
                vec!["source/0".into()],
            ));
        }
    }
    out
}

fn trace_params(settings: &TraceSettings) -> TraceParams {
    let base = match settings.colours {
        TraceColours::One => vectorcraft_trace::preset("black and white logo"),
        TraceColours::Several => vectorcraft_trace::preset("flat logo"),
    }
    .unwrap_or_default();
    TraceParams {
        threshold: settings.threshold,
        paths: f64::from(settings.detail.min(100)),
        corners: f64::from(100 - settings.smoothness.min(100)),
        ignore_white: settings.ignore_white,
        fills: true,
        strokes: false,
        ..base
    }
}

/// Is this light art (white, say, for a dark shirt) on a transparent background? Then a
/// one-colour trace follows what is visible rather than what is dark.
pub(crate) fn light_art_on_transparency(picture: &Picture, settings: &TraceSettings) -> bool {
    if settings.colours != TraceColours::One {
        return false;
    }
    let (mut transparent, mut visible, mut light) = (0u64, 0u64, 0u64);
    for p in picture.raster.rgba.chunks_exact(4) {
        if p[3] < 128 {
            transparent += 1;
            continue;
        }
        visible += 1;
        let luminance =
            (299 * u32::from(p[0]) + 587 * u32::from(p[1]) + 114 * u32::from(p[2])) / 1000;
        if luminance >= u32::from(settings.threshold) {
            light += 1;
        }
    }
    transparent > 0 && visible > 0 && light as f64 >= LIGHT_ART_SHARE * visible as f64
}

/// The visible pixels black on white, for tracing light art; and their average colour.
fn visible_pixels(raster: &Raster) -> (Raster, [u8; 3]) {
    let mut sum = [0u64; 3];
    let mut count = 0u64;
    let mut rgba = Vec::with_capacity(raster.rgba.len());
    for p in raster.rgba.chunks_exact(4) {
        if p[3] >= 128 {
            for (s, c) in sum.iter_mut().zip(p) {
                *s += u64::from(*c);
            }
            count += 1;
            rgba.extend_from_slice(&[0, 0, 0, 255]);
        } else {
            rgba.extend_from_slice(&[255, 255, 255, 255]);
        }
    }
    let average = sum.map(|s| (s / count.max(1)) as u8);
    (Raster::new(raster.width, raster.height, rgba), average)
}

/// Trace the picture at `width_mm` into a one-page document of filled paths (one per traced
/// shape, bottom first), with the picture's own inspection. The page is the picture's area.
pub(crate) fn traced(
    picture: &Picture,
    width_mm: f64,
    settings: &TraceSettings,
    mut inspection: Inspection,
) -> Result<Imported, String> {
    // Light art on transparency is traced from what is visible, in the art's own colour.
    let light =
        light_art_on_transparency(picture, settings).then(|| visible_pixels(&picture.raster));
    let raster = light.as_ref().map_or(&picture.raster, |(r, _)| r);
    let result = vectorcraft_trace::trace_within(raster, &trace_params(settings), MAX_ANCHORS)
        .map_err(|e| format!("This picture could not be traced: {e}"))?;
    let scale = width_mm / MM_PER_PT / f64::from(picture.raster.width);
    let (width_pt, height_pt) = (
        f64::from(picture.raster.width) * scale,
        f64::from(picture.raster.height) * scale,
    );
    let mut doc = Document::new(width_pt, height_pt);
    let mut children = Vec::with_capacity(result.paths.len());
    for traced in result.paths {
        let [r, g, b] = light.as_ref().map_or(traced.color, |(_, colour)| *colour);
        let fill = FillLayer::new(Paint::solid(Color::rgb8(r, g, b)));
        let appearance = Appearance {
            items: vec![AppearanceItem::Fill(fill)],
            ..Default::default()
        };
        let id = doc.alloc_id();
        let path = traced.path.transformed(Affine::scale(scale));
        children.push(Arc::new(Node::path(id, path, appearance)));
    }
    let layer = doc.alloc_id();
    let mut layer_node = Node::layer(layer, "Picture", vectorcraft_doc::LayerColor::Preset(0));
    if let Some(slot) = layer_node.children_mut() {
        *slot = children;
    }
    inspection.objects.extend(
        (0..layer_node.children().map_or(0, Vec::len)).map(|i| Object {
            source_id: format!("source/0/{i}"),
            kind: "path",
            excluded: None,
        }),
    );
    doc.layers.push(Arc::new(layer_node));
    inspection.pages = vec![Page {
        id: "page/0".into(),
        width_mm: width_pt * MM_PER_PT,
        height_mm: height_pt * MM_PER_PT,
    }];
    inspection
        .diagnostics
        .extend(picture_diagnostics(picture, Some(width_mm)));
    Ok(Imported { inspection, doc })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An authored PNG: `f(x, y)` gives each pixel.
    pub(crate) fn png(width: u32, height: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let image = image::RgbaImage::from_fn(width, height, |x, y| image::Rgba(f(x, y)));
        let mut out = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in bytes {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// The PNG with a `pHYs` chunk stating `ppm` pixels per metre, inserted after `IHDR`.
    pub(crate) fn with_phys(png: &[u8], ppm: u32) -> Vec<u8> {
        let mut chunk = b"pHYs".to_vec();
        chunk.extend(ppm.to_be_bytes());
        chunk.extend(ppm.to_be_bytes());
        chunk.push(1);
        let mut out = png[..33].to_vec(); // Signature (8) and IHDR (25).
        out.extend(9u32.to_be_bytes());
        out.extend(&chunk);
        out.extend(crc32(&chunk).to_be_bytes());
        out.extend(&png[33..]);
        out
    }

    /// An authored Photoshop document (Adobe's public file format specification): 8-bit
    /// grayscale, raw merged image from `f(x, y)`, with a ResolutionInfo resource at `ppi`.
    fn psd(width: u32, height: u32, ppi: u32, f: impl Fn(u32, u32) -> u8) -> Vec<u8> {
        let mut out = b"8BPS".to_vec();
        out.extend(1u16.to_be_bytes());
        out.extend([0; 6]);
        out.extend(1u16.to_be_bytes()); // One channel.
        out.extend(height.to_be_bytes());
        out.extend(width.to_be_bytes());
        out.extend(8u16.to_be_bytes());
        out.extend(1u16.to_be_bytes()); // Grayscale.
        out.extend(0u32.to_be_bytes()); // No colour mode data.
        let mut resolution = b"8BIM".to_vec();
        resolution.extend(1005u16.to_be_bytes());
        resolution.extend([0, 0]); // Empty name, padded to even.
        resolution.extend(16u32.to_be_bytes());
        for _ in 0..2 {
            resolution.extend((ppi << 16).to_be_bytes());
            resolution.extend(1u16.to_be_bytes());
            resolution.extend(1u16.to_be_bytes());
        }
        out.extend((resolution.len() as u32).to_be_bytes());
        out.extend(resolution);
        out.extend(0u32.to_be_bytes()); // No layers.
        out.extend(0u16.to_be_bytes()); // Raw image data.
        for y in 0..height {
            for x in 0..width {
                out.push(f(x, y));
            }
        }
        out
    }

    #[test]
    fn light_art_on_transparency_is_traced_from_what_is_visible() {
        let recipe = cutcraft_job::Recipe::new();
        // A white square on a transparent background: art for a dark shirt.
        let white = png(40, 40, |x, y| {
            if (10..30).contains(&x) && (10..30).contains(&y) {
                [255, 255, 255, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        let prepared = crate::prepare_bytes(Path::new("white.png"), &white, &recipe).unwrap();
        assert_eq!(prepared.pieces.len(), 1, "{:?}", prepared.diagnostics);
        assert_eq!(prepared.pieces[0].colour.hex, "#ffffff");
        let traced = prepared
            .changes
            .iter()
            .find(|c| c.code == "picture.traced")
            .unwrap();
        assert!(traced.message.contains("light art"), "{}", traced.message);
        // Dark art with white details on transparency is traced as before: the white is a hole.
        let dark = png(40, 40, |x, y| match (x, y) {
            (15..25, 15..25) => [255, 255, 255, 255],
            (5..35, 5..35) => [0, 0, 0, 255],
            _ => [0, 0, 0, 0],
        });
        let prepared = crate::prepare_bytes(Path::new("dark.png"), &dark, &recipe).unwrap();
        assert_eq!(prepared.pieces.len(), 1);
        assert_eq!(prepared.pieces[0].holes.len(), 1);
        assert_eq!(prepared.pieces[0].colour.hex, "#000000");
    }

    #[test]
    fn a_photoshop_document_opens_as_its_merged_picture() {
        let square = |x: u32, y: u32| {
            if (10..30).contains(&x) && (10..30).contains(&y) {
                0
            } else {
                255
            }
        };
        let bytes = psd(40, 40, 150, square);
        let picture = decode(&bytes, None).unwrap();
        assert_eq!((picture.raster.width, picture.raster.height), (40, 40));
        assert_eq!(picture.ppi, Some(150.0));
        assert!(is_picture(Path::new("design.PSD")) && is_picture(Path::new("big.psb")));
        let prepared = crate::prepare_bytes(
            Path::new("design.psd"),
            &bytes,
            &cutcraft_job::Recipe::new(),
        )
        .unwrap();
        assert_eq!(prepared.pieces.len(), 1, "{:?}", prepared.diagnostics);
        // The Artwork view and previews draw it too.
        let view = crate::SourceView::new(Path::new("design.psd"), &bytes, &prepared).unwrap();
        let mm = prepared.page.as_ref().unwrap().width_mm / 40.0;
        let pixels = view
            .render([0.0, 0.0, 40.0 * mm, 40.0 * mm], 40, 40)
            .unwrap();
        assert_eq!(&pixels[(20 * 40 + 20) * 4..][..3], [0, 0, 0]);
        assert_eq!(&pixels[(2 * 40 + 2) * 4..][..3], [255, 255, 255]);
        assert!(crate::source_png(Path::new("design.psd"), &bytes, 64).is_ok());
        assert!(decode(&bytes, Some(1)).is_err());
        assert!(decode(&bytes[..30], None).is_err());
    }

    #[test]
    fn stated_resolution_is_read_from_png_and_jfif_headers() {
        let plain = png(4, 4, |_, _| [0, 0, 0, 255]);
        assert_eq!(png_ppi(&plain), None);
        let stated = with_phys(&plain, 11811); // 300 ppi.
        assert!((png_ppi(&stated).unwrap() - 299.9994).abs() < 1e-3);
        assert!(
            decode(&stated, None).is_ok(),
            "the inserted chunk keeps the PNG valid"
        );
        let jfif = [
            0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F', 0, 1, 2, 1, 0, 150, 0, 150,
        ];
        assert_eq!(jfif_ppi(&jfif), Some(150.0));
        let mut cm = jfif;
        cm[13] = 2;
        assert!((jfif_ppi(&cm).unwrap() - 381.0).abs() < 1e-9);
    }

    /// An authored animated GIF: one solid square per frame, moving right.
    pub(crate) fn animated_gif(frames: u32) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut out);
            for f in 0..frames {
                let image = image::RgbaImage::from_fn(60, 20, |x, y| {
                    if (5..15).contains(&y) && x / 20 == f {
                        image::Rgba([0, 0, 0, 255])
                    } else {
                        image::Rgba([255; 4])
                    }
                });
                encoder.encode_frame(image::Frame::new(image)).unwrap();
            }
        }
        out
    }

    /// An authored BMP stating `ppm` pixels per metre.
    pub(crate) fn bmp(ppm: i32) -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbaImage::from_fn(40, 20, |x, _| {
            if (10..30).contains(&x) {
                image::Rgba([0, 0, 0, 255])
            } else {
                image::Rgba([255; 4])
            }
        })
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Bmp)
        .unwrap();
        out[38..42].copy_from_slice(&ppm.to_le_bytes());
        out[42..46].copy_from_slice(&ppm.to_le_bytes());
        out
    }

    #[test]
    fn gif_frames_and_bmp_resolution_are_read() {
        let gif = animated_gif(3);
        let first = decode(&gif, None).unwrap();
        assert_eq!(first.frames, 3);
        assert!(first.ppi.is_none());
        let third = decode(&gif, Some(2)).unwrap();
        // The first frame's square spans x 0..20, the third's 40..60.
        let px = |p: &Picture, x: u32| p.raster.rgba[((10 * 60 + x) * 4) as usize];
        assert_eq!((px(&first, 5), px(&third, 5), px(&third, 45)), (0, 255, 0));
        assert!(
            decode(&gif, Some(3))
                .err()
                .unwrap()
                .contains("frame 3 does not exist")
        );

        let picture = decode(&bmp(11811), None).unwrap();
        assert!((picture.ppi.unwrap() - 299.9994).abs() < 1e-3);
        assert_eq!(picture.frames, 1);
        assert!(decode(&bmp(11811), Some(1)).is_err());
    }

    #[test]
    fn exact_colour_limit_catches_shades_that_the_flatness_buckets_merge() {
        for count in [20, 21] {
            let picture = decode(
                &png(100, 100, |x, y| {
                    // Twenty or twenty-one rare shades in otherwise uniform artwork.
                    if y == 99 && x < count {
                        [144 + (x % 8) as u8, 192 + (x / 8) as u8, 96, 255]
                    } else {
                        // The main colour is also the first rare shade.
                        [144, 192, 96, 255]
                    }
                }),
                None,
            )
            .unwrap();
            assert!(picture.flat_share > 0.99);
            assert_eq!(looks_like_photo(&picture), count > 20);
        }
    }

    #[test]
    fn transparent_colours_and_alpha_variations_do_not_trigger_the_colour_limit() {
        let picture = decode(
            &png(64, 64, |x, y| {
                if y < 32 {
                    [x as u8, y as u8, 200, 0]
                } else {
                    [144, 192, 96, 128 + x as u8]
                }
            }),
            None,
        )
        .unwrap();
        assert!(!looks_like_photo(&picture));
    }

    #[test]
    fn flat_art_and_photos_are_told_apart() {
        let flat = decode(
            &png(
                64,
                64,
                |x, _| if x < 32 { [0, 0, 0, 255] } else { [255; 4] },
            ),
            None,
        )
        .unwrap();
        assert!(flat.flat_share > 0.99);
        let noisy = decode(
            &png(64, 64, |x, y| {
                let v = (x * 37 + y * 101) % 251;
                [v as u8, (v * 7 % 256) as u8, (v * 13 % 256) as u8, 255]
            }),
            None,
        )
        .unwrap();
        assert!(looks_like_photo(&noisy));
        assert!(!looks_like_photo(&flat));
    }
}
