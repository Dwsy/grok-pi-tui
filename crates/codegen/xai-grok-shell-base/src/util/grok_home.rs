// Re-exported from the defining crate so this crate stays off the tool stack.
pub use xai_grok_config::{
    create_dir_all_owner_only, decode_cwd_from_dirname, encode_cwd_dirname,
    ensure_sessions_cwd_dir, ensure_sessions_cwd_dir_in, grok_application, grok_home,
    project_config_dir, project_config_dirname, sessions_cwd_dir, sessions_cwd_dir_in,
    set_dir_owner_only,
};
