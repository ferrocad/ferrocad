//! Sprite atlas contract and tile identifiers shared by the engine and its
//! platform backends.

use crate::{RenderGlyphParams, RenderImageParams, RenderSvgParams};
use anyhow::Result;
use collections::FxHashMap;
use gpui_types::{Bounds, DevicePixels, Point, Size};
use std::{borrow::Cow, collections::hash_map::Entry, ops};

#[derive(PartialEq, Eq, Hash, Clone)]
#[expect(missing_docs)]
pub enum AtlasKey {
    Glyph(RenderGlyphParams),
    Svg(RenderSvgParams),
    Image(RenderImageParams),
}

impl AtlasKey {
    /// Returns the texture kind for this atlas key.
    pub fn texture_kind(&self) -> AtlasTextureKind {
        match self {
            AtlasKey::Glyph(params) => {
                if params.is_emoji {
                    AtlasTextureKind::Polychrome
                } else if params.subpixel_rendering {
                    AtlasTextureKind::Subpixel
                } else {
                    AtlasTextureKind::Monochrome
                }
            }
            AtlasKey::Svg(_) => AtlasTextureKind::Monochrome,
            AtlasKey::Image(_) => AtlasTextureKind::Polychrome,
        }
    }
}

impl From<RenderGlyphParams> for AtlasKey {
    fn from(params: RenderGlyphParams) -> Self {
        Self::Glyph(params)
    }
}

impl From<RenderSvgParams> for AtlasKey {
    fn from(params: RenderSvgParams) -> Self {
        Self::Svg(params)
    }
}

impl From<RenderImageParams> for AtlasKey {
    fn from(params: RenderImageParams) -> Self {
        Self::Image(params)
    }
}

#[expect(missing_docs)]
pub trait PlatformAtlas {
    /// The builder runs with the atlas locked and must not re-enter the same atlas.
    fn get_or_insert_with<'a>(
        &self,
        key: AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>>;
    fn remove(&self, key: &AtlasKey);

    #[cfg(any(test, feature = "test-support", feature = "bench-support"))]
    fn contains(&self, _key: &AtlasKey) -> bool {
        false
    }
}

#[doc(hidden)]
pub trait AtlasBackend {
    fn insert(
        &mut self,
        kind: AtlasTextureKind,
        size: Size<DevicePixels>,
        bytes: &[u8],
    ) -> Result<AtlasTile>;

    fn remove(&mut self, tile: AtlasTile);
}

#[doc(hidden)]
pub struct AtlasState<Backend> {
    tiles_by_key: FxHashMap<AtlasKey, AtlasTile>,
    pub backend: Backend,
}

impl<Backend> AtlasState<Backend> {
    pub fn new(backend: Backend) -> Self {
        Self {
            tiles_by_key: FxHashMap::default(),
            backend,
        }
    }

    pub fn contains(&self, key: &AtlasKey) -> bool {
        self.tiles_by_key.contains_key(key)
    }

    pub fn clear(&mut self, reset_backend: impl FnOnce(&mut Backend)) {
        self.tiles_by_key.clear();
        reset_backend(&mut self.backend);
    }
}

impl<Backend: Default> Default for AtlasState<Backend> {
    fn default() -> Self {
        Self::new(Backend::default())
    }
}

impl<Backend: AtlasBackend> AtlasState<Backend> {
    pub fn get_or_insert_with<'a>(
        &mut self,
        key: AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        match self.tiles_by_key.entry(key) {
            Entry::Occupied(entry) => Ok(Some(*entry.get())),
            Entry::Vacant(entry) => {
                profiling::scope!("new tile");
                let Some((size, bytes)) = build()? else {
                    return Ok(None);
                };
                let tile = self
                    .backend
                    .insert(entry.key().texture_kind(), size, &bytes)?;
                entry.insert(tile);
                Ok(Some(tile))
            }
        }
    }

    pub fn remove(&mut self, key: &AtlasKey) {
        if let Some(tile) = self.tiles_by_key.remove(key) {
            self.backend.remove(tile);
        }
    }
}

