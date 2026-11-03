//! The pull-based overlay registry (G5).
//!
//! `coin.addOverlay(node)` registers an extracted [`Scene`] beside the viewport's
//! main scene. The registry is a **pull** channel: the producer stores the scene and
//! marks the set dirty, and the host takes the scenes when it next draws. Nothing is
//! re-extracted or recompiled while the set is unchanged.
//!
//! The store lives in the pure inventor crate, not in the Python facade, so a
//! platform-neutral host (the browser) can drain the same registry without linking
//! CPython. Registration is keyed by an opaque `usize` the producer chooses (the
//! Python facade uses the wrapper's address, which is its identity); the pure crate
//! never interprets it and only compares keys.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::Scene;

fn entries() -> &'static Mutex<Vec<(usize, Scene)>> {
    static OVERLAYS: OnceLock<Mutex<Vec<(usize, Scene)>>> = OnceLock::new();
    OVERLAYS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Whether the registered set changed since the last [`take_scenes`] call.
static OVERLAYS_DIRTY: AtomicBool = AtomicBool::new(false);

/// Register `scene` under `key`, replacing a previous entry with the same key.
///
/// Re-registering is how a producer refreshes a live node: the tree is copied on
/// extraction (value semantics), so later mutations on the producer side are
/// invisible until the node is registered again.
pub fn put(key: usize, scene: Scene) {
    let mut entries = entries()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match entries.iter_mut().find(|(existing, _)| *existing == key) {
        Some((_, slot)) => *slot = scene,
        None => entries.push((key, scene)),
    }
    OVERLAYS_DIRTY.store(true, Ordering::Release);
}

/// Drop the overlay registered under `key`. Returns whether anything was removed.
pub fn remove(key: usize) -> bool {
    let mut entries = entries()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let before = entries.len();
    entries.retain(|(existing, _)| *existing != key);
    let removed = entries.len() != before;
    if removed {
        OVERLAYS_DIRTY.store(true, Ordering::Release);
    }
    removed
}

/// Drop every registered overlay. Returns whether anything was removed.
pub fn clear() -> bool {
    let mut entries = entries()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let cleared = !entries.is_empty();
    entries.clear();
    if cleared {
        OVERLAYS_DIRTY.store(true, Ordering::Release);
    }
    cleared
}

/// Whether the registered set changed since the last [`take_scenes`].
///
/// A pure check: the shell calls this cheaply on every refresh and only pays for
/// the clone below when it returns `true`.
pub fn dirty() -> bool {
    OVERLAYS_DIRTY.load(Ordering::Acquire)
}

/// The registered overlay scenes, in registration order.
///
/// Taking the scenes **clears** the dirty flag: this is the pull half of the
/// bridge. The shell merges the returned roots into the viewport's draw without
/// re-extracting or recompiling anything else.
pub fn take_scenes() -> Vec<Scene> {
    let entries = entries()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let scenes: Vec<Scene> = entries.iter().map(|(_, scene)| scene.clone()).collect();
    // Clear under the lock: an add between the clone and the store would otherwise
    // lose its dirty notification.
    OVERLAYS_DIRTY.store(false, Ordering::Release);
    scenes
}
