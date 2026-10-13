use std::str::FromStr;

use serde::Deserialize;

use super::{
    ModeDegradation, RefusedLayers, ResolvedSandboxMode, SandboxMode, SandboxModeLayers,
    SandboxModeSource,
};
use crate::remote_settings::RemoteSettings;

fn resolve(layers: SandboxModeLayers<'_>) -> (SandboxMode, SandboxModeSource) {
    let resolved = SandboxMode::resolve(layers);
    assert_eq!(None, resolved.degraded, "the layers alone never degrade");
    (resolved.mode, resolved.source)
}

/// The sandbox ships off; observe/enforce are turned on in Settings or in
/// `workspaced.toml`.
#[test]
fn default_is_off_from_the_default_layer() {
    assert_eq!(SandboxMode::Off, SandboxMode::default());
    assert_eq!(
        ResolvedSandboxMode {
            mode: SandboxMode::Off,
            source: SandboxModeSource::Default,
            degraded: None,
        },
        SandboxMode::resolve(SandboxModeLayers::default())
    );
}

/// The rollout switch flips a fleet at once, so its `enforce` on a host with no backend runs
/// as `off`, recorded and attributed to the switch; the same `enforce` from the developer's
/// own layers is kept, and refuses. With a backend nothing changes.
#[test]
fn the_rollout_switch_runs_off_on_a_host_without_a_backend() {
    let remote_enforce = SandboxMode::resolve(SandboxModeLayers {
        remote: Some(SandboxMode::Enforce),
        ..SandboxModeLayers::default()
    });
    assert_eq!(
        ResolvedSandboxMode {
            mode: SandboxMode::Off,
            source: SandboxModeSource::Remote,
            degraded: Some(ModeDegradation::EnforceWithoutBackend),
        },
        remote_enforce.on_host(false)
    );
    assert_eq!(remote_enforce, remote_enforce.on_host(true));

    for layers in [
        SandboxModeLayers {
            env: Some("enforce"),
            ..SandboxModeLayers::default()
        },
        SandboxModeLayers {
            user: Some(SandboxMode::Enforce),
            ..SandboxModeLayers::default()
        },
        SandboxModeLayers {
            workspace: Some(SandboxMode::Enforce),
            ..SandboxModeLayers::default()
        },
        // The developer's env `enforce` over a remote `observe`: theirs, so it stays
        SandboxModeLayers {
            env: Some("enforce"),
            remote: Some(SandboxMode::Observe),
            ..SandboxModeLayers::default()
        },
    ] {
        let resolved = SandboxMode::resolve(layers);
        assert_eq!(SandboxMode::Enforce, resolved.mode, "{layers:?}");
        assert_eq!(resolved, resolved.on_host(false), "{layers:?}");
    }
    // A remote `observe` or `off` has nothing to degrade
    for mode in [SandboxMode::Observe, SandboxMode::Off] {
        let resolved = SandboxMode::resolve(SandboxModeLayers {
            remote: Some(mode),
            ..SandboxModeLayers::default()
        });
        assert_eq!(resolved, resolved.on_host(false), "{mode:?}");
    }
    // The user's or folder's own `enforce` beneath the switch's still refuses there; the switch's
    // alone, or over a lower or refused layer, runs `off`
    let switch_off = ResolvedSandboxMode {
        mode: SandboxMode::Off,
        source: SandboxModeSource::Remote,
        degraded: Some(ModeDegradation::EnforceWithoutBackend),
    };
    let own = |source| ResolvedSandboxMode::new(SandboxMode::Enforce, source);
    let under_switch = |user, workspace, refused| SandboxModeLayers {
        remote: Some(SandboxMode::Enforce),
        user,
        workspace,
        refused,
        ..SandboxModeLayers::default()
    };
    let refused_user = RefusedLayers {
        user: true,
        ..RefusedLayers::default()
    };
    let enforce = Some(SandboxMode::Enforce);
    for (layers, expected) in [
        (
            under_switch(enforce, None, RefusedLayers::default()),
            own(SandboxModeSource::UserConfig),
        ),
        (
            under_switch(Some(SandboxMode::Off), enforce, RefusedLayers::default()),
            own(SandboxModeSource::WorkspaceConfig),
        ),
        (
            under_switch(None, None, RefusedLayers::default()),
            switch_off,
        ),
        (
            under_switch(Some(SandboxMode::Observe), None, RefusedLayers::default()),
            switch_off,
        ),
        (under_switch(enforce, None, refused_user), switch_off),
    ] {
        assert_eq!(
            expected,
            SandboxMode::resolve_on_host(layers, false),
            "{layers:?}"
        );
        assert_eq!(
            own(SandboxModeSource::Remote),
            SandboxMode::resolve_on_host(layers, true),
            "{layers:?}"
        );
    }
}

