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
            f64::NAN
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

    pub fn is_unity(&self, tol: f64) -> bool {
        Matrix4::identity()
            .m
            .iter()
            .zip(self.m.iter())
            .all(|(a, b)| (a - b).abs() <= tol)
    }

    pub fn is_null(&self) -> bool {
        self.m.iter().all(|v| v.abs() <= 1e-30)
    }

    pub fn unity(&mut self) {
        *self = Matrix4::identity();
    }

    pub fn nullify(&mut self) {
        self.m = [0.0; 16];
    }

    pub fn determinant(&self) -> f64 {
        let m = &self.m;
        m[0] * det3(m[5], m[6], m[7], m[9], m[10], m[11], m[13], m[14], m[15])
            - m[1] * det3(m[4], m[6], m[7], m[8], m[10], m[11], m[12], m[14], m[15])
            + m[2] * det3(m[4], m[5], m[7], m[8], m[9], m[11], m[12], m[13], m[15])
            - m[3] * det3(m[4], m[5], m[6], m[8], m[9], m[10], m[12], m[13], m[14])
    }

    pub fn transpose(&self) -> Matrix4 {
        let m = &self.m;
        let mut r = [0.0; 16];
        for i in 0..4 {
            for j in 0..4 {
                r[i * 4 + j] = m[j * 4 + i];
            }
        }
        Matrix4 { m: r }
    }

    /// The inverse, or `None` if the matrix is (near-)singular.
    pub fn inverse(&self) -> Option<Matrix4> {
        let det = self.determinant();
        if det.abs() < 1e-15 {
            return None;
        }
        // Adjugate / det.
        let m = &self.m;
        let mut r = [0.0; 16];
        for i in 0..4 {
            for j in 0..4 {
                // cofactor C_ji (transposed)
                let mut minor = [[0.0; 3]; 3];
                let (mut mi, mut mj) = (0, 0);
                for ri in 0..4 {
                    if ri == j {
                        continue;
                    }
                    mj = 0;
                    for ci in 0..4 {
                        if ci == i {
                            continue;
                        }
                        minor[mi][mj] = m[ri * 4 + ci];
                        mj += 1;
                    }
                    mi += 1;
                }
                let c = det3(
                    minor[0][0], minor[0][1], minor[0][2],
                    minor[1][0], minor[1][1], minor[1][2],
                    minor[2][0], minor[2][1], minor[2][2],
                );
                let sign = if (i + j) % 2 == 0 { 1.0 } else { -1.0 };
                r[i * 4 + j] = sign * c / det;
            }
        }
        Some(Matrix4 { m: r })
    }

    pub fn row(&self, r: usize) -> Vector3 {
        Vector3::new(self.m[r * 4], self.m[r * 4 + 1], self.m[r * 4 + 2])
    }

    pub fn set_row(&mut self, r: usize, v: Vector3) {
        self.m[r * 4] = v.x;
        self.m[r * 4 + 1] = v.y;
        self.m[r * 4 + 2] = v.z;
    }

    pub fn col(&self, c: usize) -> Vector3 {
        Vector3::new(self.m[c], self.m[4 + c], self.m[8 + c])
    }

    pub fn set_col(&mut self, c: usize, v: Vector3) {
        self.m[c] = v.x;
        self.m[4 + c] = v.y;
        self.m[8 + c] = v.z;
    }

    pub fn diagonal(&self) -> Vector3 {
        Vector3::new(self.m[0], self.m[5], self.m[10])
    }

    /// Pre-multiply by a translation.
    pub fn pre_move(&mut self, t: Vector3) {
        let mut m = Matrix4::identity();
        m.m[3] = t.x;
        m.m[7] = t.y;
        m.m[11] = t.z;
        *self = m.mul(self);
    }

    /// Pre-multiply by a (possibly non-uniform) scale.
    pub fn pre_scale(&mut self, s: Vector3) {
        let mut m = Matrix4::identity();
        m.m[0] = s.x;
        m.m[5] = s.y;
        m.m[10] = s.z;
        *self = m.mul(self);
    }

    /// Pre-multiply by a rotation about the X/Y/Z axis by `angle` (radians).
    pub fn pre_rotate(&mut self, axis: usize, angle: f64) {
        let unit = match axis {
            0 => Vector3::new(1.0, 0.0, 0.0),
            1 => Vector3::new(0.0, 1.0, 0.0),
            _ => Vector3::new(0.0, 0.0, 1.0),
        };
        let r = Rotation::from_axis_angle(&unit, angle).to_matrix();
        *self = r.mul(self);
    }
}

