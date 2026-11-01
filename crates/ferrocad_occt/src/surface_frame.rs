//! The typed replacement for `fc_face_surface_frame`.
//!
//! The old bridge flattened a face's backing elementary surface into a
//! `Vec<f64>` and let `backend::surface_data` re-assemble it by index. This
//! module reads the same `BRepAdaptor_Surface`/`gp_*` accessors directly and
//! returns the neutral [`SurfaceFrame`], which `backend::surface_data` maps onto
//! the public `ferrocad_geom::SurfaceData`.

use ferrocad_curves::Vector3;
use ferrocad_geom::SurfaceData;
use opencascade_sys as ffi;

use ffi::geom_abs::GeomAbs_SurfaceType;
use ffi::topo_ds::TopoDS_Face;

/// The frame shared by every elementary surface.
pub struct SurfaceFrame {
    pub origin: Vector3,
    /// Plane normal / cylinder-cone-sphere-torus axis.
    pub axis: Vector3,
    pub x_dir: Vector3,
    pub direct: bool,
    pub shape: SurfaceFrameShape,
}

/// The per-kind payload carried beside the frame.
pub enum SurfaceFrameShape {
    Plane { y_dir: Vector3 },
    Cylinder { radius: f64 },
    Sphere { radius: f64 },
    Cone { radius: f64, semi_angle: f64 },
    Torus { major: f64, minor: f64 },
}

impl SurfaceFrame {
    /// The `ferrocad_geom` classification of this surface.
    pub fn into_surface_data(self) -> SurfaceData {
        match self.shape {
            SurfaceFrameShape::Plane { y_dir } => SurfaceData::Plane {
                origin: self.origin,
                x_dir: self.x_dir,
                y_dir,
                normal: self.axis,
            },
            SurfaceFrameShape::Cylinder { radius } => SurfaceData::Cylinder {
                origin: self.origin,
                axis: self.axis,
                x_dir: self.x_dir,
                radius,
                direct: self.direct,
            },
            SurfaceFrameShape::Sphere { radius } => SurfaceData::Sphere {
                center: self.origin,
                axis: self.axis,
                x_dir: self.x_dir,
                radius,
                direct: self.direct,
            },
            SurfaceFrameShape::Cone { radius, semi_angle } => SurfaceData::Cone {
                origin: self.origin,
                axis: self.axis,
                x_dir: self.x_dir,
                radius,
                semi_angle,
                direct: self.direct,
            },
            SurfaceFrameShape::Torus { major, minor } => SurfaceData::Torus {
                center: self.origin,
                axis: self.axis,
                x_dir: self.x_dir,
                major_radius: major,
                minor_radius: minor,
                direct: self.direct,
            },
        }
    }
}

/// The elementary surface frame behind `face`, or `None` for any other kind.
pub fn of(face: &TopoDS_Face) -> Option<SurfaceFrame> {
    let surface = ffi::b_rep_adaptor::BRepAdaptor_Surface_new(face);
    match ffi::b_rep_adaptor::BRepAdaptor_Surface_GetType(&surface) {
        GeomAbs_SurfaceType::GeomAbs_Plane => {
            let plane = ffi::b_rep_adaptor::BRepAdaptor_Surface_Plane(&surface).ok()?;
            let pos = ffi::gp::gp_Pln_Position(&plane);
            Some(SurfaceFrame {
                origin: point(pos.Location()),
                axis: dir(pos.Direction()),
                x_dir: dir(pos.XDirection()),
                direct: pos.Direct(),
                shape: SurfaceFrameShape::Plane {
                    y_dir: dir(pos.YDirection()),
                },
            })
        }
        GeomAbs_SurfaceType::GeomAbs_Cylinder => {
            let cylinder = ffi::b_rep_adaptor::BRepAdaptor_Surface_Cylinder(&surface).ok()?;
            let pos = ffi::gp::gp_Cylinder_Position(&cylinder);
            Some(SurfaceFrame {
                origin: point(pos.Location()),
                axis: dir(pos.Direction()),
                x_dir: dir(pos.XDirection()),
                direct: pos.Direct(),
                shape: SurfaceFrameShape::Cylinder {
                    radius: cylinder.Radius(),
                },
            })
        }
        GeomAbs_SurfaceType::GeomAbs_Sphere => {
            let sphere = ffi::b_rep_adaptor::BRepAdaptor_Surface_Sphere(&surface).ok()?;
            let pos = ffi::gp::gp_Sphere_Position(&sphere);
            Some(SurfaceFrame {
                origin: point(pos.Location()),
                axis: dir(pos.Direction()),
                x_dir: dir(pos.XDirection()),
                direct: pos.Direct(),
                shape: SurfaceFrameShape::Sphere {
                    radius: sphere.Radius(),
                },
            })
        }
        GeomAbs_SurfaceType::GeomAbs_Cone => {
            let cone = ffi::b_rep_adaptor::BRepAdaptor_Surface_Cone(&surface).ok()?;
            let pos = ffi::gp::gp_Cone_Position(&cone);
            Some(SurfaceFrame {
                origin: point(pos.Location()),
                axis: dir(pos.Direction()),
                x_dir: dir(pos.XDirection()),
                direct: pos.Direct(),
                shape: SurfaceFrameShape::Cone {
                    radius: cone.RefRadius(),
                    semi_angle: cone.SemiAngle(),
                },
            })
        }
        GeomAbs_SurfaceType::GeomAbs_Torus => {
            let torus = ffi::b_rep_adaptor::BRepAdaptor_Surface_Torus(&surface).ok()?;
            let pos = ffi::gp::gp_Torus_Position(&torus);
            Some(SurfaceFrame {
                origin: point(pos.Location()),
                axis: dir(pos.Direction()),
                x_dir: dir(pos.XDirection()),
                direct: pos.Direct(),
                shape: SurfaceFrameShape::Torus {
                    major: torus.MajorRadius(),
                    minor: torus.MinorRadius(),
                },
            })
        }
        _ => None,
    }
}

fn point(point: &ffi::gp::gp_Pnt) -> Vector3 {
    Vector3::new(point.X(), point.Y(), point.Z())
}

fn dir(dir: &ffi::gp::gp_Dir) -> Vector3 {
    Vector3::new(dir.X(), dir.Y(), dir.Z())
}
