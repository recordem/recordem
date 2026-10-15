// The call window as video, through ScreenCaptureKit's recording output (H.264 in an .mp4, macOS 15+): one stream
// of the whole window, cheap, independent of what else is on screen. Each person's video is cut from it afterwards
// with the tile frames the scrape recorded (`crop_plan`), which is plain data and works anywhere.
use recordem_ax::Rect;
use serde::{Deserialize, Serialize};

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
pub use mac::{WindowRecording, record_window};

/// Where one person's tile was, from `t_ms` on (until the next entry), as fractions of the window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TileAt {
    pub t_ms: u64,
    pub name: String,
    pub frame: Rect,
}

/// One person's crop: the time range and the rect in pixels of a `width`×`height` video. Tiles move when people
/// join, leave or the layout changes, so a person has several segments; a tile that moves less than `tolerance` (a
/// fraction of the window) keeps its segment.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Segment {
    pub name: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Each person's crops in a `width`×`height` recording of the window that ends at `end_ms`.
///
/// `times`: when the window was read (or known not to show the call): a person missing from a reading has no tile
/// from then on. Times when the window showed the call but couldn't be read (another Space) are left out, so tiles
/// keep their last place.
pub fn crop_plan(tiles: &[TileAt], times: &[u64], end_ms: u64, width: u32, height: u32, tolerance: f64) -> Vec<Segment> {
    let mut names: Vec<&str> = tiles.iter().map(|t| t.name.as_str()).collect();
    names.sort();
    names.dedup();
    let mut times: Vec<u64> = times.iter().copied().chain(tiles.iter().map(|t| t.t_ms)).collect();
    times.sort();
    times.dedup();
    let mut out = Vec::new();
    for name in names {
        let mut current: Option<(u64, Rect)> = None;
        let close = |out: &mut Vec<Segment>, start: u64, r: Rect, end: u64| {
            // Even pixel sizes: H.264 wants them.
            let px = |v: f64, max: u32| ((v.clamp(0.0, 1.0) * max as f64) as u32) & !1;
            let (x, y) = (px(r.x, width), px(r.y, height));
            let (w, h) = (px(r.w, width).min(width - x), px(r.h, height).min(height - y));
            if end > start && w >= 16 && h >= 16 {
                out.push(Segment { name: name.to_string(), start_ms: start, end_ms: end, x, y, w, h });
            }
        };
        for &t in &times {
            let at = tiles.iter().find(|x| x.t_ms == t && x.name == name).map(|x| x.frame);
            match (current, at) {
                (Some((_, r)), Some(n)) if moved(&r, &n) <= tolerance => {}
                (Some((s, r)), next) => {
                    close(&mut out, s, r, t);
                    current = next.map(|n| (t, n));
                }
                (None, next) => current = next.map(|n| (t, n)),
            }
        }
        if let Some((s, r)) = current {
            close(&mut out, s, r, end_ms);
        }
    }
    out
}

/// When the recorded window was not showing the call (`shown` samples: time, call on screen in that window), as
/// time ranges to black out: the window then shows whatever else the person opened, which is not theirs to share.
/// A sample stands until the next one; `margin_ms` around each change also covers the frames of the switch.
pub fn hidden_spans(shown: &[(u64, bool)], end_ms: u64, margin_ms: u64) -> Vec<(u64, u64)> {
    let mut out: Vec<(u64, u64)> = Vec::new();
    for (i, &(t, on)) in shown.iter().enumerate() {
        if on {
            continue;
        }
        let next = shown.get(i + 1).map_or(end_ms, |&(n, _)| n);
        let (s, e) = (t.saturating_sub(margin_ms), (next + margin_ms).min(end_ms));
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

fn moved(a: &Rect, b: &Rect) -> f64 {
    [(a.x - b.x), (a.y - b.y), (a.w - b.w), (a.h - b.h)].iter().map(|d| d.abs()).fold(0.0, f64::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t: u64, name: &str, x: f64) -> TileAt {
        TileAt { t_ms: t, name: name.into(), frame: Rect { x, y: 0.0, w: 0.5, h: 0.5 } }
    }

    #[test]
    fn unread_times_keep_the_tile_and_hidden_times_end_it() {
        let tiles = [at(0, "Ana", 0.0), at(500, "Ana", 0.0)];
        // 500..2000: another Space (not in times): Ana's tile holds. 2000: a background tab, read as nobody.
        let plan = crop_plan(&tiles, &[0, 500, 2000], 3000, 1000, 600, 0.02);
        assert_eq!(plan.iter().map(|s| (s.start_ms, s.end_ms)).collect::<Vec<_>>(), [(0, 2000)]);
    }

    #[test]
    fn hidden_spans_cover_the_switch() {
        let shown = [(0, true), (1000, false), (1500, false), (3000, true), (4000, false)];
        assert_eq!(hidden_spans(&shown, 5000, 200), [(800, 3200), (3800, 5000)]);
    }

    #[test]
    fn a_tile_that_moves_starts_a_new_segment() {
        let tiles = [at(0, "Ana", 0.0), at(0, "Bruno", 0.5), at(1000, "Ana", 0.001), at(2000, "Ana", 0.5)];
        let plan = crop_plan(&tiles, &[], 3000, 1000, 600, 0.02);
        let ana: Vec<_> = plan.iter().filter(|s| s.name == "Ana").map(|s| (s.start_ms, s.end_ms, s.x)).collect();
        assert_eq!(ana, [(0, 2000, 0), (2000, 3000, 500)]);
        // Bruno vanished at 1 s (left, or the layout hid him).
        let bruno: Vec<_> = plan.iter().filter(|s| s.name == "Bruno").map(|s| (s.start_ms, s.end_ms)).collect();
        assert_eq!(bruno, [(0, 1000)]);
    }
}
