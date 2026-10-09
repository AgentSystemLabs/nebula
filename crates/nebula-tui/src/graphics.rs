//! Inline visual previews for image files and Mermaid diagrams.
//!
//! The renderer deliberately has a no-network surface: it reads local files
//! the caller already selected, and Mermaid diagrams are rendered only by a
//! local `mmdc` executable when one is on PATH. Pixels are painted with the
//! Unicode half-block fallback, so the same output survives native terminals,
//! ttyd/xterm.js, and `nebula ssh`.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use image::{DynamicImage, ImageBuffer, Rgba};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::theme::{Theme, BLACK_BACKGROUND};

pub const INLINE_GRAPHICS_MODES: &[&str] =
    &["auto", "kitty", "iterm", "sixel", "halfblocks", "off"];
pub const DEFAULT_INLINE_GRAPHICS: &str = "auto";

const MAX_MARKDOWN_DIAGRAM_ROWS: u16 = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphicsMode {
    Off,
    Halfblocks,
}

pub fn graphics_mode(value: &str) -> GraphicsMode {
    match value.trim().to_ascii_lowercase().as_str() {
        "off" => GraphicsMode::Off,
        _ => GraphicsMode::Halfblocks,
    }
}

pub fn mode_label(value: &str) -> &'static str {
    INLINE_GRAPHICS_MODES
        .iter()
        .copied()
        .find(|mode| mode.eq_ignore_ascii_case(value.trim()))
        .unwrap_or(DEFAULT_INLINE_GRAPHICS)
}

pub fn is_image_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg")
    )
}

pub fn is_mermaid_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("mmd"))
}

#[derive(Debug, Clone)]
pub struct RasterImage {
    width: u32,
    height: u32,
    pixels: Vec<[u8; 4]>,
}

impl RasterImage {
    fn from_dynamic(image: DynamicImage) -> Self {
        let rgba = image.to_rgba8();
        let (width, height) = rgba.dimensions();
        Self {
            width,
            height,
            pixels: rgba.pixels().map(|p| p.0).collect(),
        }
    }

    fn from_png_bytes(bytes: &[u8]) -> Result<Self, String> {
        image::load_from_memory(bytes)
            .map(Self::from_dynamic)
            .map_err(|err| format!("couldn't decode rendered diagram: {err}"))
    }
}

#[derive(Debug, Clone)]
pub struct VisualRender {
    width: u16,
    height: u16,
    mode: GraphicsMode,
    lines: Vec<Line<'static>>,
}

#[derive(Debug, Clone)]
pub struct VisualPreview {
    pub title: String,
    pub raster: Option<RasterImage>,
    pub message: Option<String>,
    pub source: Option<String>,
    pub rendered: Option<VisualRender>,
}

impl VisualPreview {
    pub fn image(path: &Path) -> Self {
        match load_image(path) {
            Ok(raster) => Self {
                title: image_title(path),
                raster: Some(raster),
                message: None,
                source: None,
                rendered: None,
            },
            Err(message) => Self {
                title: image_title(path),
                raster: None,
                message: Some(message),
                source: None,
                rendered: None,
            },
        }
    }

    pub fn mermaid(source: String) -> Self {
        let rendered = render_mermaid(&source);
        let (raster, message) = match rendered {
            MermaidOutput::Image(raster) => (Some(raster), None),
            MermaidOutput::Hint(hint) => (None, Some(hint)),
        };
        Self {
            title: "Mermaid diagram".into(),
            raster,
            message,
            source: Some(source),
            rendered: None,
        }
    }