/// `sandbox.status` carries the degradation only when there is one, so the shape of every
/// other answer is unchanged.
#[test]
fn a_degradation_is_serialized_only_when_present() {
    let plain = ResolvedSandboxMode::new(SandboxMode::Enforce, SandboxModeSource::Remote);
    assert_eq!(
        serde_json::json!({"mode": "enforce", "source": "remote"}),
        serde_json::to_value(plain).unwrap()
    );
    assert_eq!(
        serde_json::json!({
            "mode": "off",
            "source": "remote",
            "degraded": "enforce_without_backend",
        }),
        serde_json::to_value(plain.on_host(false)).unwrap()
    );
    assert_eq!(
        "enforce_without_backend",
        <&str>::from(ModeDegradation::EnforceWithoutBackend)
    );
}

/// `Enforce`, `ENFORCE` and ` enforce ` all name the mode, from the environment as from a file
/// or the remote settings; a word that is not a mode is still refused, whatever its case.
#[test]
fn modes_are_parsed_in_any_case_from_every_source() {
    for (text, mode) in [
        ("Enforce", SandboxMode::Enforce),
        ("ENFORCE", SandboxMode::Enforce),
        ("observe", SandboxMode::Observe),
        ("Observe", SandboxMode::Observe),
        ("OFF", SandboxMode::Off),
    ] {
        assert_eq!(Ok(mode), SandboxMode::from_str(text), "{text}");
        assert_eq!(
            mode,
            serde_json::from_str::<SandboxMode>(&format!("\" {text} \"")).unwrap(),
            "{text}"
        );
        assert_eq!(
            (mode, SandboxModeSource::Env),
            resolve(SandboxModeLayers {
                env: Some(text),
                ..SandboxModeLayers::default()
            }),
            "{text}"
        );
    }
    assert!(SandboxMode::from_str("Audit").is_err());
    let error = serde_json::from_str::<SandboxMode>("\"Audit\"").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("expected off, observe or enforce"),
        "{error}"
    );
    assert!(
        !error.to_string().contains("Audit"),
        "the value is never echoed: {error}"
    );
    assert!(serde_json::from_str::<SandboxMode>("3").is_err());
}