/// A 3x3 determinant (row-major).
fn det3(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64, g: f64, h: f64, i: f64) -> f64 {
    a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g)
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

    /// The normalized rotation axis (Z if the rotation is (near-)identity).
    pub fn axis(&self) -> Vector3 {
        let s = (1.0 - self.q[0] * self.q[0]).sqrt();
        if s < 1e-12 {
            Vector3::new(0.0, 0.0, 1.0)
        } else {
            Vector3::new(self.q[1] / s, self.q[2] / s, self.q[3] / s)
        }
    }

    /// Rebuild the rotation about the current axis with a new `angle` (radians).
    pub fn set_angle(&mut self, angle: f64) {
        *self = Rotation::from_axis_angle(&self.axis(), angle);
    }

    /// Quaternion product (`self * o`): apply `o` first, then `self`.
    pub fn multiply(&self, o: &Rotation) -> Rotation {
        let (w1, x1, y1, z1) = (self.q[0], self.q[1], self.q[2], self.q[3]);
        let (w2, x2, y2, z2) = (o.q[0], o.q[1], o.q[2], o.q[3]);
        Rotation {
            q: [
                w1 * w2 - x1 * x2 - y1 * y2 - z1 * z2,
                w1 * x2 + x1 * w2 + y1 * z2 - z1 * y2,
                w1 * y2 - x1 * z2 + y1 * w2 + z1 * x2,
                w1 * z2 + x1 * y2 - y1 * x2 + z1 * w2,
            ],
        }
    }

    /// The inverse rotation (quaternion conjugate).
    pub fn inverse(&self) -> Rotation {
        Rotation { q: [self.q[0], -self.q[1], -self.q[2], -self.q[3]] }
    }

    /// Whether two rotations are equal within `tol`.
    pub fn is_same(&self, o: &Rotation, tol: f64) -> bool {
        let dot = self.q[0] * o.q[0] + self.q[1] * o.q[1] + self.q[2] * o.q[2] + self.q[3] * o.q[3];
        (1.0 - dot.abs()) <= tol.max(1e-12)
    }

    /// FreeCAD's `Rotation(yaw, pitch, roll)` (degrees) = Rz*Ry*Rx.
    pub fn from_euler_deg(yaw: f64, pitch: f64, roll: f64) -> Rotation {
        Rotation::from_yaw_pitch_roll(
            yaw.to_radians(),
            pitch.to_radians(),
            roll.to_radians(),
        )
    }

    pub fn from_yaw_pitch_roll(yaw: f64, pitch: f64, roll: f64) -> Rotation {
        let rz = Rotation::from_axis_angle(&Vector3::new(0.0, 0.0, 1.0), yaw);
        let ry = Rotation::from_axis_angle(&Vector3::new(0.0, 1.0, 0.0), pitch);
        let rx = Rotation::from_axis_angle(&Vector3::new(1.0, 0.0, 0.0), roll);
        rz.multiply(&ry).multiply(&rx)
    }

    /// Extract `(yaw, pitch, roll)` (radians) from R = Rz*Ry*Rx.
    pub fn yaw_pitch_roll(&self) -> (f64, f64, f64) {
        let m = self.to_matrix().m;
        let (r00, r10, r20, r21, r22) = (m[0], m[4], m[8], m[9], m[10]);
        let pitch = (-r20).clamp(-1.0, 1.0).asin();
        if pitch.cos().abs() > 1e-6 {
            (r10.atan2(r00), pitch, r21.atan2(r22))
        } else {
            (0.0, pitch, r10.atan2(r00))
        }
    }

    /// Extract a rotation from the upper-left 3x3 block of a matrix.
    pub fn from_matrix(m: &Matrix4) -> Rotation {
        let (m00, m01, m02) = (m.m[0], m.m[1], m.m[2]);
        let (m10, m11, m12) = (m.m[4], m.m[5], m.m[6]);
        let (m20, m21, m22) = (m.m[8], m.m[9], m.m[10]);
        let trace = m00 + m11 + m22;
        let q = if trace > 0.0 {
            let s = (trace + 1.0).sqrt() * 2.0;
            [0.25 * s, (m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s]
        } else if m00 > m11 && m00 > m22 {
            let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
            [(m21 - m12) / s, 0.25 * s, (m01 + m10) / s, (m02 + m20) / s]
        } else if m11 > m22 {
            let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
            [(m02 - m20) / s, (m01 + m10) / s, 0.25 * s, (m12 + m21) / s]
        } else {
            let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
            [(m10 - m01) / s, (m02 + m20) / s, (m12 + m21) / s, 0.25 * s]
        };
        Rotation { q }
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

    /// The inverse placement.
    pub fn inverse(&self) -> Placement {
        let inv = self.rotation.inverse().to_matrix();
        Placement {
            base: inv.transform(&self.base.neg()),
            rotation: self.rotation.inverse(),
        }
    }

    /// Whether two placements are equal within `tol`.
    pub fn is_same(&self, o: &Placement, tol: f64) -> bool {
        self.base.is_equal(&o.base, tol.max(1e-12))
            && self.rotation.is_same(&o.rotation, tol)
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
