// Selectors: which elements of a tree a profile means, as JSON arrays:
//
//   ["eq", "role", "AXButton"]             attribute equals
//   ["contains", "description", "muted"]   attribute contains (case-insensitive)
//   ["matches", "description", "(?i)active speaker$"]
//   ["class", "participant-tile"]          one of the DOM classes (web apps)
//   ["has", "title"]                       attribute present and not blank
//   ["and", s1, s2, …]  ["or", s1, s2, …]  ["not", s]
//   ["one_of", "role_description", ["Video render", "Rendu vidéo", …]]   attribute equals one of (translations)
//   ["child", s]                           some direct child matches s
//   ["descendant", s]                      some element below matches s
//   ["count", s, min, max]                 between min and max elements below match s (structure, not names)
//   ["min_size", w, h]  ["max_size", w, h] the element's frame, in points
//
// Attributes: role, subrole, role_description, title, description, value, identifier, dom_id.
use recordem_ax::AxNode;
use regex::Regex;
use serde::{Deserialize, Deserializer, de::Error};
use serde_json::Value;

#[derive(Debug, Clone)]
pub enum Selector {
    Eq(String, String),
    OneOf(String, Vec<String>),
    Contains(String, String),
    Matches(String, Regex),
    Class(String),
    Has(String),
    And(Vec<Selector>),
    Or(Vec<Selector>),
    Not(Box<Selector>),
    Child(Box<Selector>),
    Descendant(Box<Selector>),
    Count(Box<Selector>, usize, usize),
    MinSize(f64, f64),
    MaxSize(f64, f64),
}

impl Selector {
    pub fn matches(&self, node: &AxNode) -> bool {
        match self {
            Selector::Eq(a, v) => node.attr(a) == Some(v.as_str()),
            Selector::OneOf(a, vs) => node.attr(a).is_some_and(|s| vs.iter().any(|v| v == s)),
            Selector::Contains(a, v) => node.attr(a).is_some_and(|s| s.to_lowercase().contains(&v.to_lowercase())),
            Selector::Matches(a, re) => node.attr(a).is_some_and(|s| re.is_match(s)),
            Selector::Class(c) => node.dom_classes.iter().any(|x| x == c),
            Selector::Has(a) => node.attr(a).is_some_and(|s| !s.trim().is_empty()),
            Selector::And(all) => all.iter().all(|s| s.matches(node)),
            Selector::Or(any) => any.iter().any(|s| s.matches(node)),
            Selector::Not(s) => !s.matches(node),
            Selector::Child(s) => node.children.iter().any(|c| s.matches(c)),
            Selector::Descendant(s) => node.descendants().any(|(_, n)| s.matches(n)),
            Selector::Count(s, min, max) => {
                let n = node.descendants().filter(|(_, n)| s.matches(n)).take(max + 1).count();
                (*min..=*max).contains(&n)
            }
            Selector::MinSize(w, h) => node.frame.is_some_and(|f| f.w >= *w && f.h >= *h),
            Selector::MaxSize(w, h) => node.frame.is_some_and(|f| f.w <= *w && f.h <= *h),
        }
    }

    /// Elements below `root` (not root itself) that match, down to `max_depth`, outermost first: once an element
    /// matches, its own descendants are not searched (a tile's inner groups are not tiles too).
    pub fn find_all<'a>(&self, root: &'a AxNode, max_depth: usize) -> Vec<&'a AxNode> {
        let mut out = Vec::new();
        self.collect(root, 1, max_depth, &mut out);
        out
    }

    fn collect<'a>(&self, node: &'a AxNode, depth: usize, max_depth: usize, out: &mut Vec<&'a AxNode>) {
        if depth > max_depth {
            return;
        }
        for c in &node.children {
            if self.matches(c) {
                out.push(c);
            } else {
                self.collect(c, depth + 1, max_depth, out);
            }
        }
    }

    /// `root` itself or the first element below it that matches.
    pub fn find_first<'a>(&self, root: &'a AxNode) -> Option<&'a AxNode> {
        std::iter::once(root).chain(root.descendants().map(|(_, n)| n)).find(|n| self.matches(n))
    }

    fn parse(v: &Value) -> Result<Selector, String> {
        let arr = v.as_array().ok_or_else(|| format!("a selector is an array, got {v}"))?;
        let op = arr.first().and_then(Value::as_str).ok_or("a selector starts with its operator")?;
        let s = |i: usize| -> Result<String, String> {
            arr.get(i).and_then(Value::as_str).map(str::to_string).ok_or_else(|| format!("{op}: argument {i} must be a string"))
        };
        let subs = || arr[1..].iter().map(Selector::parse).collect::<Result<Vec<_>, _>>();
        let num = |i: usize| -> Result<f64, String> {
            arr.get(i).and_then(Value::as_f64).ok_or_else(|| format!("{op}: argument {i} must be a number"))
        };
        let one = || arr.get(1).ok_or_else(|| format!("{op} takes a selector")).and_then(Selector::parse).map(Box::new);
        Ok(match op {
            "eq" => Selector::Eq(s(1)?, s(2)?),
            "one_of" => Selector::OneOf(
                s(1)?,
                arr.get(2)
                    .and_then(Value::as_array)
                    .ok_or("one_of: argument 2 must be a list of strings")?
                    .iter()
                    .map(|v| v.as_str().map(str::to_string).ok_or("one_of: list of strings"))
                    .collect::<Result<_, _>>()?,
            ),
            "contains" => Selector::Contains(s(1)?, s(2)?),
            "matches" => Selector::Matches(s(1)?, Regex::new(&s(2)?).map_err(|e| e.to_string())?),
            "class" => Selector::Class(s(1)?),
            "has" => Selector::Has(s(1)?),
            "and" => Selector::And(subs()?),
            "or" => Selector::Or(subs()?),
            "not" => Selector::Not(one()?),
            "child" => Selector::Child(one()?),
            "descendant" => Selector::Descendant(one()?),
            "count" => Selector::Count(one()?, num(2)? as usize, num(3)? as usize),
            "min_size" => Selector::MinSize(num(1)?, num(2)?),
            "max_size" => Selector::MaxSize(num(1)?, num(2)?),
            _ => return Err(format!("unknown selector operator {op:?}")),
        })
    }
}

impl<'de> Deserialize<'de> for Selector {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Selector::parse(&Value::deserialize(d)?).map_err(D::Error::custom)
    }
}