    pub fn render(&mut self, area: Rect, mode: GraphicsMode, th: Theme) -> Vec<Line<'static>> {
        if area.width == 0 || area.height == 0 {
            return Vec::new();
        }
        if mode == GraphicsMode::Off {
            return self.fallback_lines(
                "inline graphics are off (set `inline_graphics` to `auto` or `halfblocks`)",
            );
        }
        let Some(raster) = &self.raster else {
            return self.fallback_lines(
                self.message
                    .as_deref()
                    .unwrap_or("couldn't render this visual preview"),
            );
        };
        if let Some(cached) = &self.rendered {
            if cached.width == area.width && cached.height == area.height && cached.mode == mode {
                return cached.lines.clone();
            }
        }
        let lines = raster_to_lines(raster, area.width, area.height, th);
        self.rendered = Some(VisualRender {
            width: area.width,
            height: area.height,
            mode,
            lines: lines.clone(),
        });
        lines
    }

    fn fallback_lines(&self, hint: &str) -> Vec<Line<'static>> {
        let mut lines = vec![Line::from(Span::styled(
            hint.to_string(),
            Style::default().fg(Color::Yellow),
        ))];
        if let Some(source) = &self.source {
            lines.push(Line::default());
            lines.extend(source.lines().map(|line| Line::from(line.to_string())));
        }
        lines
    }
}

#[derive(Debug, Clone)]
pub struct MermaidDiagram {
    pub source: String,
    pub visual: VisualPreview,
}

pub fn extract_mermaid_blocks(markdown: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut lines = markdown.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let fence = if trimmed.starts_with("```") {
            '`'
        } else if trimmed.starts_with("~~~") {
            '~'
        } else {
            continue;
        };
        let fence_len = trimmed.chars().take_while(|c| *c == fence).count();
        let info = trimmed[fence_len..].trim();
        if !info
            .split_whitespace()
            .next()
            .is_some_and(|lang| lang.eq_ignore_ascii_case("mermaid"))
        {
            continue;
        }
        let mut source = String::new();
        for body in lines.by_ref() {
            let close = body.trim_start();
            if close.chars().take_while(|c| *c == fence).count() >= fence_len {
                break;
            }
            if !source.is_empty() {
                source.push('\n');
            }
            source.push_str(body);
        }
        blocks.push(source);
    }
    blocks
}

pub fn render_mermaid_blocks(markdown: &str) -> Vec<MermaidDiagram> {
    extract_mermaid_blocks(markdown)
        .into_iter()
        .map(|source| MermaidDiagram {
            visual: VisualPreview::mermaid(source.clone()),
            source,
        })
        .collect()
}

pub fn markdown_diagram_lines(
    source: &str,
    diagrams: &mut [MermaidDiagram],
    width: u16,
    mode: GraphicsMode,
    th: Theme,
) -> Option<Vec<Line<'static>>> {
    let diagram = diagrams
        .iter_mut()
        .find(|diagram| diagram.source == source)?;
    let rows = diagram_rows(diagram.visual.raster.as_ref(), width);
    let area = Rect::new(0, 0, width, rows);
    Some(diagram.visual.render(area, mode, th))
}

fn diagram_rows(raster: Option<&RasterImage>, width: u16) -> u16 {
    let Some(raster) = raster else {
        return 8;
    };
    if raster.width == 0 || raster.height == 0 || width == 0 {
        return 1;
    }
    let pixel_h = (raster.height as f32 * width as f32 / raster.width as f32).ceil() as u16;
    pixel_h.div_ceil(2).clamp(1, MAX_MARKDOWN_DIAGRAM_ROWS)
}

pub fn load_image(path: &Path) -> Result<RasterImage, String> {
    if path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
    {
        return load_svg(path);
    }
    image::ImageReader::open(path)
        .map_err(|err| format!("couldn't read image: {err}"))?
        .with_guessed_format()
        .map_err(|err| format!("couldn't identify image format: {err}"))?
        .decode()
        .map(RasterImage::from_dynamic)
        .map_err(|err| format!("couldn't decode image: {err}"))
}

