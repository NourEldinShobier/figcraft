//! Layer keys: an optional, user-assigned stable string ("icon/folder") that names a layer across
//! save, open, undo and `.fig` round trips (ids are renumbered by imports). It lives in the layer's
//! `plugin_data` under [`KEY_FIELD`], so `Node` does not grow and older builds read the file (they
//! see an ordinary plugin-data entry). Keys are unique per document; the commands enforce that.

use crate::{Document, Node, NodeId};

/// The plugin-data entry that holds a layer's key.
pub const KEY_FIELD: &str = "figcraft.key";

/// Longest key, in bytes.
pub const MAX_KEY: usize = 256;

impl Node {
    /// This layer's key, if it has one.
    pub fn key(&self) -> Option<&str> {
        self.plugin_data.get(KEY_FIELD).map(String::as_str).filter(|k| !k.is_empty())
    }

    /// Set (or with `None` clear) the key. Uniqueness is the caller's check.
    pub fn set_key(&mut self, key: Option<&str>) {
        match key.filter(|k| !k.is_empty()) {
            Some(k) => {
                self.plugin_data.insert(KEY_FIELD.into(), k.to_string());
            }
            None => {
                self.plugin_data.remove(KEY_FIELD);
            }
        }
    }
}

/// Why `key` can't be a key, or `None` when it can.
pub fn key_problem(key: &str) -> Option<String> {
    if key.is_empty() || key.len() > MAX_KEY || key.chars().any(char::is_control) || key != key.trim() {
        return Some(format!("a key is 1 to {MAX_KEY} characters without control characters or surrounding spaces, e.g. \"icon/folder\""));
    }
    None
}

impl Document {
    /// The layer with this key.
    pub fn node_by_key(&self, key: &str) -> Option<NodeId> {
        self.nodes().find(|n| n.key() == Some(key)).map(|n| n.id)
    }

    /// Every key in the document, sorted.
    pub fn keys(&self) -> Vec<(String, NodeId)> {
        let mut v: Vec<_> = self.nodes().filter_map(|n| n.key().map(|k| (k.to_string(), n.id))).collect();
        v.sort();
        v
    }

    /// A free key near `key`: `key-2`, `key-3`, …
    pub fn free_key(&self, key: &str) -> String {
        (2..).map(|i| format!("{key}-{i}")).find(|k| self.node_by_key(k).is_none()).unwrap_or_else(|| key.to_string())
    }
}

#[cfg(test)]
mod tests {
    use crate::{Document, NodeKind};

    #[test]
    fn keys_are_found_listed_and_cleared() {
        let mut d = Document::new("k");
        let p = d.first_page().unwrap();
        let a = d.create(p, NodeKind::Rectangle, "A", 0.0, 0.0, 10.0, 10.0).unwrap();
        let b = d.create(p, NodeKind::Rectangle, "B", 0.0, 0.0, 10.0, 10.0).unwrap();
        d.get_mut(a).unwrap().set_key(Some("icon/folder"));
        d.get_mut(b).unwrap().set_key(Some("icon/file"));
        assert_eq!(d.node_by_key("icon/folder"), Some(a));
        assert_eq!(d.keys(), vec![("icon/file".to_string(), b), ("icon/folder".to_string(), a)]);
        assert_eq!(d.free_key("icon/file"), "icon/file-2");
        d.get_mut(a).unwrap().set_key(None);
        assert_eq!(d.node_by_key("icon/folder"), None);
        assert!(super::key_problem("").is_some() && super::key_problem(" x").is_some() && super::key_problem("a/b").is_none());
    }
}
