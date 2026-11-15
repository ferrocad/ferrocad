//! The whole appearance policy, read from and written to the host's settings store.
//!
//! [`grid_settings`](crate::grid_settings) owns the `Settings/Grid` card. This module is
//! the other half of the same seam: it turns every appearance group the native
//! providers draw from — grid, axis cross and transform gizmo — into the one
//! [`ViewRequest`] a host applies at boot. The browser has no script half to answer
//! `FerroCAD.Grid/Axis/Gizmo`, and the desktop can be launched with no `mods/` (so the
//! document half answers), so both builds read the same groups from the host store:
//!
//! * `Mods/grid/ferrocad`
//! * `Mods/axis/ferrocad`
//! * `Mods/gizmo/ferrocad`
//!
//! A missing key keeps the shell's default, exactly as an omitted field in a
//! `ViewRequest` does. The provider claims (`grid_overlay`, `axis.overlay`,
//! `gizmo.backend`) are deliberately **not** read here: they say which backend draws,
//! not what the user chose, and belong to the script half's provider registry.

use ferrocad_shell_api::settings::{SettingKind, SettingValue, SettingsBackend};
use ferrocad_shell_api::view::{AxisRequest, GizmoRequest, ViewRequest, ViewState, srgb_hex};

use crate::grid_settings;

/// The axis cross's native parameter group.
pub const AXIS_GROUP: &str = "Mods/axis/ferrocad";
/// The transform gizmo's native parameter group.
pub const GIZMO_GROUP: &str = "Mods/gizmo/ferrocad";

fn boolean(settings: &dyn SettingsBackend, group: &str, name: &str) -> Option<bool> {
    settings
        .get(group, name, SettingKind::Bool)
        .and_then(|value| value.as_bool())
}

fn number(settings: &dyn SettingsBackend, group: &str, name: &str) -> Option<f64> {
    settings
        .get(group, name, SettingKind::Float)
        .and_then(|value| value.as_f64())
}

fn text(settings: &dyn SettingsBackend, group: &str, name: &str) -> Option<String> {
    settings
        .get(group, name, SettingKind::String)
        .and_then(|value| value.as_str().map(str::to_string))
}

fn axis_request(settings: &dyn SettingsBackend) -> AxisRequest {
    AxisRequest {
        enabled: boolean(settings, AXIS_GROUP, "enabled"),
        color_x: text(settings, AXIS_GROUP, "color_x"),
        color_y: text(settings, AXIS_GROUP, "color_y"),
        color_z: text(settings, AXIS_GROUP, "color_z"),
        opacity: number(settings, AXIS_GROUP, "opacity").map(|value| value as f32),
        length: number(settings, AXIS_GROUP, "length"),
        width: number(settings, AXIS_GROUP, "width"),
        origin: boolean(settings, AXIS_GROUP, "origin"),
        labels: boolean(settings, AXIS_GROUP, "labels"),
        ghost_alpha: number(settings, AXIS_GROUP, "ghost_alpha").map(|value| value as f32),
        // `overlay` is a provider claim, not a setting.
        ..Default::default()
    }
}

fn gizmo_request(settings: &dyn SettingsBackend) -> GizmoRequest {
    GizmoRequest {
        enabled: boolean(settings, GIZMO_GROUP, "enabled"),
        active_color: text(settings, GIZMO_GROUP, "active_color"),
        inactive_color: text(settings, GIZMO_GROUP, "inactive_color"),
        outline_color: text(settings, GIZMO_GROUP, "outline_color"),
        size: number(settings, GIZMO_GROUP, "size"),
        width: number(settings, GIZMO_GROUP, "width"),
        halo_width: number(settings, GIZMO_GROUP, "halo_width"),
        hover_scale: number(settings, GIZMO_GROUP, "hover_scale"),
        hit_radius_px: number(settings, GIZMO_GROUP, "hit_radius_px"),
        opacity: number(settings, GIZMO_GROUP, "opacity").map(|value| value as f32),
        ghost_alpha: number(settings, GIZMO_GROUP, "ghost_alpha").map(|value| value as f32),
        ring_snap_deg: number(settings, GIZMO_GROUP, "ring_snap_deg"),
        show_readout: boolean(settings, GIZMO_GROUP, "show_readout"),
        // `backend` is the provider claim, not a setting.
        ..Default::default()
    }
}

/// The appearance request the host's store describes.
///
/// This is what `appearance_view` hands the shell at boot. The grid half is
/// [`grid_settings::request`]; the axis and gizmo halves are the native groups above.
pub fn request(settings: &dyn SettingsBackend) -> ViewRequest {
    let mut request = grid_settings::request(settings);
    request.axis = Some(axis_request(settings));
    request.gizmo = Some(gizmo_request(settings));
    request
}

/// Hydrate a presentation state from every persisted appearance group.
pub fn load(settings: &dyn SettingsBackend, view: &ViewState) -> ViewState {
    let mut hydrated = view.clone();
    let _ = hydrated.apply(&request(settings));
    hydrated
}