fn load_svg(path: &Path) -> Result<RasterImage, String> {
    let try_rsvg = std::process::Command::new("rsvg-convert")
        .arg("-f")
        .arg("png")
        .arg(path)
        .output();
    if let Ok(out) = try_rsvg {
        if out.status.success() {
            return RasterImage::from_png_bytes(&out.stdout);
        }
    }
    let try_magick = std::process::Command::new("magick")
        .arg(path)
        .arg("png:-")
        .output();
    if let Ok(out) = try_magick {
        if out.status.success() {
            return RasterImage::from_png_bytes(&out.stdout);
        }
    }
    Err("SVG preview needs `rsvg-convert` or ImageMagick's `magick` on PATH".into())
}

fn image_title(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    format!("Image preview — {name}")
}

fn raster_to_lines(
    raster: &RasterImage,
    max_cols: u16,
    max_rows: u16,
    _th: Theme,
) -> Vec<Line<'static>> {
    if raster.width == 0 || raster.height == 0 || max_cols == 0 || max_rows == 0 {
        return Vec::new();
    }
    let max_px_w = max_cols as u32;
    let max_px_h = (max_rows as u32).saturating_mul(2).max(1);
    let scale = (max_px_w as f32 / raster.width as f32)
        .min(max_px_h as f32 / raster.height as f32)
        .max(0.01);
    let target_w = ((raster.width as f32 * scale).round() as u32).clamp(1, max_px_w);
    let target_h = ((raster.height as f32 * scale).round() as u32).clamp(1, max_px_h);
    let mut src = Vec::with_capacity((raster.width * raster.height * 4) as usize);
    for px in &raster.pixels {
        src.extend_from_slice(px);
    }
    let Some(buf) = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(raster.width, raster.height, src)
    else {
        return vec![Line::from("couldn't prepare image pixels")];
    };
    let resized = image::imageops::resize(
        &buf,
        target_w,
        target_h,
        image::imageops::FilterType::Triangle,
    );
    let left_pad = ((max_cols as u32).saturating_sub(target_w) / 2) as usize;
    let rows = target_h.div_ceil(2).min(max_rows as u32);
    (0..rows)
        .map(|row| {
            let mut spans = Vec::with_capacity(target_w as usize + 1);
            if left_pad > 0 {
                spans.push(Span::raw(" ".repeat(left_pad)));
            }
            let top_y = row * 2;
            let bottom_y = top_y + 1;
            for x in 0..target_w {
                let top = resized.get_pixel(x, top_y).0;
                let bottom = if bottom_y < target_h {
                    resized.get_pixel(x, bottom_y).0
                } else {
                    [0, 0, 0, 0]
                };
                spans.push(pixel_pair_span(top, bottom));
            }
            Line::from(spans)
        })
        .collect()
}

fn pixel_pair_span(top: [u8; 4], bottom: [u8; 4]) -> Span<'static> {
    let top = rgba_color(top);
    let bottom = rgba_color(bottom);
    match (top, bottom) {
        (None, None) => Span::raw(" "),
        (Some(fg), Some(bg)) => Span::styled("▀", Style::default().fg(fg).bg(bg)),
        (Some(fg), None) => Span::styled("▀", Style::default().fg(fg)),
        (None, Some(fg)) => Span::styled("▄", Style::default().fg(fg)),
    }
}

fn rgba_color(px: [u8; 4]) -> Option<Color> {
    let [r, g, b, a] = px;
    if a == 0 {
        return None;
    }
    let (br, bg, bb) = match BLACK_BACKGROUND {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0, 0, 0),
    };
    let a = a as u16;
    let blend =
        |fg: u8, bg: u8| -> u8 { (((fg as u16 * a) + (bg as u16 * (255 - a))) / 255) as u8 };
    Some(Color::Rgb(blend(r, br), blend(g, bg), blend(b, bb)))
}

#[derive(Debug, Clone)]
enum MermaidOutput {
    Image(RasterImage),
    Hint(String),
}

