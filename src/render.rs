//! Software rendering of the overlay into a tiny-skia pixmap.
//! Everything is laid out in logical pixels and scaled by the output scale at draw time.

use crate::config::{Config, Edge};
use crate::state::{Member, Status, Voice};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tiny_skia::{
    Color, FillRule, FilterQuality, IntSize, LineCap, Paint, Path, PathBuilder, Pattern, Pixmap, SpreadMode, Stroke,
    Transform,
};

const FONT: &[u8] = include_bytes!("../assets/OpenSans-Semibold.ttf");

const SPEAKING: [u8; 4] = [35, 165, 90, 255];
const MUTED: [u8; 4] = [242, 63, 67, 255];
const BADGE_BG: [u8; 4] = [30, 31, 34, 255];
const ACCENT: [u8; 4] = [88, 101, 242, 255];
const PILL: [u8; 4] = [17, 18, 20, 200];
const PILL_SPEAKING: [u8; 4] = [20, 22, 24, 235];
const TEXT: [u8; 4] = [215, 215, 220, 255];
const TEXT_SPEAKING: [u8; 4] = [255, 255, 255, 255];
const TEXT_DIM: [u8; 4] = [150, 150, 158, 255];
const LEFT_DOT: [u8; 4] = [128, 132, 142, 255];
/// Notice pill: dot diameter and the space after it.
const DOT: f32 = 7.0;
const DOT_GAP: f32 = 6.0;
const GAP: f32 = 6.0;

const NAME_MAX: f32 = 170.0;
const STATUS_MAX: f32 = 260.0;
const STATUS_FONT: f32 = 12.0;

pub struct Painter {
    font: fontdue::Font,
    glyphs: HashMap<(char, u32), (fontdue::Metrics, Vec<u8>)>,
    pub avatars: Avatars,
}

/// One thing in the list.
enum Item {
    Status(String),
    /// "Name joined" (true) or "Name left" (false).
    Notice(String, bool),
    Person(Member),
    /// "+N" for people beyond the max-entries limit.
    More(usize),
}

/// What to show, in order.
fn items(voice: &Voice, cfg: &Config, editing: bool) -> Vec<Item> {
    let mut out = Vec::new();

    let status = if editing {
        Some("Drag to an edge · click to lock".to_string())
    } else {
        match &voice.status {
            Status::Authorizing => Some("Click Authorize in Discord".to_string()),
            Status::Error(e) => Some(e.clone()),
            Status::Connected if cfg.show_channel => voice.channel_name.clone(),
            _ => None,
        }
    };

    let people: Vec<Member> = if editing && voice.members.is_empty() {
        sample_members()
    } else {
        voice.members.iter().filter(|m| editing || !cfg.only_speaking || m.speaking).cloned().collect()
    };
    // A channel name with nobody to show under it is just noise.
    if let Some(s) = status {
        if editing || !people.is_empty() || !matches!(voice.status, Status::Connected) {
            out.push(Item::Status(s));
        }
    }

    if !editing {
        out.extend(voice.live_notices().map(|n| Item::Notice(n.name.clone(), n.joined)));
    }

    let max = if cfg.max_entries == 0 { usize::MAX } else { cfg.max_entries as usize };
    if people.len() <= max {
        out.extend(people.into_iter().map(Item::Person));
    } else {
        // Over the limit: keep whoever is talking, fill up with the rest, keep join order.
        let mut keep = vec![false; people.len()];
        let mut n = 0;
        for pass_speaking in [true, false] {
            for (i, m) in people.iter().enumerate() {
                if n < max && !keep[i] && m.speaking == pass_speaking {
                    keep[i] = true;
                    n += 1;
                }
            }
        }
        let hidden = people.len() - n;
        out.extend(people.into_iter().zip(keep).filter(|(_, k)| *k).map(|(m, _)| Item::Person(m)));
        out.push(Item::More(hidden));
    }
    out
}