/// Persist the appearance groups the shell just applied.
///
/// The keys are the schemas `FerroCAD.Grid`, `FerroCAD.Axis` and `FerroCAD.Gizmo`
/// read, so a card edit, a `FerroCAD.*` write and a capture's imported file all land
/// in one store.
pub fn store(settings: &dyn SettingsBackend, view: &ViewState) -> Result<(), String> {
    grid_settings::store(settings, view)?;

    for (name, value) in [
        ("enabled", SettingValue::Bool(view.axis.enabled)),
        (
            "color_x",
            SettingValue::String(srgb_hex(view.axis.colors[0])),
        ),
        (
            "color_y",
            SettingValue::String(srgb_hex(view.axis.colors[1])),
        ),
        (
            "color_z",
            SettingValue::String(srgb_hex(view.axis.colors[2])),
        ),
        ("opacity", SettingValue::Float(f64::from(view.axis.opacity))),
        ("length", SettingValue::Float(view.axis.length)),
        ("width", SettingValue::Float(view.axis.width)),
        ("origin", SettingValue::Bool(view.axis.origin)),
        ("labels", SettingValue::Bool(view.axis.labels)),
        (
            "ghost_alpha",
            SettingValue::Float(f64::from(view.axis.ghost_alpha)),
        ),
    ] {
        settings.set(AXIS_GROUP, name, value)?;
    }

    for (name, value) in [
        ("enabled", SettingValue::Bool(view.gizmo.enabled)),
        (
            "active_color",
            SettingValue::String(srgb_hex(view.gizmo.active)),
        ),
        (
            "inactive_color",
            SettingValue::String(srgb_hex(view.gizmo.inactive)),
        ),
        (
            "outline_color",
            SettingValue::String(srgb_hex(view.gizmo.outline)),
        ),
        ("size", SettingValue::Float(view.gizmo.size)),
        ("width", SettingValue::Float(view.gizmo.width)),
        ("halo_width", SettingValue::Float(view.gizmo.halo_width)),
        ("hover_scale", SettingValue::Float(view.gizmo.hover_scale)),
        (
            "hit_radius_px",
            SettingValue::Float(view.gizmo.hit_radius_px),
        ),
        (
            "opacity",
            SettingValue::Float(f64::from(view.gizmo.opacity)),
        ),
        (
            "ghost_alpha",
            SettingValue::Float(f64::from(view.gizmo.ghost_alpha)),
        ),
        (
            "ring_snap_deg",
            SettingValue::Float(view.gizmo.ring_snap_deg),
        ),
        ("show_readout", SettingValue::Bool(view.gizmo.show_readout)),
    ] {
        settings.set(GIZMO_GROUP, name, value)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use ferrocad_shell_api::settings::MemorySettings;

    use super::*;

    #[test]
    fn the_request_carries_the_stored_axis_and_gizmo() {
        let settings = MemorySettings::default();
        settings.set(AXIS_GROUP, "enabled", true.into()).unwrap();
        settings.set(AXIS_GROUP, "length", 400.0.into()).unwrap();
        settings.set(AXIS_GROUP, "width", 6.0.into()).unwrap();
        settings
            .set(AXIS_GROUP, "color_x", "#f08080".into())
            .unwrap();
        settings.set(GIZMO_GROUP, "size", 120.0.into()).unwrap();
        settings
            .set(GIZMO_GROUP, "show_readout", false.into())
            .unwrap();

        let request = request(&settings);
        let axis = request.axis.expect("the axis request");
        assert_eq!(axis.enabled, Some(true));
        assert_eq!(axis.length, Some(400.0));
        assert_eq!(axis.width, Some(6.0));
        assert_eq!(axis.color_x.as_deref(), Some("#f08080"));
        assert_eq!(axis.overlay, None, "the provider claim is not a setting");

        let gizmo = request.gizmo.expect("the gizmo request");
        assert_eq!(gizmo.size, Some(120.0));
        assert_eq!(gizmo.show_readout, Some(false));
        assert_eq!(gizmo.backend, None, "the provider claim is not a setting");
    }

    #[test]
    fn an_empty_store_leaves_the_request_empty() {
        let settings = MemorySettings::default();
        let request = request(&settings);
        assert_eq!(request.axis, Some(AxisRequest::default()));
        assert_eq!(request.gizmo, Some(GizmoRequest::default()));
        assert_eq!(request.grid, None);
    }

    #[test]
    fn the_applied_state_is_stored_under_every_group_key() {
        let settings = MemorySettings::default();
        let mut view = ViewState::default();
        view.axis.enabled = false;
        view.axis.length = 400.0;
        view.axis.width = 6.0;
        view.gizmo.size = 120.0;
        view.gizmo.show_readout = false;

        store(&settings, &view).unwrap();

        assert_eq!(
            settings.get(AXIS_GROUP, "enabled", SettingKind::Bool),
            Some(SettingValue::Bool(false))
        );
        assert_eq!(
            settings.get(AXIS_GROUP, "length", SettingKind::Float),
            Some(SettingValue::Float(400.0))
        );
        assert_eq!(
            settings.get(GIZMO_GROUP, "size", SettingKind::Float),
            Some(SettingValue::Float(120.0))
        );
        assert_eq!(
            settings.get(GIZMO_GROUP, "show_readout", SettingKind::Bool),
            Some(SettingValue::Bool(false))
        );
        // The grid half is still written by `grid_settings::store`.
        assert_eq!(
            settings.get(grid_settings::GROUP, "enabled", SettingKind::Bool),
            Some(SettingValue::Bool(view.grid))
        );
    }
}
