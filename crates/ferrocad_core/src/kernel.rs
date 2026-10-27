//! The process-wide kernel facade.
//!
//! FerroCAD has exactly **one kernel per process**, made of the document registry
//! ([`Application`]), the object-type behaviours and the property types (factories
//! and extension restore hooks). [`Kernel`] is a single handle for that state.
//!
//! This is a naming and ownership boundary, not a state migration: the registries
//! stay in the `OnceLock`s they already had, and the free functions
//! ([`crate::application()`], [`crate::object_registry`], [`crate::property_types`])
//! keep working unchanged. New code should reach the kernel state through
//! [`kernel()`] so there is one obvious place to look.
//!
//! A workbench crate must **not** introduce its own `OnceLock<…Registry>` for
//! documents, object types or property types. That would be a second registry:
//! invisible to this facade and to every other module, and a split image in all but
//! name. Register through [`kernel()`] (or the existing free functions, which reach
//! the same state). Workbench *singletons* such as the Part geometry backend are a
//! different scope and stay in their crate; see
//! `docs/developer/architecture/kernel-state-sharing.md` §2.
//!
//! ```
//! use ferrocad_core::{application, kernel};
//!
//! // Create through the free function, read through the facade: same registry.
//! let (name, handle) = application()
//!     .lock()
//!     .unwrap()
//!     .new_document(Some("KernelFacadeDoc"));
//! let visible = kernel().documents().lock().unwrap().get(&name).unwrap();
//! assert!(std::sync::Arc::ptr_eq(&handle, &visible));
//! kernel().documents().lock().unwrap().close_document(&name);
//! ```

use std::sync::Mutex;

use crate::application::{application, Application};
use crate::object_registry::{self, ObjectTypeRegistry};
use crate::property_types::{self, PropertyExtensionRegistry, PropertyRegistry};

/// The handle to the process-wide kernel state.
///
/// Every method returns the one registry that already exists for this process;
/// `Kernel` itself holds no state.
#[derive(Debug)]
pub struct Kernel;

/// The one kernel handle (FreeCAD's `App::GetApplication()` equivalent).
///
/// A workbench crate must not create its own `OnceLock<…Registry>` next to this;
/// use the registries reached through this handle instead.
pub fn kernel() -> &'static Kernel {
    static KERNEL: Kernel = Kernel;
    &KERNEL
}

impl Kernel {
    /// The document registry: open documents and the active document
    /// (FreeCAD's `App::Application`).
    ///
    /// This is the same `Mutex<Application>` returned by [`crate::application()`].
    pub fn documents(&self) -> &'static Mutex<Application> {
        application()
    }

    /// The registered object-type behaviours (`"Part::Feature"`, …).
    ///
    /// This is the same registry used by [`crate::object_registry::register`] and
    /// friends.
    pub fn object_types(&self) -> &'static ObjectTypeRegistry {
        object_registry::registry()
    }

    /// The registered property types (`"App::PropertyLength"`, …).
    ///
    /// This is the same registry used by [`crate::property_types::register`] and
    /// friends.
    pub fn properties(&self) -> &'static PropertyRegistry {
        property_types::registry()
    }

    /// The extension restore hooks that pair with [`Kernel::properties`].
    ///
    /// Some property types are module-owned (`Part::PropertyPartShape`) and also
    /// register a restore hook for saved documents; this is that sibling registry,
    /// the same one used by [`crate::property_types::register_extension`].
    pub fn property_extensions(&self) -> &'static PropertyExtensionRegistry {
        property_types::extensions()
    }
}