/// A sprite atlas for windows without a GPU. It hands out uniquely identified
/// tiles without uploading any pixels, so glyph, SVG, and image painting can
/// run to completion in tests and headless platforms.
#[derive(Default)]
pub struct HeadlessAtlas(parking_lot::Mutex<AtlasState<HeadlessAtlasBackend>>);

#[doc(hidden)]
#[derive(Default)]
pub struct HeadlessAtlasBackend {
    next_id: u32,
}

impl AtlasBackend for HeadlessAtlasBackend {
    fn insert(
        &mut self,
        kind: AtlasTextureKind,
        size: Size<DevicePixels>,
        _bytes: &[u8],
    ) -> Result<AtlasTile> {
        self.next_id += 1;
        let texture_id = self.next_id;
        self.next_id += 1;
        let tile_id = self.next_id;
        Ok(AtlasTile {
            texture_id: AtlasTextureId {
                index: texture_id,
                kind,
            },
            tile_id: TileId(tile_id),
            padding: 0,
            bounds: Bounds {
                origin: Point::default(),
                size,
            },
        })
    }

    fn remove(&mut self, _tile: AtlasTile) {}
}

impl PlatformAtlas for HeadlessAtlas {
    fn get_or_insert_with<'a>(
        &self,
        key: AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        self.0.lock().get_or_insert_with(key, build)
    }

    fn remove(&self, key: &AtlasKey) {
        self.0.lock().remove(key);
    }

    #[cfg(any(test, feature = "test-support", feature = "bench-support"))]
    fn contains(&self, key: &AtlasKey) -> bool {
        self.0.lock().contains(key)
    }
}

#[doc(hidden)]
pub struct AtlasTextureList<T> {
    pub textures: Vec<Option<T>>,
    pub free_list: Vec<usize>,
}

impl<T> Default for AtlasTextureList<T> {
    fn default() -> Self {
        Self {
            textures: Vec::default(),
            free_list: Vec::default(),
        }
    }
}

impl<T> ops::Index<usize> for AtlasTextureList<T> {
    type Output = Option<T>;

    fn index(&self, index: usize) -> &Self::Output {
        &self.textures[index]
    }
}

impl<T> AtlasTextureList<T> {
    #[allow(unused)]
    pub fn drain(&mut self) -> std::vec::Drain<'_, Option<T>> {
        self.free_list.clear();
        self.textures.drain(..)
    }

    #[allow(dead_code)]
    pub fn iter_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut T> {
        self.textures.iter_mut().flatten()
    }
}

/// A tile within a sprite atlas texture.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct AtlasTile {
    /// The texture this tile belongs to.
    pub texture_id: AtlasTextureId,
    /// The unique ID of this tile within its texture.
    pub tile_id: TileId,
    /// Padding around the tile content in pixels.
    pub padding: u32,
    /// The bounds of this tile within the texture.
    pub bounds: Bounds<DevicePixels>,
}

/// Identifies a texture within the sprite atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct AtlasTextureId {
    // We use u32 instead of usize for Metal Shader Language compatibility
    /// The index of this texture in the atlas.
    pub index: u32,
    /// The kind of content stored in this texture.
    pub kind: AtlasTextureKind,
}

/// The kind of content stored in an atlas texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
pub enum AtlasTextureKind {
    /// Single-channel coverage.
    Monochrome = 0,
    /// Full-color pixels.
    Polychrome = 1,
    /// Subpixel-antialiased coverage.
    Subpixel = 2,
}

/// The unique ID of a tile within its texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(C)]
pub struct TileId(pub u32);

impl From<etagere::AllocId> for TileId {
    fn from(id: etagere::AllocId) -> Self {
        Self(id.serialize())
    }
}