impl Painter {
    pub fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let font = fontdue::Font::from_bytes(FONT, fontdue::FontSettings::default()).expect("bundled font");
        Self { font, glyphs: HashMap::new(), avatars: Avatars::new(wake) }
    }

    fn name_font(cfg: &Config) -> f32 {
        (cfg.avatar_size * 0.42).clamp(12.0, 20.0)
    }

    fn pill_h(font: f32) -> f32 {
        font * 1.45 + 4.0
    }

    /// Logical size of one item.
    fn item_size(&mut self, item: &Item, cfg: &Config) -> (f32, f32) {
        let nf = Self::name_font(cfg);
        match item {
            Item::Status(t) => (self.pill_w(t, STATUS_FONT, STATUS_MAX), Self::pill_h(STATUS_FONT)),
            Item::More(n) => (self.pill_w(&format!("+{n}"), nf, NAME_MAX), Self::pill_h(nf)),
            Item::Notice(name, joined) => {
                let text = self.notice_text(name, *joined, nf);
                (DOT + DOT_GAP + self.text_width(&text, nf) + 16.0, Self::pill_h(nf))
            }
            Item::Person(m) => {
                let name_w = if cfg.show_names { GAP + self.pill_w(&m.name, nf, NAME_MAX) } else { 0.0 };
                (cfg.avatar_size + name_w, cfg.avatar_size)
            }
        }
    }

    /// Surface size (logical pixels) needed to show the current state on `cfg.edge`.
    pub fn measure(&mut self, voice: &Voice, cfg: &Config, editing: bool) -> (u32, u32) {
        let items = items(voice, cfg, editing);
        if items.is_empty() {
            return (1, 1);
        }
        let sizes: Vec<(f32, f32)> = items.iter().map(|i| self.item_size(i, cfg)).collect();
        let n = sizes.len() as f32;
        let (w, h) = if cfg.edge.is_horizontal() {
            (
                sizes.iter().map(|s| s.0).sum::<f32>() + GAP * (n + 1.0),
                sizes.iter().map(|s| s.1).fold(0.0, f32::max) + 2.0 * GAP,
            )
        } else {
            (
                sizes.iter().map(|s| s.0).fold(0.0, f32::max) + 2.0 * GAP,
                sizes.iter().map(|s| s.1).sum::<f32>() + GAP * (n + 1.0),
            )
        };
        (w.ceil() as u32, h.ceil() as u32)
    }

    pub fn draw(&mut self, pm: &mut Pixmap, scale: f32, voice: &Voice, cfg: &Config, editing: bool) {
        pm.fill(Color::TRANSPARENT);
        let ts = Transform::from_scale(scale, scale);
        let w = pm.width() as f32 / scale;
        let h = pm.height() as f32 / scale;

        if editing {
            if let Some(p) = rounded_rect(1.0, 1.0, w - 2.0, h - 2.0, 8.0) {
                pm.fill_path(&p, &paint([10, 10, 14, 150]), FillRule::Winding, ts, None);
                pm.stroke_path(&p, &paint(ACCENT), &Stroke { width: 2.0, ..Default::default() }, ts, None);
            }
        }

        let mirrored = cfg.edge == Edge::Right;
        let nf = Self::name_font(cfg);
        let (mut x, mut y) = (GAP, GAP);
        for item in items(voice, cfg, editing) {
            let (iw, ih) = self.item_size(&item, cfg);
            let (ix, iy) = if cfg.edge.is_horizontal() {
                (x, (h - ih) / 2.0)
            } else if mirrored {
                (w - GAP - iw, y)
            } else {
                (GAP, y)
            };
            match &item {
                Item::Status(t) => self.pill(pm, scale, ix, iy, t, STATUS_FONT, STATUS_MAX, PILL, TEXT),
                Item::More(n) => self.pill(pm, scale, ix, iy, &format!("+{n}"), nf, NAME_MAX, PILL, TEXT),
                Item::Notice(name, joined) => self.notice(pm, scale, ix, iy, iw, name, *joined, nf),
                Item::Person(m) => {
                    let size = cfg.avatar_size;
                    let ax = if mirrored { ix + iw - size } else { ix };
                    self.avatar(pm, scale, m, ax, iy, size);
                    if cfg.show_names {
                        let (bg, fg) = if m.speaking { (PILL_SPEAKING, TEXT_SPEAKING) } else { (PILL, TEXT) };
                        let px = if mirrored { ix } else { ix + size + GAP };
                        let py = iy + (size - Self::pill_h(nf)) / 2.0;
                        self.pill(pm, scale, px, py, &m.name, nf, NAME_MAX, bg, fg);
                    }
                }
            }
            if cfg.edge.is_horizontal() {
                x += iw + GAP;
            } else {
                y += ih + GAP;
            }
        }
    }

    fn avatar(&mut self, pm: &mut Pixmap, scale: f32, m: &Member, x: f32, y: f32, size: f32) {
        let ts = Transform::from_scale(scale, scale);
        let r = size / 2.0;
        let (cx, cy) = (x + r, y + r);
        let Some(circle) = PathBuilder::from_circle(cx, cy, r) else { return };

        pm.fill_path(&circle, &paint([40, 40, 44, 230]), FillRule::Winding, ts, None);
        if let Some(img) = self.avatars.get(&m.avatar_url) {
            let opacity = if m.speaking { 1.0 } else { 0.8 };
            let shader_ts =
                Transform::from_scale(size / img.width() as f32, size / img.height() as f32).post_translate(x, y);
            let mut p = Paint::default();
            p.anti_alias = true;
            p.shader = Pattern::new(img.as_ref().as_ref(), SpreadMode::Pad, FilterQuality::Bilinear, opacity, shader_ts);
            pm.fill_path(&circle, &p, FillRule::Winding, ts, None);
        }
        if m.speaking {
            if let Some(ring) = PathBuilder::from_circle(cx, cy, r - 1.25) {
                pm.stroke_path(&ring, &paint(SPEAKING), &Stroke { width: 2.5, ..Default::default() }, ts, None);
            }
        }

        if m.muted || m.deafened {
            // Discord-style status badge: a dark disc on the bottom-right with a red
            // crossed-out mic (muted) or headphones (deafened, which implies muted).
            let br = (size * 0.31).max(8.0);
            let (bx, by) = (x + size - br * 0.7, y + size - br * 0.7);
            if let Some(dot) = PathBuilder::from_circle(bx, by, br) {
                pm.fill_path(&dot, &paint(BADGE_BG), FillRule::Winding, ts, None);
            }
            let icon = if m.deafened { Icon::Deafened } else { Icon::Muted };
            draw_icon(pm, ts, icon, bx, by, br * 1.5, MUTED);
        }
    }

    /// "Name joined" / "Name left", with only the name truncated.
    fn notice_text(&mut self, name: &str, joined: bool, font: f32) -> String {
        let verb = if joined { " joined" } else { " left" };
        let room = NAME_MAX - 16.0 - self.text_width(verb, font);
        format!("{}{verb}", self.truncate(name, font, room))
    }

    #[allow(clippy::too_many_arguments)]
    fn notice(&mut self, pm: &mut Pixmap, scale: f32, x: f32, y: f32, w: f32, name: &str, joined: bool, font: f32) {
        let ts = Transform::from_scale(scale, scale);
        let h = Self::pill_h(font);
        if let Some(p) = rounded_rect(x, y, w, h, 4.0) {
            pm.fill_path(&p, &paint(PILL_SPEAKING), FillRule::Winding, ts, None);
        }
        let (dot, fg) = if joined { (SPEAKING, TEXT_SPEAKING) } else { (LEFT_DOT, TEXT_DIM) };
        if let Some(c) = PathBuilder::from_circle(x + 8.0 + DOT / 2.0, y + h / 2.0, DOT / 2.0) {
            pm.fill_path(&c, &paint(dot), FillRule::Winding, ts, None);
        }
        let text = self.notice_text(name, joined, font);
        let m = self.font.horizontal_line_metrics(font);
        let (asc, desc) = m.map_or((font, 0.0), |m| (m.ascent, m.descent));
        let baseline = y + (h - (asc - desc)) / 2.0 + asc;
        self.text(pm, scale, x + 8.0 + DOT + DOT_GAP, baseline, &text, font, fg);
    }

    /// Width of a pill holding `text` (truncated to fit `max_w`).
    fn pill_w(&mut self, text: &str, font: f32, max_w: f32) -> f32 {
        let t = self.truncate(text, font, max_w - 16.0);
        self.text_width(&t, font) + 16.0
    }

    #[allow(clippy::too_many_arguments)]
    fn pill(&mut self, pm: &mut Pixmap, scale: f32, x: f32, y: f32, text: &str, font: f32, max_w: f32, bg: [u8; 4], fg: [u8; 4]) {
        let text = self.truncate(text, font, max_w - 16.0);
        let w = self.text_width(&text, font) + 16.0;
        let h = Self::pill_h(font);
        if let Some(p) = rounded_rect(x, y, w, h, 4.0) {
            pm.fill_path(&p, &paint(bg), FillRule::Winding, Transform::from_scale(scale, scale), None);
        }
        let m = self.font.horizontal_line_metrics(font);
        let (asc, desc) = m.map_or((font, 0.0), |m| (m.ascent, m.descent));
        // Center the ascent..descent box vertically in the pill.
        let baseline = y + (h - (asc - desc)) / 2.0 + asc;
        self.text(pm, scale, x + 8.0, baseline, &text, font, fg);
    }

    fn glyph(&mut self, c: char, px: f32) -> &(fontdue::Metrics, Vec<u8>) {
        let key = (c, (px * 4.0) as u32);
        let font = &self.font;
        self.glyphs.entry(key).or_insert_with(|| font.rasterize(c, px))
    }

    fn text_width(&mut self, text: &str, px: f32) -> f32 {
        text.chars().map(|c| self.glyph(c, px).0.advance_width).sum()
    }

    fn truncate(&mut self, text: &str, px: f32, max_w: f32) -> String {
        if self.text_width(text, px) <= max_w {
            return text.to_string();
        }
        let ell = self.text_width("…", px);
        let mut out = String::new();
        let mut w = 0.0;
        for c in text.chars() {
            let a = self.glyph(c, px).0.advance_width;
            if w + a + ell > max_w {
                break;
            }
            w += a;
            out.push(c);
        }
        out.push('…');
        out
    }

    /// Draws text with its baseline at logical (x, baseline).
    #[allow(clippy::too_many_arguments)]
    fn text(&mut self, pm: &mut Pixmap, scale: f32, x: f32, baseline: f32, text: &str, size: f32, color: [u8; 4]) {
        let px = size * scale;
        let mut pen = x * scale;
        let base = baseline * scale;
        for c in text.chars() {
            let (m, bitmap) = self.glyph(c, px).clone();
            let gx = (pen + m.xmin as f32).round() as i32;
            let gy = (base - m.height as f32 - m.ymin as f32).round() as i32;
            blend_mask(pm, gx, gy, m.width, m.height, &bitmap, color);
            pen += m.advance_width;
        }
    }
}

