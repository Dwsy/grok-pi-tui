//! Vendor compatibility configuration; `xai-grok-config` owns it.

pub(crate) use xai_grok_config::compat::INSTRUCTION_FILENAMES;
pub use xai_grok_config::compat::{
    COMPAT_CELLS, CompatCell, CompatConfig, CompatConfigToml, CompatRemoteKey, CompatSurface,
    CompatVendor, VendorCompat, VendorCompatToml,
};
