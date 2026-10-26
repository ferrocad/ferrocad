//! FreeCAD's `.bms` mesh format (`MeshIO::BMS`, `MeshKernel::Write`/`Read`).
//!
//! `.bms` is a compact binary kernel dump: a magic/version pair, a free-form
//! 256-byte info field, the point and facet counts, `f32` points, and per facet
//! three point indices plus three neighbour indices (`0xFFFFFFFF` for an open
//! edge). It ends with the stored bounding box (`minx, maxx, miny, maxy, minz,
//! maxz`, all `f32`). Unlike STL it stores the point array itself, so **duplicate
//! points keep distinct identities** and facet indices survive a round trip
//! unchanged.
//!
//! The reader accepts little- and big-endian files (upstream detects the byte
//! order by trying the swapped magic). The writer emits little-endian, like
//! upstream on every platform we run on.
//!
//! # The transform
//!
//! Upstream's `.bms` has no transform field: `MeshObject::save` writes the local
//! kernel (`Mesh.cpp:359-404`) and `MeshKernel::Write` has nowhere to put a
//! matrix. The 256-byte info field is free-form and ignored by FreeCAD's reader,
//! so this writer stores the mesh [`Placement`] there under a `FerroCAD-BMS`
//! marker; files written by FreeCAD (the `MESH-MESH-…` info string) still read as
//! identity placement. That is an additive extension: the rest of the file is
//! byte-for-byte the upstream layout.

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::Path;

use ferrocad_types::{Placement, Rotation, Vector3};

use crate::mesh::directed_edges;
use crate::{Mesh, MeshError};

/// `MeshKernel::Write`'s magic number.
const MAGIC: u32 = 0xA0B0_C0D0;
/// The only version upstream writes (`0x010000`).
const VERSION: u32 = 0x0001_0000;
/// The fixed info-field length.
const INFO_LEN: usize = 256;
/// The bytes before the first point: magic + version + info + counts.
const HEADER_LEN: usize = 8 + INFO_LEN + 8;
/// The neighbour value that marks an open edge.
const OPEN_EDGE: u32 = u32::MAX;
/// Marks a FerroCAD placement inside the free-form info field.
const INFO_MAGIC: &[u8] = b"FerroCAD-BMS\0";
/// The placement payload version inside the info field.
const INFO_VERSION: u32 = 1;

impl Mesh {
    /// Write this mesh to `path` as FreeCAD `.bms`.
    ///
    /// Points are written as `f32` (the format's precision) in the mesh's
    /// **local** frame; the placement rides in the info field, not in the
    /// points. Facet neighbours are derived from shared undirected edges: an
    /// edge used by exactly two facets links them, and a border or non-manifold
    /// edge is written as open.
    pub fn write_bms(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let point_count = u32::try_from(self.count_points()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the mesh has too many points for BMS",
            )
        })?;
        let facet_count = u32::try_from(self.count_facets()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the mesh has too many facets for BMS",
            )
        })?;

        let mut writer = BufWriter::new(fs::File::create(path)?);
        writer.write_all(&MAGIC.to_le_bytes())?;
        writer.write_all(&VERSION.to_le_bytes())?;
        writer.write_all(&encode_info(self.placement()))?;
        writer.write_all(&point_count.to_le_bytes())?;
        writer.write_all(&facet_count.to_le_bytes())?;

        for point in self.points() {
            write_f32(&mut writer, point.x)?;
            write_f32(&mut writer, point.y)?;
            write_f32(&mut writer, point.z)?;
        }

        let neighbours = facet_neighbours(self);
        for (facet, adjacent) in self.facets().iter().zip(&neighbours) {
            for &index in facet {
                writer.write_all(&index.to_le_bytes())?;
            }
            for &index in adjacent {
                writer.write_all(&index.to_le_bytes())?;
            }
        }

        let bounds = self.bounds();
        for value in [
            bounds.min.x,
            bounds.max.x,
            bounds.min.y,
            bounds.max.y,
            bounds.min.z,
            bounds.max.z,
        ] {
            write_f32(&mut writer, value)?;
        }
        writer.flush()
    }

    /// Read a FreeCAD `.bms` file into a mesh.
    ///
    /// Points are not welded: their indices, including duplicates, are exactly
    /// the ones stored in the file. The placement is the identity for a file
    /// written by FreeCAD, or the one this writer recorded in the info field.
    /// A file whose point index, facet neighbour or facet indices are invalid is
    /// rejected with [`MeshError::MalformedBms`].
    pub fn read_bms(path: impl AsRef<Path>) -> Result<Mesh, MeshError> {
        let bytes = fs::read(path)?;
        parse(&bytes)
    }
}