#[derive(Clone, Copy)]
enum Icon {
    Muted,
    Deafened,
}

/// Draws a status icon `s` logical pixels wide centered on (cx, cy). Paths use a 24-unit
/// box like Discord's own SVG icons.
fn draw_icon(pm: &mut Pixmap, ts: Transform, icon: Icon, cx: f32, cy: f32, s: f32, color: [u8; 4]) {
    let ts = ts.pre_translate(cx - s / 2.0, cy - s / 2.0).pre_scale(s / 24.0, s / 24.0);
    let stroke = |w: f32| Stroke { width: w, line_cap: LineCap::Round, ..Default::default() };
    let fill = paint(color);

    match icon {
        Icon::Muted => {
            if let Some(body) = rounded_rect(8.5, 2.0, 7.0, 12.0, 3.5) {
                pm.fill_path(&body, &fill, FillRule::Winding, ts, None);
            }
            let mut pb = PathBuilder::new();
            pb.move_to(5.0, 11.0);
            pb.cubic_to(5.0, 15.5, 8.5, 18.0, 12.0, 18.0);
            pb.cubic_to(15.5, 18.0, 19.0, 15.5, 19.0, 11.0);
            pb.move_to(12.0, 18.0);
            pb.line_to(12.0, 21.5);
            if let Some(p) = pb.finish() {
                pm.stroke_path(&p, &fill, &stroke(2.2), ts, None);
            }
        }
        Icon::Deafened => {
            let mut pb = PathBuilder::new();
            pb.move_to(4.0, 15.0);
            pb.line_to(4.0, 12.0);
            pb.cubic_to(4.0, 7.6, 7.6, 4.0, 12.0, 4.0);
            pb.cubic_to(16.4, 4.0, 20.0, 7.6, 20.0, 12.0);
            pb.line_to(20.0, 15.0);
            if let Some(p) = pb.finish() {
                pm.stroke_path(&p, &fill, &stroke(2.4), ts, None);
            }
            for x in [2.5, 15.5] {
                if let Some(cup) = rounded_rect(x, 12.5, 6.0, 9.0, 2.2) {
                    pm.fill_path(&cup, &fill, FillRule::Winding, ts, None);
                }
            }
        }
    }

    // The slash, with a cut-out in the badge colour so it reads at small sizes.
    let mut pb = PathBuilder::new();
    pb.move_to(3.5, 3.5);
    pb.line_to(20.5, 20.5);
    if let Some(p) = pb.finish() {
        pm.stroke_path(&p, &paint(BADGE_BG), &stroke(4.2), ts, None);
        pm.stroke_path(&p, &fill, &stroke(2.0), ts, None);
    }
}

