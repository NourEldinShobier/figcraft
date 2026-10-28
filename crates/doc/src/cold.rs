//! `Cold<T>`: a rarely non-default payload kept out of line. A node whose value equals the default
//! stores one null pointer instead of the whole struct; reads see the shared default, writes
//! allocate. Compares, clones and (de)serialises exactly like `T`.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::ops::{Deref, DerefMut};

/// Types usable in [`Cold`]: a process-wide default value to read through when nothing is stored.
pub trait ColdDefault: Default + PartialEq + 'static {
    fn shared_default() -> &'static Self;
}

/// Implements [`ColdDefault`] with a lazily built static default.
#[macro_export]
macro_rules! cold_default {
    ($t:ty) => {
        impl $crate::cold::ColdDefault for $t {
            fn shared_default() -> &'static Self {
                static D: std::sync::OnceLock<$t> = std::sync::OnceLock::new();
                D.get_or_init(<$t as Default>::default)
            }
        }
    };
}

pub struct Cold<T: ColdDefault>(Option<Box<T>>);

impl<T: ColdDefault> Cold<T> {
    pub fn new(v: T) -> Self {
        if v == *T::shared_default() { Cold(None) } else { Cold(Some(Box::new(v))) }
    }
    /// Take the value out (the default when nothing was stored).
    pub fn into_inner(self) -> T {
        self.0.map(|b| *b).unwrap_or_default()
    }
}

impl<T: ColdDefault> Default for Cold<T> {
    fn default() -> Self {
        Cold(None)
    }
}
impl<T: ColdDefault> From<T> for Cold<T> {
    fn from(v: T) -> Self {
        Cold::new(v)
    }
}
impl<T: ColdDefault> Deref for Cold<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0.as_deref().unwrap_or_else(|| T::shared_default())
    }
}
impl<T: ColdDefault> DerefMut for Cold<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.0.get_or_insert_with(Default::default)
    }
}
impl<T: ColdDefault + Clone> Clone for Cold<T> {
    fn clone(&self) -> Self {
        Cold(self.0.clone())
    }
}
impl<T: ColdDefault> PartialEq for Cold<T> {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}
impl<T: ColdDefault + std::fmt::Debug> std::fmt::Debug for Cold<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}
impl<T: ColdDefault + Serialize> Serialize for Cold<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        (**self).serialize(s)
    }
}
impl<'de, T: ColdDefault + Deserialize<'de>> Deserialize<'de> for Cold<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        T::deserialize(d).map(Cold::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
    struct P {
        a: u32,
        b: String,
    }
    cold_default!(P);

    #[test]
    fn default_is_not_stored_and_reads_like_the_value() {
        let mut c: Cold<P> = Cold::default();
        assert!(c.0.is_none());
        assert_eq!(c.a, 0);
        c.a = 7;
        assert!(c.0.is_some());
        assert_eq!(serde_json::to_string(&c).unwrap(), r#"{"a":7,"b":""}"#);
        let back: Cold<P> = serde_json::from_str(r#"{"a":0,"b":""}"#).unwrap();
        assert!(back.0.is_none());
        assert_eq!(back, Cold::default());
        assert_eq!(Cold::new(P { a: 0, b: String::new() }), c.clone().tap_zero());
    }

    impl Cold<P> {
        fn tap_zero(mut self) -> Self {
            self.a = 0;
            self
        }
    }
}
