//! The outline an imported file stored for a boolean group.
//!
//! A `.fig` carries the computed result of every boolean group (`fillGeometry`). Drawing that
//! outline instead of recomputing the union reproduces the incumbent's curves exactly. The outline
//! is only valid for the operands it was computed from: `key` fingerprints them (set by the
//! renderer the first time the group is drawn, and saved with the file), so editing an operand,
//! the operation or the corner rounding makes the group fall back to the computed result.

use figcraft_geom::BezPath;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "Repr", into = "Repr")]
pub struct StoredOutline {
    /// The outline in the group's own coordinates at `width` x `height`.
    pub path: Arc<BezPath>,
    pub width: f64,
    pub height: f64,
    pub even_odd: bool,
    /// Fingerprint of what the outline was computed from (unset until first drawn).
    pub key: OnceLock<u64>,
}

#[derive(Serialize, Deserialize)]
struct Repr {
    d: String,
    w: f64,
    h: f64,
    #[serde(default)]
    eo: bool,
    #[serde(default)]
    key: Option<u64>,
}

impl From<Repr> for StoredOutline {
    fn from(r: Repr) -> Self {
        let path = BezPath::from_svg(&r.d).unwrap_or_default();
        Self { path: Arc::new(path), width: r.w, height: r.h, even_odd: r.eo, key: r.key.map(OnceLock::from).unwrap_or_default() }
    }
}

impl From<StoredOutline> for Repr {
    fn from(s: StoredOutline) -> Self {
        Self { d: s.path.to_svg(), w: s.width, h: s.height, eo: s.even_odd, key: s.key.get().copied() }
    }
}

impl StoredOutline {
    pub fn new(path: BezPath, width: f64, height: f64, even_odd: bool) -> Self {
        Self { path: Arc::new(path), width, height, even_odd, key: OnceLock::new() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_with_its_key() {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((4.0, 0.0));
        p.line_to((4.0, 2.0));
        p.close_path();
        let o = StoredOutline::new(p, 4.0, 2.0, true);
        let json = serde_json::to_string(&o).unwrap();
        assert_eq!(serde_json::from_str::<StoredOutline>(&json).unwrap(), o);
        o.key.set(77).unwrap();
        let back: StoredOutline = serde_json::from_str(&serde_json::to_string(&o).unwrap()).unwrap();
        assert_eq!(back.key.get(), Some(&77));
        assert_eq!(back, o);
    }
}