fn parse(bytes: &[u8]) -> Result<Mesh, MeshError> {
    if bytes.len() < HEADER_LEN {
        return Err(malformed("the file is shorter than the BMS header"));
    }
    let magic = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
    let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    let endian = if magic == MAGIC && version == VERSION {
        Endian::Little
    } else if magic.swap_bytes() == MAGIC && version.swap_bytes() == VERSION {
        Endian::Big
    } else {
        return Err(malformed("the magic number or version is not BMS"));
    };

    let placement = decode_info(&bytes[8..8 + INFO_LEN]);
    let mut offset = 8 + INFO_LEN;
    let point_count = endian.read_u32(&bytes[offset..offset + 4]) as usize;
    offset += 4;
    let facet_count = endian.read_u32(&bytes[offset..offset + 4]) as usize;
    offset += 4;

    let needed = point_count
        .checked_mul(12)
        .and_then(|points| points.checked_add(facet_count.checked_mul(24)?))
        .and_then(|payload| payload.checked_add(24));
    let Some(needed) = needed else {
        return Err(malformed("the point or facet count overflows"));
    };
    if bytes.len() < offset.saturating_add(needed) {
        return Err(malformed(format!(
            "the file declares {point_count} points and {facet_count} facets but is only {} bytes",
            bytes.len()
        )));
    }

    let mut points = Vec::with_capacity(point_count);
    for _ in 0..point_count {
        let x = endian.read_f32(&bytes[offset..offset + 4]);
        let y = endian.read_f32(&bytes[offset + 4..offset + 8]);
        let z = endian.read_f32(&bytes[offset + 8..offset + 12]);
        offset += 12;
        points.push(Vector3::new(x, y, z));
    }

    let mut facets = Vec::with_capacity(facet_count);
    for facet_index in 0..facet_count {
        let mut facet = [0u32; 3];
        for slot in facet.iter_mut() {
            *slot = endian.read_u32(&bytes[offset..offset + 4]);
            offset += 4;
        }
        for &index in &facet {
            if index as usize >= point_count {
                return Err(malformed(format!(
                    "facet {facet_index} references point {index} but the file has {point_count} points"
                )));
            }
        }
        for _ in 0..3 {
            let neighbour = endian.read_u32(&bytes[offset..offset + 4]);
            offset += 4;
            if neighbour != OPEN_EDGE && neighbour as usize >= facet_count {
                return Err(malformed(format!(
                    "facet {facet_index} has a neighbour index {neighbour} outside {facet_count} facets"
                )));
            }
        }
        facets.push(facet);
    }

    // The bounding box is stored for readers that want it without scanning the
    // points; we recompute it, so it is read past rather than trusted.
    let _ = endian.read_f32(&bytes[offset..offset + 4]);

    let mut mesh = Mesh::new(points, facets)?;
    mesh.set_placement(placement);
    Ok(mesh)
}

/// Wrap a parse failure as a malformed-BMS error.
fn malformed(message: impl Into<String>) -> MeshError {
    MeshError::MalformedBms(message.into())
}

/// The byte order of a BMS payload.
#[derive(Clone, Copy)]
enum Endian {
    Little,
    Big,
}

impl Endian {
    fn read_u32(self, bytes: &[u8]) -> u32 {
        let bytes: [u8; 4] = bytes.try_into().expect("a four-byte slice");
        match self {
            Endian::Little => u32::from_le_bytes(bytes),
            Endian::Big => u32::from_be_bytes(bytes),
        }
    }

