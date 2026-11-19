//! The pull-based overlay registry (G5).
//!
//! `coin.addOverlay(node)` registers an extracted [`Scene`] beside the viewport's
//! main scene. The registry is a **pull** channel: the producer stores the scene and
//! marks the set dirty, and the host takes the scenes when it next draws. Nothing is
//! re-extracted or recompiled while the set is unchanged.
//!
//! The store lives in the pure inventor crate, not in the Python facade, so a
//! platform-neutral host (the browser) can drain the same registry without linking
//! CPython. Registration is either **keyed** by an opaque `usize` the producer chooses
//! (the Python facade uses the wrapper's address, which is its identity) or **named**
//! (`put_named`), which is what a script runtime with no stable object identity uses.
//! The pure crate never interprets either key and only compares it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::Scene;

fn entries() -> &'static Mutex<Vec<(usize, Option<String>, Scene)>> {
    static OVERLAYS: OnceLock<Mutex<Vec<(usize, Option<String>, Scene)>>> = OnceLock::new();
    OVERLAYS.get_or_init(|| Mutex::new(Vec::new()))
}

/// A registered overlay scene and the provider feature that registered it, when known.
///
/// The role is what lets a host treat one overlay differently from the rest without
/// the mod naming itself: a scene registered while the `grid` provider activates is the
/// grid, and the viewport replays **it** on a candidate plane (the hover bloom) rather
/// than duplicating every overlay onto a plane its author never asked for. A scene with
/// no role still draws; it is simply never replayed on a second plane.
#[derive(Clone, Debug)]
pub struct OverlayScene {
    /// The provider feature the scene belongs to (`"grid"`), if the host knows it.
    pub role: Option<String>,
    /// The extracted root.
    pub scene: Scene,
}

/// Overlays registered by name rather than by object identity.
///
/// A JS workbench has no wrapper address to key a scene on: the page describes an
/// overlay, the host extracts it, and the next description replaces it. The name is
/// the producer's own stable id (`"axis-overlay-js"`), so re-registering is how it
/// refreshes and removing is how it switches off.
fn named_entries() -> &'static Mutex<Vec<(String, Scene)>> {
    static NAMED: OnceLock<Mutex<Vec<(String, Scene)>>> = OnceLock::new();
    NAMED.get_or_init(|| Mutex::new(Vec::new()))
}

/// Whether the registered set changed since the last [`take_scenes`] call.
static OVERLAYS_DIRTY: AtomicBool = AtomicBool::new(false);

/// Register `scene` under `key`, replacing a previous entry with the same key.
///
/// Re-registering is how a producer refreshes a live node: the tree is copied on
/// extraction (value semantics), so later mutations on the producer side are
/// invisible until the node is registered again.
pub fn put(key: usize, scene: Scene) {
    put_with_role(key, scene, None);
}

/// [`put`] with the provider feature that owns the scene.
///
/// A re-registration that passes no role keeps the one already stored: a mod
/// re-extracts its overlay from a settings callback as well as from a provider
/// callback, and only the latter knows the feature.
pub fn put_with_role(key: usize, scene: Scene, role: Option<String>) {
    let mut entries = entries()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match entries.iter_mut().find(|(existing, _, _)| *existing == key) {
        Some((_, stored, slot)) => {
            *slot = scene;
            if role.is_some() {
                *stored = role;
            }
        }
        None => entries.push((key, role, scene)),
    }
    OVERLAYS_DIRTY.store(true, Ordering::Release);
}

/// Drop the overlay registered under `key`. Returns whether anything was removed.
pub fn remove(key: usize) -> bool {
    let mut entries = entries()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let before = entries.len();
    entries.retain(|(existing, _, _)| *existing != key);
    let removed = entries.len() != before;
    if removed {
        OVERLAYS_DIRTY.store(true, Ordering::Release);
    }
    removed
}

/// Register `scene` under a stable name, replacing a previous entry with that name.
///
/// This is the key a script runtime uses: it has no object address to share with the
/// host, so the producer and the overlay description agree on a name instead.
pub fn put_named(name: impl Into<String>, scene: Scene) {
    let name = name.into();
    let mut entries = named_entries()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match entries.iter_mut().find(|(existing, _)| *existing == name) {
        Some((_, slot)) => *slot = scene,
        None => entries.push((name, scene)),
    }
    OVERLAYS_DIRTY.store(true, Ordering::Release);
}

