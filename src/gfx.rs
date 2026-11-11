//! Software compositing on premultiplied 0xAARRGGBB pixels.

use crate::font;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Rect { x, y, w, h }
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y && px < self.right() && py < self.bottom()
    }
    pub fn intersect(&self, o: &Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        Rect::new(x, y, (r - x).max(0), (b - y).max(0))
    }
    pub fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        let r = self.right().max(o.right());
        let b = self.bottom().max(o.bottom());
        Rect::new(x, y, r - x, b - y)
    }
    pub fn inflate(&self, d: i32) -> Rect {
        Rect::new(self.x - d, self.y - d, self.w + 2 * d, self.h + 2 * d)
    }
}

/// A premultiplied ARGB image. `spans[y]` is the opaque run of row y, so
/// blitting can memcpy the middle and only blend the antialiased edges.
#[derive(Clone, Default)]
pub struct Image {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u32>,
    spans: Vec<(u16, u16)>,
}

impl Image {
    pub fn new(w: i32, h: i32) -> Self {
        let w = w.max(0);
        let h = h.max(0);
        Image {
            w,
            h,
            px: vec![0; (w * h) as usize],
            spans: Vec::new(),
        }
    }

    pub fn from_pixels(w: i32, h: i32, px: Vec<u32>) -> Self {
        let mut img = Image {
            w,
            h,
            px,
            spans: Vec::new(),
        };
        img.compute_spans();
        img
    }

    /// From tiny-skia's premultiplied RGBA bytes.
    pub fn from_rgba_premul(w: i32, h: i32, data: &[u8]) -> Self {
        let px = data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| (c[3] as u32) << 24 | (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32)
            .collect();
        Self::from_pixels(w, h, px)
    }

    pub fn compute_spans(&mut self) {
        let w = self.w as usize;
        self.spans = (0..self.h as usize)
            .map(|y| {
                let row = &self.px[y * w..(y + 1) * w];
                match row.iter().position(|p| p >> 24 == 255) {
                    None => (0, 0),
                    Some(l) => {
                        let r = row[l..]
                            .iter()
                            .position(|p| p >> 24 != 255)
                            .map_or(w, |n| l + n);
                        (l as u16, r as u16)
                    }
                }
            })
            .collect();
    }

    pub fn rect(&self) -> Rect {
        Rect::new(0, 0, self.w, self.h)
    }

    /// Opaque fill of a rectangle.
    pub fn fill(&mut self, r: Rect, color: u32) {
        let r = r.intersect(&self.rect());
        for y in r.y..r.bottom() {
            let o = (y * self.w) as usize;
            self.px[o + r.x as usize..o + r.right() as usize].fill(color);
        }
    }

    /// Copy `src` region `sr` to (dx, dy), no blending, clipped to `clip`.
    pub fn copy_from(&mut self, src: &Image, dx: i32, dy: i32, clip: Rect) {
        let clip = clip
            .intersect(&self.rect())
            .intersect(&Rect::new(dx, dy, src.w, src.h));
        for y in clip.y..clip.bottom() {
            let so = ((y - dy) * src.w + clip.x - dx) as usize;
            let d = (y * self.w + clip.x) as usize;
            self.px[d..d + clip.w as usize].copy_from_slice(&src.px[so..so + clip.w as usize]);
        }
    }