    fn read_f32(self, bytes: &[u8]) -> f64 {
        let bytes: [u8; 4] = bytes.try_into().expect("a four-byte slice");
        let value = match self {
            Endian::Little => f32::from_le_bytes(bytes),
            Endian::Big => f32::from_be_bytes(bytes),
        };
        f64::from(value)
    }
}

fn write_f32(writer: &mut impl Write, value: f64) -> io::Result<()> {
    let narrowed = value as f32;
    if !narrowed.is_finite() && value.is_finite() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a coordinate is too large for the f32 BMS payload",
        ));
    }
    writer.write_all(&narrowed.to_le_bytes())
}

/// The neighbour across each facet side (side `i` is the edge from point `i` to
/// point `i + 1`). A side with zero or more than two facets is open.
fn facet_neighbours(mesh: &Mesh) -> Vec<[u32; 3]> {
    let mut by_edge: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for (index, facet) in mesh.facets().iter().enumerate() {
        for (a, b) in directed_edges(*facet) {
            let key = if a < b { (a, b) } else { (b, a) };
            by_edge.entry(key).or_default().push(index as u32);
        }
    }

    let mut neighbours = vec![[OPEN_EDGE; 3]; mesh.count_facets()];
    for (index, facet) in mesh.facets().iter().enumerate() {
        for (side, (a, b)) in directed_edges(*facet).into_iter().enumerate() {
            let key = if a < b { (a, b) } else { (b, a) };
            let Some(bucket) = by_edge.get(&key) else {
                continue;
            };
            if bucket.len() == 2 {
                let index = index as u32;
                neighbours[index as usize][side] =
                    if bucket[0] == index { bucket[1] } else { bucket[0] };
            }
        }
    }
    neighbours
}

