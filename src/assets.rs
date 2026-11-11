//! KDE card decks (carddecks/*/index.desktop) and KPat background themes
//! (kpat/themes/*.desktop), loaded and rendered with resvg.

use crate::config::{data_dirs, get, read_ini};
use crate::gfx::Image;
use resvg::{tiny_skia, usvg};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const BUILTIN: &str = "builtin";

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub path: PathBuf, // empty for built-ins
}

fn dedup_sort(mut v: Vec<Entry>) -> Vec<Entry> {
    // First occurrence wins (user data dir is searched first).
    let mut seen = std::collections::HashSet::new();
    v.retain(|e| seen.insert(e.id.clone()));
    v.sort_by_key(|a| a.name.to_lowercase());
    v
}

pub fn find_decks() -> Vec<Entry> {
    let mut v = vec![Entry {
        id: BUILTIN.into(),
        name: "Windows 95 (built-in)".into(),
        path: PathBuf::new(),
    }];
    for dir in data_dirs() {
        let Ok(rd) = std::fs::read_dir(dir.join("carddecks")) else {
            continue;
        };
        for d in rd.flatten() {
            let index = d.path().join("index.desktop");
            let Some(ini) = read_ini(&index) else {
                continue;
            };
            let g = "KDE Backdeck";
            let Some(svg) = get(&ini, g, "SVG").filter(|s| !s.is_empty()) else {
                continue;
            };
            // Decks declare what they contain; we need a normal 52 card deck.
            if let Some(f) = get(&ini, g, "Features")
                && !f.split([',', ';']).any(|x| x.trim() == "AngloAmerican")
            {
                continue;
            }
            let Some(path) = existing(&d.path().join(svg)) else {
                continue;
            };
            let id = d.file_name().to_string_lossy().into_owned();
            let name = get(&ini, g, "Name").unwrap_or(&id).to_string();
            v.push(Entry { id, name, path });
        }
    }
    dedup_sort(v)
}

pub fn find_themes() -> Vec<Entry> {
    let mut v = vec![Entry {
        id: BUILTIN.into(),
        name: "Clean Green (built-in)".into(),
        path: PathBuf::new(),
    }];
    for dir in data_dirs() {
        let Ok(rd) = std::fs::read_dir(dir.join("kpat/themes")) else {
            continue;
        };
        for d in rd.flatten() {
            let p = d.path();
            if p.extension().is_none_or(|e| e != "desktop") {
                continue;
            }
            let Some(ini) = read_ini(&p) else { continue };
            let g = "KGameTheme";
            let Some(file) = get(&ini, g, "FileName") else {
                continue;
            };
            let Some(path) = existing(&p.with_file_name(file)) else {
                continue;
            };
            let id = p.file_stem().unwrap().to_string_lossy().into_owned();
            let name = get(&ini, g, "Name").unwrap_or(&id).to_string();
            v.push(Entry { id, name, path });
        }
    }
    dedup_sort(v)
}

/// Installed themes are compressed (.svgz) even when the .desktop file
/// names the .svg, and vice versa; accept either.
fn existing(p: &Path) -> Option<PathBuf> {
    if p.is_file() {
        return Some(p.to_path_buf());
    }
    let s = p.to_string_lossy();
    let alt = if let Some(b) = s.strip_suffix(".svgz") {
        format!("{b}.svg")
    } else {
        format!("{s}z")
    };
    let alt = PathBuf::from(alt);
    alt.is_file().then_some(alt)
}

/// A parsed SVG document that can render individual elements by id.
pub struct Svg {
    tree: usvg::Tree,
}

const USE_PREFIX: &str = "solitaire-rs-use-";