    /// Alpha-blend `src` at (dx, dy) clipped to `clip`. `dim` darkens the
    /// source like KCard's highlight (0 = none, 255 = black).
    pub fn blit(&mut self, src: &Image, dx: i32, dy: i32, clip: Rect, dim: u8) {
        let c = clip
            .intersect(&self.rect())
            .intersect(&Rect::new(dx, dy, src.w, src.h));
        if c.is_empty() {
            return;
        }
        let keep = 255 - dim as u32;
        for y in c.y..c.bottom() {
            let sy = y - dy;
            let srow = &src.px[(sy * src.w) as usize..((sy + 1) * src.w) as usize];
            let drow = &mut self.px[(y * self.w) as usize..((y + 1) * self.w) as usize];
            let (sx0, sx1) = (c.x - dx, c.right() - dx);
            if dim == 0 {
                let (l, r) = src
                    .spans
                    .get(sy as usize)
                    .map_or((0, 0), |&(l, r)| (l as i32, r as i32));
                let ol = l.clamp(sx0, sx1);
                let or = r.clamp(ol, sx1);
                for sx in sx0..ol {
                    let d = &mut drow[(sx + dx) as usize];
                    *d = over(*d, srow[sx as usize]);
                }
                drow[(ol + dx) as usize..(or + dx) as usize]
                    .copy_from_slice(&srow[ol as usize..or as usize]);
                for sx in or..sx1 {
                    let d = &mut drow[(sx + dx) as usize];
                    *d = over(*d, srow[sx as usize]);
                }
            } else {
                for sx in sx0..sx1 {
                    let d = &mut drow[(sx + dx) as usize];
                    *d = over(*d, scale_rgb(srow[sx as usize], keep));
                }
            }
        }
    }

    /// Blit horizontally squashed to `dw` columns (card flip), centred in
    /// the source width. Each column averages the source columns it covers,
    /// so the artwork doesn't shimmer as the width changes.
    pub fn blit_squashed(&mut self, src: &Image, dx: i32, dy: i32, dw: i32, clip: Rect, dim: u8) {
        if dw <= 0 {
            return;
        }
        if dw >= src.w {
            return self.blit(src, dx, dy, clip, dim);
        }
        let x0 = dx + (src.w - dw) / 2;
        let c = clip
            .intersect(&self.rect())
            .intersect(&Rect::new(x0, dy, dw, src.h));
        let keep = 255 - dim as u32;
        for y in c.y..c.bottom() {
            let sy = y - dy;
            let srow = &src.px[(sy * src.w) as usize..((sy + 1) * src.w) as usize];
            for x in c.x..c.right() {
                let s0 = ((x - x0) as i64 * src.w as i64 / dw as i64) as usize;
                let s1 = (((x - x0 + 1) as i64 * src.w as i64 / dw as i64) as usize).max(s0 + 1);
                let n = (s1 - s0) as u32;
                let mut sum = [0u32; 4];
                for p in &srow[s0..s1] {
                    for (k, ch) in sum.iter_mut().enumerate() {
                        *ch += p >> (8 * k) & 0xff;
                    }
                }
                let avg = sum
                    .iter()
                    .enumerate()
                    .fold(0, |a, (k, ch)| a | (ch / n) << (8 * k));
                let s = scale_rgb(avg, keep);
                let d = &mut self.px[(y * self.w + x) as usize];
                *d = over(*d, s);
            }
        }
    }

    /// Nearest-neighbour stretch of the whole image (used for quick
    /// interim backgrounds while a resize settles).
    pub fn scaled(&self, w: i32, h: i32) -> Image {
        let mut out = Image::new(w, h);
        if self.w == 0 || self.h == 0 {
            return out;
        }
        let xs: Vec<usize> = (0..w)
            .map(|x| (x as i64 * self.w as i64 / w as i64) as usize)
            .collect();
        for y in 0..h {
            let sy = (y as i64 * self.h as i64 / h as i64) as usize;
            let srow = &self.px[sy * self.w as usize..];
            let drow = &mut out.px[(y * w) as usize..((y + 1) * w) as usize];
            for (d, &sx) in drow.iter_mut().zip(&xs) {
                *d = srow[sx];
            }
        }
        out.compute_spans();
        out
    }

