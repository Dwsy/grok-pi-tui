//! The user's `config.toml` writers write through a dotfile-managed symlink; a project link is
//! replaced at the link name.

#![cfg(unix)]

use std::path::Path;

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

#[test]
fn user_config_writers_write_through_a_symlink() {
    // One #[test] per binary: the env is process-global and `grok_home()` is cached.
    let grok_home = tempfile::tempdir().expect("grok home");
    // SAFETY: no other threads are running yet.
    unsafe { std::env::set_var("GROK_HOME", grok_home.path()) };

    let dotfiles = tempfile::tempdir().expect("dotfiles");
    let target = dotfiles.path().join("config.toml");
    std::fs::write(&target, "[models]\ndefault = \"grok-4.6\"\n").unwrap();
    let link = xai_grok_shell::util::config::user_config_path();
    assert_eq!(link, grok_home.path().join("config.toml"));
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let config = xai_grok_shell::util::config::McpServerConfig {
        transport: xai_grok_shell::util::config::McpServerTransportConfig::Stdio {
            command: "/bin/echo".to_string(),
            args: vec!["hi".to_string()],
            env: None,
            cwd: None,
        },
        enabled: true,
        oauth: None,
        setup: None,
        startup_timeout_sec: None,
        tool_timeout_sec: None,
        tool_timeouts: None,
        expose_image_base64: None,
    };

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            xai_grok_shell::util::config::update_config(|cfg| {
                cfg.ui.simple_mode = Some(true);
            })
            .await
            .expect("settings save");
            assert!(is_symlink(&link), "settings save must keep the link");

            xai_grok_shell::util::config::save_mcp_server_config_at(&link, "qa-echo", &config)
                .await
                .expect("mcp add");
            assert!(is_symlink(&link), "mcp add must keep the link");
            let written = std::fs::read_to_string(&target).unwrap();
            assert!(
                written.contains("[mcp_servers.qa-echo]")
                    && written.contains("simple_mode = true")
                    && written.contains("default = \"grok-4.6\""),
                "the link target must hold every edit: {written}"
            );

            assert!(
                xai_grok_shell::util::config::delete_mcp_server_config_at(&link, "qa-echo")
                    .await
                    .expect("mcp remove")
            );
            assert!(is_symlink(&link), "mcp remove must keep the link");
            let written = std::fs::read_to_string(&target).unwrap();
            assert!(
                !written.contains("qa-echo") && written.contains("simple_mode = true"),
                "the link target must lose only the server: {written}"
            );

            xai_grok_shell::claude_import::mark_claude_imported().expect("claude import marker");
            assert!(is_symlink(&link), "the import marker must keep the link");
            let written: toml::Value = toml::from_str(&std::fs::read_to_string(&target).unwrap())
                .expect("target stays valid TOML");
            assert_eq!(
                written["claude_compat"]["imported"].as_bool(),
                Some(true),
                "the link target must carry the import marker"
            );

            // A link a repository committed at `.grok/config.toml` is replaced at the link name, never followed.
            let repo = tempfile::tempdir().expect("repo");
            let outside = repo.path().join("outside.toml");
            let defined = "[mcp_servers.planted]\ncommand = \"/bin/echo\"\n";
            std::fs::write(&outside, defined).unwrap();
            let project_link = repo.path().join(".grok").join("config.toml");
            std::fs::create_dir_all(project_link.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&outside, &project_link).unwrap();
            assert!(
                xai_grok_shell::util::config::delete_mcp_server_config_at(&project_link, "planted")
                    .await
                    .expect("project mcp remove")
            );
            assert!(
                !is_symlink(&project_link),
                "the committed link must be replaced by a regular file"
            );
            assert!(
                !std::fs::read_to_string(&project_link)
                    .unwrap()
                    .contains("planted")
            );
            assert_eq!(
                defined,
                std::fs::read_to_string(&outside).unwrap(),
                "the link target outside the repo must not be written"
            );
        });
}