fn paint(c: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c[0], c[1], c[2], c[3]);
    p.anti_alias = true;
    p
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

/// Source-over blend of a coverage mask in a solid color into a premultiplied RGBA pixmap.
fn blend_mask(pm: &mut Pixmap, x: i32, y: i32, w: usize, h: usize, mask: &[u8], c: [u8; 4]) {
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    for row in 0..h as i32 {
        let py = y + row;
        if py < 0 || py >= ph {
            continue;
        }
        for col in 0..w as i32 {
            let px = x + col;
            if px < 0 || px >= pw {
                continue;
            }
            let cov = mask[(row as usize) * w + col as usize] as u32;
            if cov == 0 {
                continue;
            }
            let a = cov * c[3] as u32 / 255;
            let i = ((py * pw + px) * 4) as usize;
            for k in 0..3 {
                let src = c[k] as u32 * a / 255;
                data[i + k] = (src + data[i + k] as u32 * (255 - a) / 255) as u8;
            }
            data[i + 3] = (a + data[i + 3] as u32 * (255 - a) / 255) as u8;
        }
    }
}

fn sample_members() -> Vec<Member> {
    ["Talking friend", "Muted friend", "Quiet friend"]
        .iter()
        .enumerate()
        .map(|(i, name)| Member {
            id: format!("sample{i}"),
            name: (*name).into(),
            avatar_url: format!("https://cdn.discordapp.com/embed/avatars/{i}.png"),
            muted: i == 1,
            deafened: false,
            speaking: i == 0,
        })
        .collect()
}

