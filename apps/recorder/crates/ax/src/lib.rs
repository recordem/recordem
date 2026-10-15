// An app's Accessibility tree as plain data. The snapshot itself only exists on macOS; the model is portable, so
// snapshots saved as JSON (fixtures) are read and scraped anywhere, tests included.
use serde::{Deserialize, Serialize};

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
pub use mac::{app_version, is_trusted, pid_of, snapshot, windows};

/// Subrole of a window only the window server knows of (another Space, minimised): a title, no contents.
pub const ELSEWHERE: &str = "RecordemElsewhere";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    /// This rect in `outer`'s coordinates, as fractions of its size (0..1): what a video crop needs.
    pub fn relative_to(&self, outer: &Rect) -> Option<Rect> {
        (outer.w > 0.0 && outer.h > 0.0).then(|| Rect {
            x: (self.x - outer.x) / outer.w,
            y: (self.y - outer.y) / outer.h,
            w: self.w / outer.w,
            h: self.h / outer.h,
        })
    }
}

/// One element. Only the attributes scrapers use; empty ones are left out of the JSON.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AxNode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subrole: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dom_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dom_classes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<Rect>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<AxNode>,
    /// Some children were not read: the snapshot hit its limits here. A scrape of a cut tree can miss people.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

impl AxNode {
    /// A text attribute by the names selectors use.
    pub fn attr(&self, name: &str) -> Option<&str> {
        match name {
            "role" => self.role.as_deref(),
            "subrole" => self.subrole.as_deref(),
            "role_description" => self.role_description.as_deref(),
            "title" => self.title.as_deref(),
            "description" => self.description.as_deref(),
            "value" => self.value.as_deref(),
            "identifier" => self.identifier.as_deref(),
            "dom_id" => self.dom_id.as_deref(),
            _ => None,
        }
    }

    /// Every element below this one, depth first, with its depth relative to this one (children are 1).
    pub fn descendants(&self) -> impl Iterator<Item = (usize, &AxNode)> {
        let mut stack: Vec<(usize, &AxNode)> = self.children.iter().rev().map(|c| (1, c)).collect();
        std::iter::from_fn(move || {
            let (depth, node) = stack.pop()?;
            stack.extend(node.children.iter().rev().map(|c| (depth + 1, c)));
            Some((depth, node))
        })
    }

    /// All the text this element and its descendants carry, in tree order (what a person would read in it).
    pub fn texts(&self) -> Vec<&str> {
        std::iter::once(self)
            .chain(self.descendants().map(|(_, n)| n))
            .flat_map(|n| [n.title.as_deref(), n.description.as_deref(), n.value.as_deref()])
            .flatten()
            .filter(|s| !s.trim().is_empty())
            .collect()
    }

    pub fn count(&self) -> usize {
        1 + self.descendants().count()
    }

    pub fn is_truncated(&self) -> bool {
        self.truncated || self.descendants().any(|(_, n)| n.truncated)
    }
}

/// Which parts of an app a snapshot reads. Both tests see an element before its children are read, with no
/// children yet: they can only look at its own attributes.
pub struct Scope<'a> {
    /// Windows to read in full (by title, frame…).
    pub window: &'a dyn Fn(&AxNode) -> bool,
    /// Elements whose contents are skipped in those windows: a browser's toolbar and menus.
    pub prune: &'a dyn Fn(&AxNode) -> bool,
    /// The same, in the other windows: there a browser's tab strip is still read (the call may be a background tab)
    /// but not the page.
    pub prune_elsewhere: &'a dyn Fn(&AxNode) -> bool,
}

impl Scope<'_> {
    pub const ALL: Scope<'static> = Scope { window: &|_| true, prune: &|_| false, prune_elsewhere: &|_| false };
}

/// How much of a tree a snapshot reads: a call window in Chrome is thousands of elements, and every attribute read
/// is a round trip to the app.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_depth: usize,
    pub max_nodes: usize,
    /// Per AX call, seconds: a busy app answers slowly and must not stall the caller.
    pub timeout: f32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_depth: 80, max_nodes: 20_000, timeout: 0.5 }
    }
}