fn mermaid_cache() -> &'static Mutex<HashMap<u64, MermaidOutput>> {
    static CACHE: OnceLock<Mutex<HashMap<u64, MermaidOutput>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn render_mermaid(source: &str) -> MermaidOutput {
    let key = content_hash(source);
    if let Some(cached) = mermaid_cache().lock().unwrap().get(&key).cloned() {
        return cached;
    }
    let rendered = render_mermaid_uncached(source);
    mermaid_cache()
        .lock()
        .unwrap()
        .insert(key, rendered.clone());
    rendered
}

fn render_mermaid_uncached(source: &str) -> MermaidOutput {
    let paths = MermaidTempPaths::new();
    if let Err(err) = std::fs::write(&paths.input, source) {
        return MermaidOutput::Hint(format!("couldn't prepare Mermaid render: {err}"));
    }
    let output = std::process::Command::new("mmdc")
        .arg("-i")
        .arg(&paths.input)
        .arg("-o")
        .arg(&paths.output)
        .arg("-b")
        .arg("transparent")
        .output();
    let output = match output {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            paths.cleanup();
            return MermaidOutput::Hint(
                "install Mermaid CLI (`npm install -g @mermaid-js/mermaid-cli`) to render diagrams"
                    .into(),
            );
        }
        Err(err) => {
            paths.cleanup();
            return MermaidOutput::Hint(format!("couldn't run Mermaid CLI: {err}"));
        }
    };
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let one_line = err
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("Mermaid CLI failed");
        paths.cleanup();
        return MermaidOutput::Hint(format!("Mermaid CLI failed: {one_line}"));
    }
    let bytes = match std::fs::read(&paths.output) {
        Ok(bytes) => bytes,
        Err(err) => {
            paths.cleanup();
            return MermaidOutput::Hint(format!("couldn't read Mermaid output: {err}"));
        }
    };
    paths.cleanup();
    RasterImage::from_png_bytes(&bytes).map_or_else(MermaidOutput::Hint, MermaidOutput::Image)
}

#[derive(Debug)]
struct MermaidTempPaths {
    input: PathBuf,
    output: PathBuf,
}

impl MermaidTempPaths {
    fn new() -> Self {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let stem = format!("nebula-mermaid-{}-{millis}", std::process::id());
        let dir = std::env::temp_dir();
        Self {
            input: dir.join(format!("{stem}.mmd")),
            output: dir.join(format!("{stem}.png")),
        }
    }

    fn cleanup(&self) {
        let _ = std::fs::remove_file(&self.input);
        let _ = std::fs::remove_file(&self.output);
    }
}

fn content_hash(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_and_mermaid_paths_are_detected_by_extension() {
        assert!(is_image_path(Path::new("mock.PNG")));
        assert!(is_image_path(Path::new("photo.jpeg")));
        assert!(is_image_path(Path::new("anim.gif")));
        assert!(is_image_path(Path::new("vector.svg")));
        assert!(is_mermaid_path(Path::new("diagram.MMD")));
        assert!(!is_image_path(Path::new("README.md")));
        assert!(!is_mermaid_path(Path::new("README.md")));
    }

    #[test]
    fn mermaid_blocks_are_extracted_from_markdown_fences() {
        let text = "before\n```mermaid\nflowchart LR\n  A-->B\n```\n\
                    ```rust\nfn main() {}\n```\n~~~ Mermaid\nsequenceDiagram\nA->>B: hi\n~~~\n";
        assert_eq!(
            extract_mermaid_blocks(text),
            vec![
                "flowchart LR\n  A-->B".to_string(),
                "sequenceDiagram\nA->>B: hi".to_string()
            ]
        );
    }

    #[test]
    fn graphics_mode_off_disables_and_everything_else_uses_halfblocks() {
        assert_eq!(graphics_mode("off"), GraphicsMode::Off);
        assert_eq!(graphics_mode("auto"), GraphicsMode::Halfblocks);
        assert_eq!(graphics_mode("kitty"), GraphicsMode::Halfblocks);
        assert_eq!(mode_label("bogus"), DEFAULT_INLINE_GRAPHICS);
    }
}