/// Encode a placement into the free-form 256-byte info field.
fn encode_info(placement: Placement) -> [u8; INFO_LEN] {
    let mut info = [0u8; INFO_LEN];
    let mut offset = INFO_MAGIC.len();
    info[..offset].copy_from_slice(INFO_MAGIC);
    info[offset..offset + 4].copy_from_slice(&INFO_VERSION.to_le_bytes());
    offset += 4;
    for value in placement.rotation.q {
        info[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        offset += 8;
    }
    for value in [placement.base.x, placement.base.y, placement.base.z] {
        info[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        offset += 8;
    }
    info
}

/// Read a placement from the info field; anything without our marker (including
/// FreeCAD's `MESH-MESH-…` string) means identity.
fn decode_info(info: &[u8]) -> Placement {
    if !info.starts_with(INFO_MAGIC) {
        return Placement::identity();
    }
    let mut offset = INFO_MAGIC.len();
    let Some(version) = info.get(offset..offset + 4) else {
        return Placement::identity();
    };
    if u32::from_le_bytes(version.try_into().unwrap()) != INFO_VERSION {
        return Placement::identity();
    }
    offset += 4;
    let mut next = || -> Option<f64> {
        let bytes: [u8; 8] = info.get(offset..offset + 8)?.try_into().ok()?;
        offset += 8;
        Some(f64::from_le_bytes(bytes))
    };
    let Some(q) = (|| Some([next()?, next()?, next()?, next()?]))() else {
        return Placement::identity();
    };
    let Some(base) = (|| Some(Vector3::new(next()?, next()?, next()?)))() else {
        return Placement::identity();
    };
    if q.iter().chain([base.x, base.y, base.z].iter()).any(|v| !v.is_finite()) {
        return Placement::identity();
    }
    Placement::new(base, Rotation { q, raw_axis: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_path(name: &str) -> PathBuf {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "ferrocad-bms-{}-{unique}-{name}",
            std::process::id()
        ))
    }

    #[test]
    fn a_box_round_trips_through_bms() {
        let mesh = Mesh::box_(2.0, 3.0, 4.0).unwrap();
        let path = temp_path("box.bms");
        mesh.write_bms(&path).unwrap();
        let read = Mesh::read_bms(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(read.count_points(), mesh.count_points());
        assert_eq!(read.count_facets(), mesh.count_facets());
        assert_eq!(read.facets(), mesh.facets(), "indices survive verbatim");
        assert_eq!(read.bounds().max, Vector3::new(1.0, 1.5, 2.0));
        assert!(read.is_solid());
        assert!(!read.has_non_manifolds());
        assert_eq!(read.placement(), Placement::identity());
        assert!((read.volume() - 24.0).abs() < 1e-5);
    }

    #[test]
    fn bms_preserves_duplicate_point_identities() {
        // Two facets share the coordinate (1, 0, 0) through *different* point
        // indices. STL would weld them; BMS must not.
        let mesh = Mesh::new(
            vec![
                Vector3::new(0.0, 0.0, 0.0),
                Vector3::new(1.0, 0.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
                Vector3::new(1.0, 0.0, 0.0),
            ],
            vec![[0, 1, 2], [0, 3, 2]],
        )
        .unwrap();
        let path = temp_path("duplicates.bms");
        mesh.write_bms(&path).unwrap();
        let read = Mesh::read_bms(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(read.count_points(), 4);
        assert_eq!(read.count_facets(), 2);
        assert_eq!(read.facets(), &[[0, 1, 2], [0, 3, 2]]);
        assert_eq!(read.points()[1], read.points()[3]);
    }

    #[test]
    fn bms_round_trips_the_placement() {
        let mut mesh = Mesh::box_(1.0, 1.0, 1.0).unwrap();
        let placement = Placement::new(
            Vector3::new(4.0, 5.0, 6.0),
            Rotation::from_axis_angle(&Vector3::new(1.0, 2.0, 3.0), 0.75),
        );
        mesh.set_placement(placement);
        let path = temp_path("placed.bms");
        mesh.write_bms(&path).unwrap();
        let read = Mesh::read_bms(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(read.placement(), placement);
        assert_eq!(read.points(), mesh.points(), "the transform is not baked");
    }

    #[test]
    fn the_writer_uses_the_upstream_header_and_derives_neighbours() {
        let mesh = Mesh::box_(1.0, 1.0, 1.0).unwrap();
        let path = temp_path("layout.bms");
        mesh.write_bms(&path).unwrap();
        let bytes = fs::read(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(&bytes[0..4], &MAGIC.to_le_bytes());
        assert_eq!(&bytes[4..8], &VERSION.to_le_bytes());
        assert_eq!(
            u32::from_le_bytes(bytes[264..268].try_into().unwrap()),
            8,
            "point count"
        );
        assert_eq!(
            u32::from_le_bytes(bytes[268..272].try_into().unwrap()),
            12,
            "facet count"
        );
        // Facet 0 is [0, 2, 1]; its three neighbours are across 0-2, 2-1 and 1-0.
        let facets = 272 + 8 * 12;
        let facet_0 = &bytes[facets..facets + 24];
        let neighbour = |side: usize| {
            u32::from_le_bytes(facet_0[12 + side * 4..16 + side * 4].try_into().unwrap())
        };
        assert_eq!([neighbour(0), neighbour(1), neighbour(2)], [1, 10, 4]);
    }

    #[test]
    fn an_unrelated_file_is_rejected() {
        let path = temp_path("junk.bms");
        fs::write(&path, b"not a mesh file at all").unwrap();
        let error = Mesh::read_bms(&path).unwrap_err();
        let _ = fs::remove_file(&path);
        assert!(matches!(error, MeshError::MalformedBms(_)), "{error}");
    }

    #[test]
    fn a_truncated_payload_is_rejected() {
        let mesh = Mesh::box_(1.0, 1.0, 1.0).unwrap();
        let path = temp_path("truncated.bms");
        mesh.write_bms(&path).unwrap();
        let mut bytes = fs::read(&path).unwrap();
        bytes.truncate(bytes.len() - 12);
        fs::write(&path, &bytes).unwrap();
        let error = Mesh::read_bms(&path).unwrap_err();
        let _ = fs::remove_file(&path);
        assert!(matches!(error, MeshError::MalformedBms(_)), "{error}");
    }
}
