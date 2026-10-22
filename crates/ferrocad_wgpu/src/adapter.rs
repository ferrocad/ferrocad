//! Adapter selection for the viewport renderer.
//!
//! The renderer's default path asks wgpu for whichever adapter it prefers. This
//! module adds the persisted choice the Dev Tools GPU panel writes: an explicit
//! backend, or an adapter named by a substring of [`wgpu::AdapterInfo::name`]. It
//! also enumerates the adapters so the panel can list them.
//!
//! Zero-copy presentation is **outside** this choice: `gpui` builds the
//! presenter's renderer on the window compositor's adapter, so while zero-copy is
//! active that adapter wins. See `crates/ferrocad_gpui/src/interop.rs`.

use crate::renderer::block_on;

/// Which adapter the viewport renderer should use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterChoice {
    /// Let wgpu pick (the long-standing behaviour).
    Auto,
    /// Restrict the instance to one backend and ask it for an adapter.
    Backend(wgpu::Backends),
    /// Pick an adapter whose name contains this substring (case-insensitive).
    Name(String),
}

impl AdapterChoice {
    /// Parse the persisted grammar: `"auto"`, `"backend:vulkan|gl|metal|dx12"`,
    /// or `"adapter:<substring>"`.
    ///
    /// Surrounding whitespace is ignored, the keywords and prefixes are
    /// case-insensitive, and an adapter name keeps its case. Anything else is an
    /// error naming the accepted forms.
    pub fn parse(value: &str) -> Result<Self, String> {
        let trimmed = value.trim();
        let lowered = trimmed.to_ascii_lowercase();
        if lowered == "auto" {
            return Ok(Self::Auto);
        }
        if let Some(rest) = lowered.strip_prefix("backend:") {
            let backend = match rest.trim() {
                "vulkan" => wgpu::Backends::VULKAN,
                "gl" => wgpu::Backends::GL,
                "metal" => wgpu::Backends::METAL,
                "dx12" => wgpu::Backends::DX12,
                _ => return Err(invalid(trimmed)),
            };
            return Ok(Self::Backend(backend));
        }
        if lowered.starts_with("adapter:") {
            let name = trimmed["adapter:".len()..].trim();
            if name.is_empty() {
                return Err(invalid(trimmed));
            }
            return Ok(Self::Name(name.to_string()));
        }
        Err(invalid(trimmed))
    }

    /// The canonical spelling to persist.
    pub fn as_setting(&self) -> String {
        match self {
            Self::Auto => "auto".to_string(),
            Self::Backend(backends) => format!("backend:{}", backend_name(*backends)),
            Self::Name(name) => format!("adapter:{name}"),
        }
    }
}

/// One enumerated adapter, for the Dev Tools panel.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdapterSummary {
    /// The adapter's name (`AdapterInfo::name`).
    pub name: String,
    /// The wgpu backend (`vulkan`, `gl`, `metal`, `dx12`, …).
    pub backend: String,
    /// The device type: `discrete`, `integrated`, `virtual`, `cpu` or `other`.
    pub device_type: String,
    /// The driver name, or its info string when the name is empty.
    pub driver: String,
}

impl From<wgpu::AdapterInfo> for AdapterSummary {
    fn from(info: wgpu::AdapterInfo) -> Self {
        Self {
            name: info.name,
            backend: info.backend.to_string(),
            device_type: device_type(&info.device_type).to_string(),
            driver: if info.driver.is_empty() {
                info.driver_info
            } else {
                info.driver
            },
        }
    }
}

/// Enumerate what this machine offers (no device created; shared with the panel).
///
/// This uses the same default instance [`AdapterChoice::Auto`] would, so the
/// panel lists the adapters the renderer can actually see.
pub fn adapters() -> Vec<AdapterSummary> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    block_on(instance.enumerate_adapters(wgpu::Backends::all()))
        .into_iter()
        .map(|adapter| AdapterSummary::from(adapter.get_info()))
        .collect()
}

