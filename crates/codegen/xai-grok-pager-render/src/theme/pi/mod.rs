//! Pi theme JSON → Grok [`Theme`] adapter.
//!
//! Loads themes in the format documented by Pi coding-agent (`themes.md`),
//! resolves `vars`, and maps color tokens onto the Grok pager's semantic
//! palette. Themes are addressed as `pi:<name>` to avoid clashing with
//! Grok built-in aliases (`dark` / `light`).

pub(crate) mod color;
mod load;
mod map;
mod registry;
mod schema;

pub use load::{
    LoadError, load_from_path, load_from_str, load_theme_palette, load_theme_palette_from_str,
};
pub use map::{MapError, map_pi_theme};
pub use registry::{
    DiscoveryReport, PI_THEME_PREFIX, PiThemeMeta, apply_pi_theme, ensure_builtins, init_discovery,
    is_pi_theme_id, list_themes, load_palette, parse_pi_theme_id, rediscover, reset_for_test,
    reset_registry, theme_id,
};
pub use schema::{ColorValue, PiThemeColors, PiThemeJson};