    /// Draw `s` with the top of the font cell at `y`.
    pub fn text(&mut self, s: &str, x: i32, y: i32, scale: i32, color: u32, clip: Rect) {
        let clip = clip.intersect(&self.rect());
        let mut cx = x;
        for ch in s.chars() {
            let (adv, rows) = glyph(ch);
            for (row, bits) in rows.iter().enumerate() {
                for col in 0..16 {
                    if bits >> col & 1 != 0 {
                        let r = Rect::new(
                            cx + (col - 1) * scale,
                            y + row as i32 * scale,
                            scale,
                            scale,
                        )
                        .intersect(&clip);
                        self.fill(r, color);
                    }
                }
            }
            cx += *adv as i32 * scale;
        }
    }

    /// Bold by double strike, one pixel wider per glyph.
    pub fn text_bold(&mut self, s: &str, x: i32, y: i32, scale: i32, color: u32, clip: Rect) {
        let clip = clip.intersect(&self.rect());
        let mut cx = x;
        let mut buf = [0u8; 4];
        for ch in s.chars() {
            let one = ch.encode_utf8(&mut buf);
            self.text(one, cx, y, scale, color, clip);
            self.text(one, cx + scale, y, scale, color, clip);
            cx += (text_advance(ch) + 1) * scale;
        }
    }

    /// 3D bevel in the Windows 95 style.
    pub fn bevel(&mut self, r: Rect, s: i32, raised: bool) {
        let (hi, mid, lo, dark) = if raised {
            (0xffffffff, 0xffdfdfdf, 0xff808080, 0xff000000)
        } else {
            (0xff000000, 0xff808080, 0xffdfdfdf, 0xffffffff)
        };
        // outer
        self.fill(Rect::new(r.x, r.y, r.w, s), mid);
        self.fill(Rect::new(r.x, r.y, s, r.h), mid);
        self.fill(Rect::new(r.x, r.bottom() - s, r.w, s), dark);
        self.fill(Rect::new(r.right() - s, r.y, s, r.h), dark);
        // inner
        let i = r.inflate(-s);
        self.fill(Rect::new(i.x, i.y, i.w - s, s), hi);
        self.fill(Rect::new(i.x, i.y, s, i.h - s), hi);
        self.fill(Rect::new(i.x, i.bottom() - s, i.w, s), lo);
        self.fill(Rect::new(i.right() - s, i.y, s, i.h), lo);
    }
}

fn glyph(ch: char) -> &'static (u8, [u16; 13]) {
    let fallback = &font::GLYPHS[b'?' as usize - font::FIRST];
    match font::GLYPHS.get((ch as usize).wrapping_sub(font::FIRST)) {
        Some(g) if g.0 != 0 => g,
        _ => fallback,
    }
}

pub fn text_advance(ch: char) -> i32 {
    glyph(ch).0 as i32
}

pub fn text_width(s: &str, scale: i32) -> i32 {
    s.chars().map(text_advance).sum::<i32>() * scale
}

pub fn text_width_bold(s: &str, scale: i32) -> i32 {
    s.chars().map(|c| text_advance(c) + 1).sum::<i32>() * scale
}

#[inline(always)]
fn scale_rgb(p: u32, k: u32) -> u32 {
    if k == 255 {
        return p;
    }
    let rb = (((p & 0x00ff00ff) * k) >> 8) & 0x00ff00ff;
    let g = (((p & 0x0000ff00) * k) >> 8) & 0x0000ff00;
    (p & 0xff000000) | rb | g
}

/// Premultiplied source-over.
#[inline(always)]
pub fn over(d: u32, s: u32) -> u32 {
    let a = s >> 24;
    if a == 255 {
        return s;
    }
    if a == 0 {
        return d;
    }
    let ia = 255 - a;
    let mut rb = (d & 0x00ff00ff) * ia + 0x00800080;
    rb = ((rb + ((rb >> 8) & 0x00ff00ff)) >> 8) & 0x00ff00ff;
    let mut ag = ((d >> 8) & 0x00ff00ff) * ia + 0x00800080;
    ag = (ag + ((ag >> 8) & 0x00ff00ff)) & 0xff00ff00;
    s.wrapping_add(rb | ag)
}
