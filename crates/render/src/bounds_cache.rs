//! Paint bounds and splat answers kept from one render to the next (see [`crate::draw::Bounds`]).
//!
//! Both are worked out from a layer's subtree alone (plus the fonts, for text), so an answer stays
//! right until a layer in that subtree changes. The next render finds what changed with
//! [`Document::changed_ids`] against the document the answers were made for, and drops the answers
//! of the changed layers and of every layer above them.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use figcraft_doc::Document;

use crate::draw::{Learned, Seed};

/// More changed layers than this start over.
const MAX_CHANGED: usize = 4096;

struct Kept {
    doc: Document,
    fonts: u64,
    seed: Seed,
}

/// The latest render's answers. A render takes them, so renders at the same time don't share:
/// one of them starts without.
static KEPT: Mutex<Option<Kept>> = Mutex::new(None);

fn lock() -> MutexGuard<'static, Option<Kept>> {
    KEPT.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The answers still right for `doc`.
pub(crate) fn seed_for(doc: &Document) -> Seed {
    let Some(k) = lock().take() else { return Seed::default() };
    if k.fonts != figcraft_text::layout_generation() {
        return Seed::default();
    }
    refresh(k.seed, &k.doc, doc).unwrap_or_default()
}

/// `seed`, made for `old`, brought up to date for `doc`; `None` when too much changed.
pub(crate) fn refresh(mut seed: Seed, old: &Document, doc: &Document) -> Option<Seed> {
    let changed = doc.changed_ids(old, MAX_CHANGED)?;
    if changed.is_empty() {
        return Some(seed);
    }
    let greek = seed.greek;
    let paint = Arc::make_mut(&mut seed.paint);
    let splat = Arc::make_mut(&mut seed.splat);
    for id in changed {
        // A removed or moved layer's old ancestors are in the old document.
        for from in [doc, old] {
            for a in std::iter::once(id).chain(from.ancestors(id)) {
                paint.remove(&(a, greek));
                splat.remove(&a);
            }
        }
    }
    Some(seed)
}

/// Keep `seed` and what the render of `doc` learned on top of it, `greek` being the scale its text
/// was greeked at (none: in full). The paint answers of another scale are dropped, so zooming
/// doesn't pile up one set per scale.
pub(crate) fn keep(doc: &Document, mut seed: Seed, learned: Vec<Learned>, greek: Option<u64>) {
    let paint = Arc::make_mut(&mut seed.paint);
    if seed.greek != greek {
        paint.clear();
        seed.greek = greek;
    }
    let splat = Arc::make_mut(&mut seed.splat);
    for l in learned {
        paint.extend(l.paint);
        splat.extend(l.splat);
    }
    *lock() = Some(Kept { doc: doc.clone(), fonts: figcraft_text::layout_generation(), seed });
}