impl Svg {
    /// Parse `data` (SVG or SVGZ). Some decks keep their cards inside
    /// <defs>; QSvgRenderer renders those by id but they aren't part of
    /// resvg's render tree, so any of `wanted` that is missing gets a
    /// <use> reference appended and the document is parsed again.
    pub fn load(data: &[u8], resources: Option<&Path>, wanted: &[String]) -> Result<Svg, String> {
        let text;
        let data = if data.starts_with(&[0x1f, 0x8b]) {
            text = usvg::decompress_svgz(data).map_err(|e| e.to_string())?;
            &text[..]
        } else {
            data
        };
        let opt = usvg::Options {
            resources_dir: resources.map(Path::to_path_buf),
            ..Default::default()
        };
        let src = String::from_utf8_lossy(data);
        let hidden: Vec<&String> = wanted.iter().filter(|id| in_defs(&src, id)).collect();
        let end = src.rfind("</svg");
        let tree = match end {
            Some(end) if !hidden.is_empty() => {
                let mut patched = String::with_capacity(src.len() + hidden.len() * 64);
                patched.push_str(&src[..end]);
                for id in hidden {
                    patched.push_str(&format!(r##"<use href="#{id}" id="{USE_PREFIX}{id}"/>"##));
                }
                patched.push_str(&src[end..]);
                usvg::Tree::from_data(patched.as_bytes(), &opt)
            }
            _ => usvg::Tree::from_data(data, &opt),
        };
        tree.map(|tree| Svg { tree }).map_err(|e| e.to_string())
    }

    pub fn load_file(path: &Path, wanted: &[String]) -> Result<Svg, String> {
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::load(&data, path.parent(), wanted).map_err(|e| format!("{}: {e}", path.display()))
    }

    fn node(&self, id: &str) -> Option<&usvg::Node> {
        self.tree
            .node_by_id(id)
            .or_else(|| self.tree.node_by_id(&format!("{USE_PREFIX}{id}")))
    }

    pub fn has(&self, id: &str) -> bool {
        self.node(id).is_some()
    }

    /// Size of an element as QSvgRenderer::boundsOnElement would report it.
    pub fn bounds(&self, id: &str) -> Option<(f32, f32)> {
        let n = self.node(id)?;
        let b = n.abs_stroke_bounding_box();
        (b.width() > 0.0 && b.height() > 0.0).then(|| (b.width(), b.height()))
    }

    /// Render element `id` stretched to exactly w x h pixels.
    pub fn render(&self, id: &str, w: i32, h: i32) -> Option<Image> {
        let node = self.node(id)?;
        let mut pix = tiny_skia::Pixmap::new(w.max(1) as u32, h.max(1) as u32)?;
        let sb = node.abs_stroke_bounding_box();
        let lb = node.abs_layer_bounding_box()?;
        // render_node() applies the node's own transform but not its
        // ancestors', and pre-translates by the layer bbox; undo/compose so
        // the stroke bbox maps exactly onto the target.
        let parent = match node {
            usvg::Node::Group(g) => g.abs_transform().pre_concat(g.transform().invert()?),
            _ => node.abs_transform(),
        };
        let t = tiny_skia::Transform::from_scale(w as f32 / sb.width(), h as f32 / sb.height())
            .pre_translate(-sb.x(), -sb.y())
            .pre_concat(parent)
            .pre_translate(lb.x(), lb.y());
        resvg::render_node(node, t, &mut pix.as_mut())?;
        Some(Image::from_rgba_premul(w, h, pix.data()))
    }
}

/// Is the element with this id defined inside a <defs> block?
fn in_defs(src: &str, id: &str) -> bool {
    let pos = src
        .find(&format!("id=\"{id}\""))
        .or_else(|| src.find(&format!("id='{id}'")));
    let Some(pos) = pos else { return false };
    let before = &src[..pos];
    match (before.rfind("<defs"), before.rfind("</defs")) {
        (Some(open), Some(close)) => open > close,
        (Some(_), None) => true,
        _ => false,
    }
}

pub fn element_name(card: u8) -> String {
    let rank = card % 13 + 1;
    let suit = ["club", "diamond", "heart", "spade"][(card / 13) as usize];
    let r = match rank {
        11 => "jack".to_string(),
        12 => "queen".to_string(),
        13 => "king".to_string(),
        n => n.to_string(),
    };
    format!("{r}_{suit}")
}

/// The 52 faces, then the back.
fn deck_ids() -> Vec<String> {
    (0..52)
        .map(element_name)
        .chain(std::iter::once("back".to_string()))
        .collect()
}

pub struct Deck {
    pub svg: Arc<Svg>,
    pub aspect: f32, // width / height
}

impl Deck {
    pub fn load(e: &Entry) -> Result<Deck, String> {
        let svg = if e.id == BUILTIN {
            Svg::load(crate::builtin::deck_svg().as_bytes(), None, &[])?
        } else {
            Svg::load_file(&e.path, &deck_ids())?
        };
        let (w, h) = svg.bounds("back").ok_or("deck has no 'back' element")?;
        if !(0..52).all(|c| svg.has(&element_name(c))) {
            return Err("deck is missing card elements".into());
        }
        Ok(Deck {
            svg: Arc::new(svg),
            aspect: w / h,
        })
    }

    /// Render all 52 faces plus the back (index 52) in parallel.
    pub fn render_all(&self, w: i32, h: i32) -> Vec<Image> {
        let ids = deck_ids();
        let threads = std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .clamp(1, 16);
        let mut out: Vec<Option<Image>> = vec![None; ids.len()];
        std::thread::scope(|s| {
            let chunks: Vec<_> = out
                .chunks_mut(ids.len().div_ceil(threads))
                .enumerate()
                .collect();
            let per = ids.len().div_ceil(threads);
            for (ci, chunk) in chunks {
                let ids = &ids;
                let svg = &self.svg;
                s.spawn(move || {
                    for (j, slot) in chunk.iter_mut().enumerate() {
                        *slot = svg.render(&ids[ci * per + j], w, h);
                    }
                });
            }
        });
        out.into_iter()
            .map(|i| i.unwrap_or_else(|| Image::new(w, h)))
            .collect()
    }
}

pub struct Theme {
    pub svg: Arc<Svg>,
}

impl Theme {
    pub fn load(e: &Entry) -> Result<Theme, String> {
        let wanted: Vec<String> = ["background", "pile", "pile_selected"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let svg = if e.id == BUILTIN {
            Svg::load(crate::builtin::THEME_SVG, None, &[])?
        } else {
            Svg::load_file(&e.path, &wanted)?
        };
        if !svg.has("background") {
            return Err("theme has no 'background' element".into());
        }
        Ok(Theme { svg: Arc::new(svg) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore]
    fn load_all_decks() {
        for e in find_decks() {
            let t = std::time::Instant::now();
            match Deck::load(&e) {
                Ok(d) => {
                    let t1 = t.elapsed();
                    let w = 120;
                    let imgs = d.render_all(w, (w as f32 / d.aspect) as i32);
                    let blank = imgs.iter().filter(|i| i.px.iter().all(|&p| p == 0)).count();
                    let b = d.svg.bounds("back").unwrap();
                    let f = d.svg.bounds("1_club").unwrap();
                    eprintln!("   back {b:?} ace {f:?}");
                    eprintln!(
                        "{:28} load {:>6.0?} render {:>6.0?} aspect {:.3} blank {blank}",
                        e.id,
                        t1,
                        t.elapsed() - t1,
                        d.aspect
                    );
                }
                Err(err) => eprintln!("{:28} ERROR {err}", e.id),
            }
        }
        for e in find_themes() {
            let t = std::time::Instant::now();
            let th = Theme::load(&e).unwrap();
            let t1 = t.elapsed();
            let bg = th.svg.render("background", 1920, 1080).unwrap();
            let p = th.svg.render("pile", 120, 170);
            eprintln!(
                "theme {:20} load {:>6.0?} bg1080p {:>6.0?} pile {}",
                e.id,
                t1,
                t.elapsed() - t1,
                p.is_some() && bg.w > 0
            );
        }
    }
}