/// Build an instance honouring the choice and resolve its adapter.
pub(crate) fn request(choice: &AdapterChoice) -> Result<wgpu::Adapter, String> {
    match choice {
        // Keep the default path byte-identical: default instance, default options.
        AdapterChoice::Auto => {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .map_err(|_| "no suitable GPU adapter found".to_string())
        }
        AdapterChoice::Backend(backends) => {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: *backends,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).map_err(
                |_| format!("no GPU adapter on backend {}", backend_name(*backends)),
            )
        }
        AdapterChoice::Name(needle) => {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let candidates = block_on(instance.enumerate_adapters(wgpu::Backends::all()));
            let wanted = needle.to_ascii_lowercase();
            let available: Vec<String> = candidates
                .iter()
                .map(|adapter| adapter.get_info().name)
                .collect();

            // Prefer a discrete GPU, then an integrated one; otherwise keep the
            // enumeration order (the first match).
            let mut best: Option<(u8, wgpu::Adapter)> = None;
            for adapter in candidates {
                let info = adapter.get_info();
                if !info.name.to_ascii_lowercase().contains(&wanted) {
                    continue;
                }
                let rank = match info.device_type {
                    wgpu::DeviceType::DiscreteGpu => 2,
                    wgpu::DeviceType::IntegratedGpu => 1,
                    _ => 0,
                };
                if best
                    .as_ref()
                    .is_none_or(|(best_rank, _)| rank > *best_rank)
                {
                    best = Some((rank, adapter));
                }
            }

            best.map(|(_, adapter)| adapter).ok_or_else(|| {
                let available = if available.is_empty() {
                    "none".to_string()
                } else {
                    available.join(", ")
                };
                format!("no adapter matching {needle:?}; available: {available}")
            })
        }
    }
}

/// The grammar an error points at.
fn invalid(value: &str) -> String {
    format!(
        "unrecognised GPU adapter setting {value:?}; use \"auto\", \
         \"backend:vulkan|gl|metal|dx12\", or \"adapter:<name substring>\""
    )
}

/// A single-backend choice's canonical name.
fn backend_name(backends: wgpu::Backends) -> &'static str {
    if backends.contains(wgpu::Backends::VULKAN) {
        "vulkan"
    } else if backends.contains(wgpu::Backends::GL) {
        "gl"
    } else if backends.contains(wgpu::Backends::METAL) {
        "metal"
    } else if backends.contains(wgpu::Backends::DX12) {
        "dx12"
    } else {
        "unknown"
    }
}

fn device_type(device_type: &wgpu::DeviceType) -> &'static str {
    match device_type {
        wgpu::DeviceType::DiscreteGpu => "discrete",
        wgpu::DeviceType::IntegratedGpu => "integrated",
        wgpu::DeviceType::VirtualGpu => "virtual",
        wgpu::DeviceType::Cpu => "cpu",
        wgpu::DeviceType::Other => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_is_the_default_spelling() {
        assert_eq!(AdapterChoice::parse("auto"), Ok(AdapterChoice::Auto));
        assert_eq!(AdapterChoice::parse("  AUTO "), Ok(AdapterChoice::Auto));
        assert_eq!(AdapterChoice::Auto.as_setting(), "auto");
    }

    #[test]
    fn a_backend_choice_is_case_insensitive() {
        assert_eq!(
            AdapterChoice::parse("backend:gl"),
            Ok(AdapterChoice::Backend(wgpu::Backends::GL))
        );
        assert_eq!(
            AdapterChoice::parse(" BACKEND:Vulkan "),
            Ok(AdapterChoice::Backend(wgpu::Backends::VULKAN))
        );
        assert_eq!(
            AdapterChoice::parse("backend:metal"),
            Ok(AdapterChoice::Backend(wgpu::Backends::METAL))
        );
        assert_eq!(
            AdapterChoice::parse("backend:dx12"),
            Ok(AdapterChoice::Backend(wgpu::Backends::DX12))
        );
        assert_eq!(
            AdapterChoice::Backend(wgpu::Backends::GL).as_setting(),
            "backend:gl"
        );
    }

    #[test]
    fn an_adapter_name_is_trimmed_but_keeps_its_case() {
        assert_eq!(
            AdapterChoice::parse("adapter: Llvm"),
            Ok(AdapterChoice::Name("Llvm".to_string()))
        );
        assert_eq!(
            AdapterChoice::parse("ADAPTER: llvmpipe"),
            Ok(AdapterChoice::Name("llvmpipe".to_string()))
        );
        assert_eq!(
            AdapterChoice::Name("Llvm".to_string()).as_setting(),
            "adapter:Llvm"
        );
    }

    #[test]
    fn unknown_forms_name_the_accepted_grammar() {
        for value in [
            "",
            "gpu",
            "backend:",
            "backend:swiftshader",
            "adapter:",
            "adapter:   ",
        ] {
            let error = AdapterChoice::parse(value).expect_err(value);
            assert!(error.contains("auto"), "{error}");
            assert!(error.contains("backend:"), "{error}");
            assert!(error.contains("adapter:"), "{error}");
        }
    }

    #[test]
    fn parsing_then_spelling_round_trips() {
        for value in ["auto", "backend:vulkan", "backend:gl", "adapter:Llvm"] {
            let choice = AdapterChoice::parse(value).expect(value);
            assert_eq!(choice.as_setting(), value);
        }
    }
}
