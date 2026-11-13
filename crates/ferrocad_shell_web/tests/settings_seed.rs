//! The host settings store reaches the Python facade's parameter store.
//!
//! The browser's `FreeCAD.ParamGet` is memory-only. `RustAppBackend` hydrates its
//! own appearance state from the host store, then seeds the same values into the
//! script half before boot; without that, the facade's `appearance_view` answers
//! with its defaults and overrides an imported settings file.

use ferrocad_shell_api::app::AppBackend;
use ferrocad_shell_api::settings::{MemorySettings, SettingsBackend};
use ferrocad_shell_web::{Runtime, backend_with_settings};

#[test]
fn a_settings_store_seeds_the_python_facade() {
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(|| {
            let settings = MemorySettings::default();
            settings
                .set("Mods/grid/ferrocad", "enabled", true.into())
                .unwrap();
            settings
                .set("Mods/axis/ferrocad", "length", 400.0.into())
                .unwrap();

            let backend =
                backend_with_settings(&[Runtime::Pywasm], Some(Box::new(settings))).unwrap();
            let view = backend.appearance_view().unwrap().view.expect("a request");
            assert_eq!(
                view.grid,
                Some(true),
                "the host store's grid toggle reached the facade"
            );
            assert_eq!(
                view.axis.as_ref().and_then(|axis| axis.length),
                Some(400.0),
                "the host store's axis length reached the facade"
            );
        })
        .expect("a worker thread")
        .join()
        .expect("no panic");
}