enum Slot {
    Loading,
    Ready(Arc<Pixmap>),
    Failed,
}

/// Avatar images, downloaded on background threads and cached by URL.
pub struct Avatars {
    slots: Arc<Mutex<HashMap<String, Slot>>>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Avatars {
    fn new(wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self { slots: Arc::new(Mutex::new(HashMap::new())), wake }
    }

    pub fn get(&self, url: &str) -> Option<Arc<Pixmap>> {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        match slots.get(url) {
            Some(Slot::Ready(p)) => return Some(p.clone()),
            Some(_) => return None,
            None => {}
        }
        slots.insert(url.to_string(), Slot::Loading);
        let (url, slots, wake) = (url.to_string(), self.slots.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let slot = match fetch(&url) {
                Ok(p) => Slot::Ready(Arc::new(p)),
                Err(e) => {
                    eprintln!("vc-overlay: avatar {url}: {e}");
                    Slot::Failed
                }
            };
            slots.lock().unwrap_or_else(|e| e.into_inner()).insert(url, slot);
            wake();
        });
        None
    }
}

fn fetch(url: &str) -> Result<Pixmap, String> {
    let bytes = ureq::get(url)
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    let img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?.to_rgba8();
    let (w, h) = img.dimensions();
    let mut data = img.into_raw();
    for px in data.chunks_exact_mut(4) {
        let a = px[3] as u32;
        for c in &mut px[..3] {
            *c = (*c as u32 * a / 255) as u8;
        }
    }
    let size = IntSize::from_wh(w, h).ok_or("empty image")?;
    Pixmap::from_vec(data, size).ok_or_else(|| "bad image size".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Notice;
    use std::time::{Duration, Instant};

    fn voice_with_notices() -> Voice {
        let now = Instant::now();
        let notice = |name: &str, joined, until| Notice { name: name.into(), joined, until };
        Voice {
            status: Status::Connected,
            notices: vec![
                notice("Night", true, now + Duration::from_secs(3)),
                notice("Prismatic", false, now + Duration::from_secs(3)),
                notice("Expired", true, now - Duration::from_secs(1)),
            ],
            ..Voice::default()
        }
    }

    #[test]
    fn shows_live_notices_only() {
        let notices: Vec<(String, bool)> = items(&voice_with_notices(), &Config::default(), false)
            .into_iter()
            .filter_map(|i| match i {
                Item::Notice(n, j) => Some((n, j)),
                _ => None,
            })
            .collect();
        assert_eq!(notices, vec![("Night".into(), true), ("Prismatic".into(), false)]);
    }

    #[test]
    fn no_notices_while_editing() {
        let shown = items(&voice_with_notices(), &Config::default(), true);
        assert!(!shown.iter().any(|i| matches!(i, Item::Notice(..))));
    }
}
