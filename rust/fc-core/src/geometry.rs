//! Core geometry types used by FreeCAD's `Base` module: `Vector3`, `Matrix4`,
//! `Rotation`, `Placement`, and `TypeId`.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Vector3
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vector3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vector3 {
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Vector3 { x, y, z }
    }

    pub fn zero() -> Self {
        Vector3::new(0.0, 0.0, 0.0)
    }

    pub fn add(&self, o: &Vector3) -> Vector3 {
        Vector3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }

    pub fn sub(&self, o: &Vector3) -> Vector3 {
        Vector3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }

    pub fn scale(&self, f: f64) -> Vector3 {
        Vector3::new(self.x * f, self.y * f, self.z * f)
    }

    pub fn neg(&self) -> Vector3 {
        Vector3::new(-self.x, -self.y, -self.z)
    }

    pub fn dot(&self, o: &Vector3) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(&self, o: &Vector3) -> Vector3 {
        Vector3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    pub fn length(&self) -> f64 {
        self.dot(self).sqrt()
    }

    pub fn normalize(&self) -> Vector3 {
        let l = self.length();
        if l == 0.0 {
            Vector3::zero()
        } else {
            self.scale(1.0 / l)
        }
    }

    pub fn distance(&self, o: &Vector3) -> f64 {
        self.sub(o).length()
    }

    pub fn angle(&self, o: &Vector3) -> f64 {
        let l = self.length() * o.length();
        if l == 0.0 {
            0.0
        } else {
            (self.dot(o) / l).clamp(-1.0, 1.0).acos()
        }
    }

    pub fn is_equal(&self, o: &Vector3, tol: f64) -> bool {
        self.distance(o) <= tol
    }
}

// ---------------------------------------------------------------------------
// Matrix4 (row-major [16]; identity is diag(1,1,1,1))
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Matrix4 {
    pub m: [f64; 16],
}

impl Matrix4 {
    pub fn identity() -> Self {
        let mut m = [0.0; 16];
        m[0] = 1.0;
        m[5] = 1.0;
        m[10] = 1.0;
        m[15] = 1.0;
        Matrix4 { m }
    }

    pub fn from_values(values: [f64; 16]) -> Self {
        Matrix4 { m: values }
    }

    /// Multiply two matrices (this * other).
    pub fn mul(&self, o: &Matrix4) -> Matrix4 {
        let mut r = [0.0; 16];
        for i in 0..4 {
            for j in 0..4 {
                let mut s = 0.0;
                for k in 0..4 {
                    s += self.m[i * 4 + k] * o.m[k * 4 + j];
                }
                r[i * 4 + j] = s;
            }
        }
        Matrix4 { m: r }
    }

    /// Transform a point (w=1), row-major `M * v`.
    pub fn transform(&self, v: &Vector3) -> Vector3 {
        let m = &self.m;
        let w = m[12] * v.x + m[13] * v.y + m[14] * v.z + m[15];
        let x = (m[0] * v.x + m[1] * v.y + m[2] * v.z + m[3]) / w;
        let y = (m[4] * v.x + m[5] * v.y + m[6] * v.z + m[7]) / w;
        let z = (m[8] * v.x + m[9] * v.y + m[10] * v.z + m[11]) / w;
        Vector3::new(x, y, z)
    }

    /// The 4x4 matrix elements in FreeCAD's A11..A44 order.
    pub fn values(&self) -> [f64; 16] {
        self.m
    }
}

// ---------------------------------------------------------------------------
// Rotation (quaternion: w, x, y, z)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rotation {
    pub q: [f64; 4],
}

impl Rotation {
    pub fn identity() -> Self {
        Rotation { q: [1.0, 0.0, 0.0, 0.0] }
    }

    /// Rotation around `axis` (normalized) by `angle` (radians).
    pub fn from_axis_angle(axis: &Vector3, angle: f64) -> Self {
        let a = axis.normalize();
        let half = angle * 0.5;
        let s = half.sin();
        Rotation { q: [half.cos(), a.x * s, a.y * s, a.z * s] }
    }

    pub fn to_matrix(&self) -> Matrix4 {
        let (w, x, y, z) = (self.q[0], self.q[1], self.q[2], self.q[3]);
        Matrix4::from_values([
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
            0.0,
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
            0.0,
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
        ])
    }

    /// Rotation angle in radians (about the rotation axis).
    pub fn angle(&self) -> f64 {
        2.0 * self.q[0].clamp(-1.0, 1.0).acos()
    }
}

// ---------------------------------------------------------------------------
// Placement (base point + rotation)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub base: Vector3,
    pub rotation: Rotation,
}

impl Placement {
    pub fn identity() -> Self {
        Placement { base: Vector3::zero(), rotation: Rotation::identity() }
    }

    pub fn new(base: Vector3, rotation: Rotation) -> Self {
        Placement { base, rotation }
    }

    /// Compose two placements (this * other).
    pub fn mul(&self, o: &Placement) -> Placement {
        let rot = self.rotation.to_matrix();
        Placement {
            base: self.base.add(&rot.transform(&o.base)),
            rotation: Rotation::identity(), // quaternion composition elided for POC
        }
    }

    pub fn to_matrix(&self) -> Matrix4 {
        let mut m = self.rotation.to_matrix();
        // Row-major: translation occupies the 4th column (m[3], m[7], m[11]).
        m.m[3] = self.base.x;
        m.m[7] = self.base.y;
        m.m[11] = self.base.z;
        m
    }
}

// ---------------------------------------------------------------------------
// TypeId (an interned type-name identifier)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TypeId(pub String);

impl TypeId {
    pub fn from_name(name: &str) -> Self {
        TypeId(name.to_string())
    }

    pub fn name(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_arithmetic() {
        let a = Vector3::new(1.0, 2.0, 3.0);
        let b = Vector3::new(4.0, 5.0, 6.0);
        assert_eq!(a.add(&b), Vector3::new(5.0, 7.0, 9.0));
        assert_eq!(a.dot(&b), 32.0);
        assert_eq!(a.cross(&b), Vector3::new(-3.0, 6.0, -3.0));
        assert!((Vector3::new(3.0, 4.0, 0.0).length() - 5.0).abs() < 1e-12);
    }

    #[test]
    fn matrix_identity_and_transform() {
        let id = Matrix4::identity();
        let v = Vector3::new(1.0, 2.0, 3.0);
        assert_eq!(id.transform(&v), v);
    }

    #[test]
    fn rotation_identity_angle() {
        let pi = std::f64::consts::PI;
        assert!(Rotation::identity().angle().abs() < 1e-12);
        let r = Rotation::from_axis_angle(&Vector3::new(0.0, 0.0, 1.0), pi);
        assert!((r.angle() - pi).abs() < 1e-12);
    }

    #[test]
    fn rotation_direction_matches_right_hand_rule() {
        let half = std::f64::consts::FRAC_PI_2;
        // Rotation about +Z by +90° maps +X to +Y.
        let r = Rotation::from_axis_angle(&Vector3::new(0.0, 0.0, 1.0), half);
        let v = r.to_matrix().transform(&Vector3::new(1.0, 0.0, 0.0));
        assert!(v.is_equal(&Vector3::new(0.0, 1.0, 0.0), 1e-9));

        // Rotation about +Y by -90° maps +X to +Z.
        let r = Rotation::from_axis_angle(&Vector3::new(0.0, 1.0, 0.0), -half);
        let v = r.to_matrix().transform(&Vector3::new(1.0, 0.0, 0.0));
        assert!(v.is_equal(&Vector3::new(0.0, 0.0, 1.0), 1e-9));
    }
}