impl From<TileId> for etagere::AllocId {
    fn from(id: TileId) -> Self {
        Self::deserialize(id.0)
    }
}

#[cfg(test)]
mod atlas_tests {
    use super::*;

    use anyhow::Context as _;

    const TILE_SIZE: Size<DevicePixels> = Size {
        width: DevicePixels(1),
        height: DevicePixels(1),
    };

    #[derive(Default)]
    struct RecordingAtlasBackend {
        insert_calls: u32,
        fail_next_insert: bool,
        removed_tiles: Vec<AtlasTile>,
    }

    impl AtlasBackend for RecordingAtlasBackend {
        fn insert(
            &mut self,
            kind: AtlasTextureKind,
            size: Size<DevicePixels>,
            _bytes: &[u8],
        ) -> Result<AtlasTile> {
            self.insert_calls += 1;
            if std::mem::take(&mut self.fail_next_insert) {
                anyhow::bail!("backend failed");
            }
            Ok(AtlasTile {
                texture_id: AtlasTextureId { index: 0, kind },
                tile_id: TileId(self.insert_calls),
                padding: 0,
                bounds: Bounds {
                    origin: Point::default(),
                    size,
                },
            })
        }

        fn remove(&mut self, tile: AtlasTile) {
            self.removed_tiles.push(tile);
        }
    }

    fn image_key(image_id: usize) -> AtlasKey {
        AtlasKey::Image(RenderImageParams {
            image_id: crate::ImageId(image_id),
            frame_index: 0,
        })
    }

    fn build_tile() -> Result<Option<(Size<DevicePixels>, Cow<'static, [u8]>)>> {
        Ok(Some((TILE_SIZE, Cow::Borrowed(&[0, 0, 0, 255]))))
    }

    #[test]
    fn only_successful_inserts_are_cached() -> Result<()> {
        let mut state = AtlasState::new(RecordingAtlasBackend::default());
        let key = image_key(1);

        assert_eq!(
            state.get_or_insert_with(key.clone(), &mut || Ok(None))?,
            None
        );
        state
            .get_or_insert_with(key.clone(), &mut || anyhow::bail!("builder failed"))
            .expect_err("builder error should propagate");
        assert!(!state.contains(&key));
        assert_eq!(state.backend.insert_calls, 0);

        state.backend.fail_next_insert = true;
        state
            .get_or_insert_with(key.clone(), &mut build_tile)
            .expect_err("backend error should propagate");
        assert!(!state.contains(&key));
        assert_eq!(state.backend.insert_calls, 1);

        let tile = state
            .get_or_insert_with(key.clone(), &mut build_tile)?
            .context("builder should produce a tile")?;
        assert_eq!(tile.texture_id.kind, key.texture_kind());
        assert_eq!(
            state.get_or_insert_with(key.clone(), &mut || {
                anyhow::bail!("cache hit must not call the builder")
            })?,
            Some(tile)
        );
        assert!(state.contains(&key));
        assert_eq!(state.backend.insert_calls, 2);
        Ok(())
    }

    #[test]
    fn remove_and_clear_invalidate_keys() -> Result<()> {
        let mut state = AtlasState::new(RecordingAtlasBackend::default());
        let key = image_key(1);
        let other_key = image_key(2);
        let tile = state
            .get_or_insert_with(key.clone(), &mut build_tile)?
            .context("builder should produce a tile")?;
        state
            .get_or_insert_with(other_key.clone(), &mut build_tile)?
            .context("builder should produce another tile")?;

        state.remove(&key);
        state.remove(&key);
        assert!(!state.contains(&key));
        assert!(state.contains(&other_key));
        assert_eq!(state.backend.removed_tiles, vec![tile]);

        let mut reset_calls = 0;
        state.clear(|_| reset_calls += 1);
        assert_eq!(reset_calls, 1);
        assert!(!state.contains(&other_key));
        assert_eq!(state.backend.removed_tiles, vec![tile]);
        Ok(())
    }
}
