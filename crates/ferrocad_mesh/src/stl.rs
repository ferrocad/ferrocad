//! STL import and export.
//!
//! FreeCAD's `.bms` kernel format lives in [`crate::bms`]; this module is only the
//! STL side. The writer emits **binary** STL: an 80-byte header, a little-endian
//! `u32` facet count, then one 50-byte record per facet (a normal and three
//! vertices, each three `f32`, plus a `u16` attribute). The reader accepts binary
//! and ASCII STL and **welds exactly equal vertex coordinates**, so a box written
//! and read back is a closed manifold rather than 36 disconnected triangles.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use ferrocad_types::Vector3;

use crate::{Mesh, MeshError};

/// The fixed binary-STL header length.
const HEADER_LEN: usize = 80;
/// The fixed binary-STL record length.
const RECORD_LEN: usize = 50;

impl Mesh {
    /// Write this mesh to `path` as binary STL.
    ///
    /// Facet normals are computed from the winding; the placement is not applied.
    /// A mesh with more facets than `u32` can count is rejected with
    /// [`io::ErrorKind::InvalidInput`].
    pub fn write_stl(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let mut writer = io::BufWriter::new(fs::File::create(path)?);
        let mut header = [0u8; HEADER_LEN];
        let label = b"FerroCAD binary STL";
        header[..label.len()].copy_from_slice(label);
        writer.write_all(&header)?;

        let count = u32::try_from(self.count_facets()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the mesh has too many facets for binary STL",
            )
        })?;
        writer.write_all(&count.to_le_bytes())?;

        for facet in self.facets() {
            let a = self.points()[facet[0] as usize];
            let b = self.points()[facet[1] as usize];
            let c = self.points()[facet[2] as usize];
            write_vector(&mut writer, facet_normal(a, b, c))?;
            write_vector(&mut writer, a)?;
            write_vector(&mut writer, b)?;
            write_vector(&mut writer, c)?;
            writer.write_all(&0u16.to_le_bytes())?;
        }
        writer.flush()
    }

    /// Read a binary or ASCII STL file into a mesh, welding exactly equal points.
    ///
    /// The placement is the identity: STL has no placement field.
    pub fn read_stl(path: impl AsRef<Path>) -> Result<Mesh, MeshError> {
        let bytes = fs::read(path)?;
        let triangles = if is_binary(&bytes) {
            parse_binary(&bytes)?
        } else {
            parse_ascii(&bytes)?
        };
        Mesh::from_facet_points(&triangles)
    }
}

fn write_vector(writer: &mut impl Write, vector: Vector3) -> io::Result<()> {
    for component in [vector.x, vector.y, vector.z] {
        writer.write_all(&(component as f32).to_le_bytes())?;
    }
    Ok(())
}

/// The unit normal of a triangle, or zero for a degenerate (zero-area) one.
fn facet_normal(a: Vector3, b: Vector3, c: Vector3) -> Vector3 {
    let normal = b.sub(&a).cross(&c.sub(&a));
    let length = normal.length();
    if length > 0.0 && length.is_finite() {
        normal.scale(1.0 / length)
    } else {
        Vector3::zero()
    }
}

/// Whether `bytes` is binary STL rather than ASCII.
///
/// The exact-length check is authoritative (some binary writers start their header
/// with the word `solid`); otherwise a leading `solid` marks ASCII.
fn is_binary(bytes: &[u8]) -> bool {
    if bytes.len() >= HEADER_LEN + 4 {
        let count = u32::from_le_bytes(bytes[HEADER_LEN..HEADER_LEN + 4].try_into().unwrap());
        let expected = (count as usize)
            .checked_mul(RECORD_LEN)
            .and_then(|payload| payload.checked_add(HEADER_LEN + 4));
        if expected == Some(bytes.len()) {
            return true;
        }
    }
    let first = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .map_or(&[][..], |start| &bytes[start..]);
    !first.starts_with(b"solid")
}

fn parse_binary(bytes: &[u8]) -> Result<Vec<[Vector3; 3]>, MeshError> {
    if bytes.len() < HEADER_LEN + 4 {
        return Err(MeshError::MalformedStl(
            "binary STL is shorter than its header".to_string(),
        ));
    }
    let count = u32::from_le_bytes(bytes[HEADER_LEN..HEADER_LEN + 4].try_into().unwrap()) as usize;
    let expected = count
        .checked_mul(RECORD_LEN)
        .and_then(|payload| payload.checked_add(HEADER_LEN + 4))
        .ok_or_else(|| MeshError::MalformedStl("facet count overflows".to_string()))?;
    if bytes.len() < expected {
        return Err(MeshError::MalformedStl(format!(
            "binary STL declares {count} facets but is only {} bytes",
            bytes.len()
        )));
    }

    let mut triangles = Vec::with_capacity(count);
    for index in 0..count {
        let record = &bytes[HEADER_LEN + 4 + index * RECORD_LEN..][..RECORD_LEN];
        // The first three f32 are the normal; recompute it from the winding on use.
        let mut corners = [Vector3::zero(); 3];
        for (corner, slot) in corners.iter_mut().enumerate() {
            let offset = 12 + corner * 12;
            let x = read_f32(record, offset);
            let y = read_f32(record, offset + 4);
            let z = read_f32(record, offset + 8);
            *slot = Vector3::new(x, y, z);
        }
        triangles.push(corners);
    }
    Ok(triangles)
}