/// The CLI parses `/v1/settings` with `serde_json::from_slice` (reqwest's `Response::json`) and
/// its cache with `serde_json::from_str`: whatever `sandbox_mode` holds, the rest still loads.
#[test]
fn a_remote_sandbox_mode_of_any_shape_never_fails_the_settings() {
    for (value, mode) in [
        (None, None),
        (Some(r#""enforce""#), Some(SandboxMode::Enforce)),
        (Some(r#"" Observe ""#), Some(SandboxMode::Observe)),
        (Some(r#""bogus""#), None),
        (Some("null"), None),
        (Some("3"), None),
        (Some("true"), None),
        (Some("18446744073709551615"), None),
        (Some("[null]"), None),
        (Some(r#"{"a":null}"#), None),
        (Some(r#"{"$__toml_private_datetime":"x"}"#), None),
        (Some(r#"{"a":1,"a":2}"#), None),
    ] {
        let field = value.map(|v| format!(r#""sandbox_mode":{v},"#));
        let body = format!(
            r#"{{"leader_mode":true,{}"release_channel":"alpha","loc_tracking":true}}"#,
            field.unwrap_or_default()
        );
        for parsed in [
            serde_json::from_slice::<RemoteSettings>(body.as_bytes()),
            serde_json::from_str::<RemoteSettings>(&body),
        ] {
            let settings = parsed.unwrap_or_else(|error| panic!("{body}: {error}"));
            assert_eq!(mode, settings.sandbox_mode, "{body}");
            assert_eq!(Some(true), settings.leader_mode, "{body}");
            assert_eq!(Some("alpha"), settings.release_channel.as_deref(), "{body}");
            assert_eq!(Some(true), settings.loc_tracking, "{body}");
        }
    }
}

/// A `workspaced.toml` layer reads the mode through the same function: a string names it, any
/// other TOML value (a datetime included) is an unset field, and the table still loads.
#[test]
fn a_file_sandbox_mode_of_any_shape_never_fails_the_table() {
    #[derive(Deserialize)]
    struct Layer {
        #[serde(default, deserialize_with = "super::optional_sandbox_mode")]
        mode: Option<SandboxMode>,
        other: bool,
    }
    for (value, mode) in [
        (r#""Observe""#, Some(SandboxMode::Observe)),
        (r#""audit""#, None),
        ("3", None),
        ("1979-05-27T07:32:00Z", None),
        (r#"["enforce"]"#, None),
        ("{ a = 1 }", None),
    ] {
        let text = format!("mode = {value}\nother = true\n");
        let layer =
            toml::from_str::<Layer>(&text).unwrap_or_else(|error| panic!("{text}: {error}"));
        assert_eq!(mode, layer.mode, "{text}");
        assert!(layer.other, "{text}");
    }
}

#[test]
fn user_config_beats_the_default() {
    assert_eq!(
        (SandboxMode::Enforce, SandboxModeSource::UserConfig),
        resolve(SandboxModeLayers {
            user: Some(SandboxMode::Enforce),
            ..SandboxModeLayers::default()
        })
    );
}

#[test]
fn remote_beats_user_config_as_the_rollout_switch() {
    assert_eq!(
        (SandboxMode::Observe, SandboxModeSource::Remote),
        resolve(SandboxModeLayers {
            remote: Some(SandboxMode::Observe),
            user: Some(SandboxMode::Enforce),
            ..SandboxModeLayers::default()
        })
    );
}

/// A cloned repository's `.grok/workspaced.toml` cannot lower the user's
/// mode.
#[test]
fn workspace_config_may_only_tighten_the_user_mode() {
    let lowered = resolve(SandboxModeLayers {
        workspace: Some(SandboxMode::Off),
        user: Some(SandboxMode::Enforce),
        ..SandboxModeLayers::default()
    });
    assert_eq!(
        (SandboxMode::Enforce, SandboxModeSource::UserConfig),
        lowered
    );

    let raised = resolve(SandboxModeLayers {
        workspace: Some(SandboxMode::Enforce),
        user: Some(SandboxMode::Observe),
        ..SandboxModeLayers::default()
    });
    assert_eq!(
        (SandboxMode::Enforce, SandboxModeSource::WorkspaceConfig),
        raised
    );

    let raised_over_default = resolve(SandboxModeLayers {
        workspace: Some(SandboxMode::Enforce),
        ..SandboxModeLayers::default()
    });
    assert_eq!(
        (SandboxMode::Enforce, SandboxModeSource::WorkspaceConfig),
        raised_over_default
    );

    let equal = resolve(SandboxModeLayers {
        workspace: Some(SandboxMode::Observe),
        user: Some(SandboxMode::Observe),
        ..SandboxModeLayers::default()
    });
    assert_eq!((SandboxMode::Observe, SandboxModeSource::UserConfig), equal);
}

/// A folder whose file says `off` with nothing above it is set by that
/// file, not by the default; an equal user value keeps the credit.
#[test]
fn an_explicit_workspace_off_is_attributed_to_the_workspace_file() {
    assert_eq!(
        (SandboxMode::Off, SandboxModeSource::WorkspaceConfig),
        resolve(SandboxModeLayers {
            workspace: Some(SandboxMode::Off),
            ..SandboxModeLayers::default()
        })
    );
    assert_eq!(
        (SandboxMode::Off, SandboxModeSource::UserConfig),
        resolve(SandboxModeLayers {
            user: Some(SandboxMode::Off),
            workspace: Some(SandboxMode::Off),
            ..SandboxModeLayers::default()
        })
    );
}

/// The workspace layer is a tighten-only layer, so it takes no trust input: the layers carry
/// nothing about the folder's trust, and a workspace `enforce` over the default is `enforce`
/// whatever the folder's trust standing. (Gated on trust, a repository's own `.envrc` — which
/// makes the folder untrusted — would drop the mode the folder asked for.)
#[test]
fn the_workspace_layer_tightens_without_a_trust_input() {
    let layers = SandboxModeLayers {
        workspace: Some(SandboxMode::Enforce),
        ..SandboxModeLayers::default()
    };
    let SandboxModeLayers {
        env: None,
        remote: None,
        user: None,
        workspace: Some(SandboxMode::Enforce),
        refused:
            RefusedLayers {
                remote: false,
                user: false,
                workspace: false,
            },
    } = layers
    else {
        panic!(
            "the layers are the four mode sources and which files were refused, nothing else: \
             {layers:?}"
        );
    };
    assert_eq!(
        (SandboxMode::Enforce, SandboxModeSource::WorkspaceConfig),
        resolve(layers)
    );
}

#[test]
fn workspace_config_never_overrides_env_or_remote() {
    assert_eq!(
        (SandboxMode::Observe, SandboxModeSource::Remote),
        resolve(SandboxModeLayers {
            remote: Some(SandboxMode::Observe),
            workspace: Some(SandboxMode::Enforce),
            ..SandboxModeLayers::default()
        })
    );
    assert_eq!(
        (SandboxMode::Off, SandboxModeSource::Env),
        resolve(SandboxModeLayers {
            env: Some("off"),
            workspace: Some(SandboxMode::Enforce),
            ..SandboxModeLayers::default()
        })
    );
}

#[test]
fn env_beats_every_layer() {
    assert_eq!(
        (SandboxMode::Enforce, SandboxModeSource::Env),
        resolve(SandboxModeLayers {
            env: Some(" enforce\n"),
            remote: Some(SandboxMode::Off),
            workspace: Some(SandboxMode::Off),
            user: Some(SandboxMode::Off),
            refused: RefusedLayers {
                remote: false,
                user: true,
                workspace: true,
            },
        })
    );
}

/// A refused file layer counts as `enforce` whatever it holds and is marked when it decides (over
/// the user's file, the default, as the tightening folder layer), never over a higher layer; with
/// no backend the readable layers' mode stands, still marked.
#[test]
fn a_refused_layer_counts_as_enforce_and_is_marked_when_it_decides() {
    let refused = |mode, source| ResolvedSandboxMode {
        mode,
        source,
        degraded: Some(ModeDegradation::ConfigRefused),
    };
    let user_layers = SandboxModeLayers {
        user: Some(SandboxMode::Off),
        refused: RefusedLayers {
            remote: false,
            user: true,
            workspace: false,
        },
        ..SandboxModeLayers::default()
    };
    let user_refused = SandboxMode::resolve(user_layers);
    assert_eq!(
        refused(SandboxMode::Enforce, SandboxModeSource::UserConfig),
        user_refused
    );
    let workspace_layers = SandboxModeLayers {
        user: Some(SandboxMode::Observe),
        workspace: Some(SandboxMode::Off),
        refused: RefusedLayers {
            remote: false,
            user: false,
            workspace: true,
        },
        ..SandboxModeLayers::default()
    };
    let workspace_refused = SandboxMode::resolve(workspace_layers);
    assert_eq!(
        refused(SandboxMode::Enforce, SandboxModeSource::WorkspaceConfig),
        workspace_refused
    );
    assert_eq!(
        ResolvedSandboxMode::new(SandboxMode::Enforce, SandboxModeSource::UserConfig),
        SandboxMode::resolve(SandboxModeLayers {
            user: Some(SandboxMode::Enforce),
            refused: RefusedLayers {
                remote: false,
                user: false,
                workspace: true,
            },
            ..SandboxModeLayers::default()
        }),
        "the user's own enforce decided"
    );
    assert_eq!(
        ResolvedSandboxMode::new(SandboxMode::Observe, SandboxModeSource::Remote),
        SandboxMode::resolve(SandboxModeLayers {
            remote: Some(SandboxMode::Observe),
            refused: RefusedLayers {
                remote: false,
                user: true,
                workspace: true,
            },
            ..SandboxModeLayers::default()
        })
    );

    for (layers, readable) in [
        (user_layers, SandboxMode::Off),
        (workspace_layers, SandboxMode::Observe),
    ] {
        let resolved = SandboxMode::resolve(layers);
        assert_eq!(resolved, SandboxMode::resolve_on_host(layers, true));
        assert_eq!(resolved, resolved.on_host(true));
        assert_eq!(
            refused(readable, resolved.source),
            SandboxMode::resolve_on_host(layers, false)
        );
        assert_eq!(
            refused(SandboxMode::Off, resolved.source),
            resolved.on_host(false)
        );
    }
    assert_eq!(
        serde_json::json!({
            "mode": "enforce",
            "source": "user_config",
            "degraded": "config_refused",
        }),
        serde_json::to_value(user_refused).unwrap(),
        "`sandbox.status` says why"
    );
}

/// Over every pairing of absent, readable and refused files a refusal never lowers a readable
/// layer's mode and is marked (at the highest refused layer) only when it raised it; a readable
/// switch decides over the files below it, yielding its no-backend `off` to their own `enforce`.
#[test]
fn a_refusal_never_wins_a_tie_or_lowers_a_readable_layer() {
    let refused_user_beside_workspace_enforce = SandboxMode::resolve(SandboxModeLayers {
        user: Some(SandboxMode::Off),
        workspace: Some(SandboxMode::Enforce),
        refused: RefusedLayers {
            remote: false,
            user: true,
            workspace: false,
        },
        ..SandboxModeLayers::default()
    });
    let refused_workspace_beside_user_enforce = SandboxMode::resolve(SandboxModeLayers {
        user: Some(SandboxMode::Enforce),
        workspace: Some(SandboxMode::Off),
        refused: RefusedLayers {
            remote: false,
            user: false,
            workspace: true,
        },
        ..SandboxModeLayers::default()
    });
    for backend_available in [true, false] {
        assert_eq!(
            ResolvedSandboxMode::new(SandboxMode::Enforce, SandboxModeSource::WorkspaceConfig),
            refused_user_beside_workspace_enforce.on_host(backend_available),
            "backend_available = {backend_available}"
        );
        assert_eq!(
            ResolvedSandboxMode::new(SandboxMode::Enforce, SandboxModeSource::UserConfig),
            refused_workspace_beside_user_enforce.on_host(backend_available),
            "backend_available = {backend_available}"
        );
    }

    let values = [
        None,
        Some(SandboxMode::Off),
        Some(SandboxMode::Observe),
        Some(SandboxMode::Enforce),
    ];
    let files: Vec<(Option<SandboxMode>, bool)> = values
        .into_iter()
        .flat_map(|value| [(value, false), (value, true)])
        .collect();
    for &(remote, remote_refused) in &files {
        for &(user, user_refused) in &files {
            for &(workspace, workspace_refused) in &files {
                let layers = SandboxModeLayers {
                    remote,
                    user,
                    workspace,
                    refused: RefusedLayers {
                        remote: remote_refused,
                        user: user_refused,
                        workspace: workspace_refused,
                    },
                    ..SandboxModeLayers::default()
                };
                let readable = SandboxMode::resolve(SandboxModeLayers {
                    remote: remote.filter(|_| !remote_refused),
                    user: user.filter(|_| !user_refused),
                    workspace: workspace.filter(|_| !workspace_refused),
                    ..SandboxModeLayers::default()
                });
                let resolved = SandboxMode::resolve(layers);
                let applies = (remote_refused || user_refused || workspace_refused)
                    && readable.source != SandboxModeSource::Remote;
                assert_eq!(
                    applies && readable.mode != SandboxMode::Enforce,
                    resolved.degraded == Some(ModeDegradation::ConfigRefused),
                    "marked only when the refusal raised the mode: {layers:?}"
                );
                if resolved.degraded == Some(ModeDegradation::ConfigRefused) {
                    let highest = if remote_refused {
                        SandboxModeSource::Remote
                    } else if user_refused {
                        SandboxModeSource::UserConfig
                    } else {
                        SandboxModeSource::WorkspaceConfig
                    };
                    assert_eq!(highest, resolved.source, "{layers:?}");
                }
                let own_enforce = [(user, user_refused), (workspace, workspace_refused)]
                    .contains(&(Some(SandboxMode::Enforce), false));
                let switch_enforce = readable
                    == ResolvedSandboxMode::new(SandboxMode::Enforce, SandboxModeSource::Remote);
                for backend_available in [true, false] {
                    let resolved = SandboxMode::resolve_on_host(layers, backend_available);
                    let readable = readable.on_host(backend_available);
                    // The switch's `enforce` yields to the user's or folder's own, never to off
                    let expected =
                        if (applies && backend_available) || (switch_enforce && own_enforce) {
                            SandboxMode::Enforce
                        } else {
                            readable.mode
                        };
                    assert_eq!(
                        expected, resolved.mode,
                        "{resolved:?} fails open, lowers {readable:?} or runs a mode no one \
                         chose: {layers:?}, backend_available = {backend_available}"
                    );
                    assert_eq!(
                        applies && readable.mode != SandboxMode::Enforce,
                        resolved.degraded == Some(ModeDegradation::ConfigRefused),
                        "{layers:?}, backend_available = {backend_available}"
                    );
                }
            }
        }
    }
}

#[test]
fn unrecognised_env_is_skipped_not_treated_as_off() {
    assert_eq!(
        (SandboxMode::Enforce, SandboxModeSource::UserConfig),
        resolve(SandboxModeLayers {
            env: Some("enforcee"),
            user: Some(SandboxMode::Enforce),
            ..SandboxModeLayers::default()
        })
    );
}

#[test]
fn modes_are_ordered_off_observe_enforce() {
    assert!(SandboxMode::Off < SandboxMode::Observe);
    assert!(SandboxMode::Observe < SandboxMode::Enforce);
}

#[test]
fn mode_strings_round_trip_through_serde_and_strum() {
    for (mode, text) in [
        (SandboxMode::Off, "off"),
        (SandboxMode::Observe, "observe"),
        (SandboxMode::Enforce, "enforce"),
    ] {
        assert_eq!(text, <&str>::from(mode));
        assert_eq!(
            mode,
            serde_json::from_str::<SandboxMode>(&format!("\"{text}\"")).unwrap()
        );
    }
    assert!(serde_json::from_str::<SandboxMode>("\"audit\"").is_err());
}

#[test]
fn only_enforce_wraps_and_off_never_injects_the_proxy() {
    assert!(SandboxMode::Enforce.is_wrapped());
    assert!(!SandboxMode::Observe.is_wrapped());
    assert!(!SandboxMode::Off.is_wrapped());
    assert!(SandboxMode::Enforce.uses_proxy());
    assert!(SandboxMode::Observe.uses_proxy());
    assert!(!SandboxMode::Off.uses_proxy());
}