/// Drop the overlay registered under `name`. Returns whether anything was removed.
pub fn remove_named(name: &str) -> bool {
    let mut entries = named_entries()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let before = entries.len();
    entries.retain(|(existing, _)| existing != name);
    let removed = entries.len() != before;
    if removed {
        OVERLAYS_DIRTY.store(true, Ordering::Release);
    }
    removed
}

/// The names currently registered by [`put_named`].
///
/// The host uses this to drop everything a script runtime registered when its page
/// goes away; the order is registration order.
pub fn named_keys() -> Vec<String> {
    named_entries()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .map(|(name, _)| name.clone())
        .collect()
}

/// Drop every registered overlay. Returns whether anything was removed.
pub fn clear() -> bool {
    let mut entries = entries()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let cleared = !entries.is_empty();
    entries.clear();
    drop(entries);
    let mut named = named_entries()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let cleared = cleared || !named.is_empty();
    named.clear();
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
/// Keyed overlays come first (the order a Python mod registered them), then named
/// overlays in name-registration order. Taking the scenes **clears** the dirty flag:
/// this is the pull half of the bridge. The shell merges the returned roots into the
/// viewport's draw without re-extracting or recompiling anything else.
pub fn take_scenes() -> Vec<Scene> {
    take_scenes_with_roles()
        .into_iter()
        .map(|overlay| overlay.scene)
        .collect()
}

/// [`take_scenes`] carrying each scene's provider role.
///
/// A host that only draws the scenes uses [`take_scenes`]; one that must tell the grid
/// from the rest (a candidate-plane replay) reads the roles here. Named overlays carry
/// no role: a page names its scenes for replacement, not for a provider feature.
pub fn take_scenes_with_roles() -> Vec<OverlayScene> {
    let entries = entries()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let named = named_entries()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let scenes: Vec<OverlayScene> = entries
        .iter()
        .map(|(_, role, scene)| OverlayScene {
            role: role.clone(),
            scene: scene.clone(),
        })
        .chain(named.iter().map(|(_, scene)| OverlayScene {
            role: None,
            scene: scene.clone(),
        }))
        .collect();
    // Clear under the locks: an add between the clone and the store would otherwise
    // lose its dirty notification.
    OVERLAYS_DIRTY.store(false, Ordering::Release);
    scenes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{Node, NodeKind};

    fn scene() -> Scene {
        Scene {
            root: Node {
                kind: NodeKind::Separator,
                children: Vec::new(),
            },
        }
    }

    #[test]
    fn a_named_overlay_replaces_and_removes_by_name() {
        clear();
        put_named("axis", scene());
        assert!(dirty(), "a registration marks the set dirty");
        assert_eq!(take_scenes().len(), 1);
        assert!(!dirty(), "taking the scenes clears the flag");

        put_named("axis", scene());
        assert_eq!(
            named_keys(),
            ["axis"],
            "re-registering replaces, not appends"
        );
        assert!(!remove_named("other"), "identity is the name");
        assert!(remove_named("axis"));
        assert!(named_keys().is_empty());
        assert!(!remove_named("axis"), "removing twice is a no-op");
        clear();
    }

    #[test]
    fn a_role_survives_a_re_registration_without_one() {
        clear();
        put_with_role(7, scene(), Some("grid".to_string()));
        let scenes = take_scenes_with_roles();
        assert_eq!(scenes.len(), 1);
        assert_eq!(scenes[0].role, Some("grid".to_string()));

        // A settings callback re-extracts the tree without knowing the feature: the
        // role is a property of who registered the scene, not of this update.
        put_with_role(7, scene(), Some("grid".to_string()));
        put(7, scene());
        let scenes = take_scenes_with_roles();
        assert_eq!(scenes.len(), 1);
        assert_eq!(scenes[0].role, Some("grid".to_string()));

        // A named overlay (a page) carries no role, and `take_scenes` drains the same
        // set without exposing the roles.
        remove(7);
        put_named("axis", scene());
        let scenes = take_scenes_with_roles();
        assert_eq!(scenes.len(), 1);
        assert_eq!(scenes[0].role, None);

        put_named("axis", scene());
        assert_eq!(take_scenes().len(), 1);
        clear();
    }

    #[test]
    fn keyed_and_named_overlays_share_one_pull() {
        clear();
        put(7, scene());
        put_named("axis", scene());
        assert_eq!(take_scenes().len(), 2);
        let names = named_keys();
        assert_eq!(names, ["axis"]);

        // `clear` drops both kinds, and reports the change once.
        assert!(clear());
        assert!(named_keys().is_empty());
        assert!(take_scenes().is_empty());
        assert!(!clear());
    }
}