fn read_f32(bytes: &[u8], offset: usize) -> f64 {
    f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as f64
}

fn parse_ascii(bytes: &[u8]) -> Result<Vec<[Vector3; 3]>, MeshError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| MeshError::MalformedStl("ASCII STL is not valid UTF-8".to_string()))?;
    let mut tokens = text.split_ascii_whitespace();
    let mut triangles = Vec::new();
    let mut corners: Vec<Vector3> = Vec::with_capacity(3);

    while let Some(token) = tokens.next() {
        if !token.eq_ignore_ascii_case("vertex") {
            continue;
        }
        let mut coordinate = || -> Result<f64, MeshError> {
            tokens
                .next()
                .ok_or_else(|| MeshError::MalformedStl("truncated vertex".to_string()))?
                .parse::<f64>()
                .map_err(|_| {
                    MeshError::MalformedStl("vertex coordinate is not a number".to_string())
                })
        };
        let x = coordinate()?;
        let y = coordinate()?;
        let z = coordinate()?;
        corners.push(Vector3::new(x, y, z));
        if corners.len() == 3 {
            triangles.push([corners[0], corners[1], corners[2]]);
            corners.clear();
        }
    }

    if !corners.is_empty() {
        return Err(MeshError::MalformedStl(
            "ASCII STL ends with an incomplete facet".to_string(),
        ));
    }
    Ok(triangles)
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
            "ferrocad-mesh-{}-{unique}-{name}",
            std::process::id()
        ))
    }

    #[test]
    fn a_box_round_trips_through_binary_stl() {
        let mesh = Mesh::box_(2.0, 3.0, 4.0).unwrap();
        let path = temp_path("box.stl");
        mesh.write_stl(&path).unwrap();
        let read = Mesh::read_stl(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(read.count_facets(), mesh.count_facets());
        assert_eq!(read.count_points(), mesh.count_points());
        assert!((read.volume() - 24.0).abs() < 1e-6);
        assert!(read.is_solid(), "welding makes the read mesh closed");
        assert!(!read.has_non_manifolds());
    }

    #[test]
    fn an_ascii_stl_is_detected_and_read() {
        // A single tetrahedron face; enough to prove ASCII detection and parsing.
        let ascii = "\
solid tetra
  facet normal 0 0 1
    outer loop
      vertex 1 0 0
      vertex 0 1 0
      vertex 0 0 1
    endloop
  endfacet
endsolid tetra
";
        let path = temp_path("tetra.stl");
        fs::write(&path, ascii).unwrap();
        let mesh = Mesh::read_stl(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(mesh.count_facets(), 1);
        assert_eq!(mesh.count_points(), 3);
        let bounds = mesh.bounds();
        assert_eq!(bounds.min, Vector3::zero());
        assert_eq!(bounds.max, Vector3::new(1.0, 1.0, 1.0));
    }

    #[test]
    fn a_binary_header_that_starts_with_solid_is_still_binary() {
        let mesh = Mesh::box_(1.0, 1.0, 1.0).unwrap();
        let path = temp_path("solid-header.stl");
        mesh.write_stl(&path).unwrap();
        let mut bytes = fs::read(&path).unwrap();
        bytes[..5].copy_from_slice(b"solid");
        fs::write(&path, &bytes).unwrap();
        let read = Mesh::read_stl(&path).unwrap();
        let _ = fs::remove_file(&path);

        assert_eq!(read.count_facets(), 12);
        assert!(read.is_solid());
    }

    #[test]
    fn a_truncated_binary_stl_is_rejected() {
        let path = temp_path("truncated.stl");
        // A valid 84-byte header that claims one facet, but no record follows.
        let mut bytes = vec![0u8; HEADER_LEN + 4];
        bytes[HEADER_LEN..HEADER_LEN + 4].copy_from_slice(&1u32.to_le_bytes());
        fs::write(&path, bytes).unwrap();
        let error = Mesh::read_stl(&path).unwrap_err();
        let _ = fs::remove_file(&path);
        assert!(matches!(error, MeshError::MalformedStl(_)), "{error}");
    }
}
