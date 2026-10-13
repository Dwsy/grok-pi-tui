use super::*;

fn models_default_patch(default: &str) -> toml::Table {
    let mut models = toml::Table::new();
    models.insert(
        "default".to_owned(),
        toml::Value::String(default.to_owned()),
    );
    let mut patch = toml::Table::new();
    patch.insert("models".to_owned(), toml::Value::Table(models));
    patch
}

fn campaign(id: &str, default: &str) -> CampaignEntry {
    CampaignEntry {
        id: id.to_owned(),
        patch: models_default_patch(default),
    }
}

fn overriding(json: &str) -> CampaignEnv {
    CampaignEnv {
        override_json: Some(json.to_owned()),
        ..CampaignEnv::default()
    }
}

fn active_ids(layers: &ConfigLayers, sources: &CampaignSources) -> Vec<String> {
    CampaignOverlay::new(layers, sources)
        .active
        .into_iter()
        .map(|campaign| campaign.id)
        .collect()
}

#[test]
fn override_beats_kill_switch() {
    let layers = ConfigLayers {
        user: toml::from_str("[features]\ncampaigns = false\n").unwrap(),
        ..ConfigLayers::default()
    };
    let cases = [
        (
            overriding(r#"[{"id":"c","models":{"default":"m"}}]"#),
            vec!["c".to_owned()],
        ),
        (CampaignEnv::default(), Vec::new()),
    ];
    for (env, expected) in cases {
        let sources = CampaignSources {
            env,
            ..CampaignSources::default()
        };
        assert_eq!(expected, active_ids(&layers, &sources));
    }
}

#[test]
fn empty_or_invalid_override_suppresses_all_campaigns() {
    let mut layers = ConfigLayers::default();
    layers.campaigns.user = vec![campaign("from-layer", "layer-model")];
    for override_json in ["[]", "{ not json"] {
        let sources = CampaignSources {
            remote: vec![campaign("from-remote", "remote-model")],
            env: overriding(override_json),
            ..CampaignSources::default()
        };
        assert_eq!(
            Vec::<String>::new(),
            active_ids(&layers, &sources),
            "{override_json}"
        );
    }
}

#[test]
fn dismiss_resolution_ignores_kill_switch() {
    let layers = ConfigLayers {
        user: toml::from_str("[features]\ncampaigns = false\n").unwrap(),
        ..ConfigLayers::default()
    };
    let sources = CampaignSources {
        remote: vec![campaign("dismiss-during-kill-switch", "m")],
        env: CampaignEnv {
            is_kill_switch_set: true,
            ..CampaignEnv::default()
        },
        ..CampaignSources::default()
    };

    let resolved: Vec<String> = dismissable_campaigns(Ok(layers), &sources)
        .into_iter()
        .map(|campaign| campaign.id)
        .collect();

    assert_eq!(vec!["dismiss-during-kill-switch".to_owned()], resolved);
}

#[test]
fn remote_campaign_patches_the_effective_config_over_loaded_layers() {
    let layers = ConfigLayers {
        user: toml::from_str("[models]\ndefault = \"user-model\"\n").unwrap(),
        ..ConfigLayers::default()
    };
    let sources = CampaignSources {
        remote: vec![campaign("hook-service-remote-campaign", "remote-model")],
        ..CampaignSources::default()
    };

    let effective = CampaignOverlay::new(&layers, &sources).effective;

    assert_eq!(
        Some("remote-model"),
        effective
            .get("models")
            .and_then(|models| models.get("default"))
            .and_then(toml::Value::as_str)
    );
}

#[test]
fn dismissed_id_is_dropped_from_override() {
    let sources = CampaignSources {
        dismissed: HashSet::from(["seen".to_owned()]),
        env: overriding(r#"[{"id":"seen","models":{"default":"m"}}]"#),
        ..CampaignSources::default()
    };
    assert_eq!(
        Vec::<String>::new(),
        active_ids(&ConfigLayers::default(), &sources),
        "a dismissed id must not re-apply"
    );
}

#[test]
fn dismiss_persists_handles_corrupt_and_caps() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(campaigns_state_path(home.path()), "{ not json").unwrap();
    dismiss_campaign_ids_at(home.path(), ["new-id".to_owned()]).unwrap();
    assert!(
        home.path().join("campaigns_state.json.corrupt").exists(),
        "corrupt state must be renamed aside, not discarded"
    );

    dismiss_campaign_ids_at(home.path(), (0..40).map(|i| format!("id-{i}"))).unwrap();
    let contents = std::fs::read_to_string(campaigns_state_path(home.path())).unwrap();
    let set: HashSet<String> = serde_json::from_str::<CampaignsState>(&contents)
        .unwrap()
        .dismissed_ids
        .into_iter()
        .collect();
    assert_eq!(MAX_DISMISSED_IDS, set.len());
    assert!(set.contains("id-39"));
    assert!(!set.contains("new-id"), "oldest ids evicted past the cap");
}

#[test]
fn remote_campaign_to_entry_builds_full_patch() {
    let mut patch = serde_json::Map::new();
    patch.insert(
        "models".to_owned(),
        serde_json::json!({ "default": "remote-model" }),
    );
    patch.insert(
        "features".to_owned(),
        serde_json::json!({ "web_fetch": true }),
    );
    let entry = remote_campaign_to_entry(CampaignOverride {
        id: Some("r1".to_owned()),
        patch,
    })
    .expect("entry with id + patch survives");
    let expected: toml::Table =
        toml::from_str("[models]\ndefault = \"remote-model\"\n[features]\nweb_fetch = true\n")
            .unwrap();
    assert_eq!(expected, entry.patch);

    let mut patch = serde_json::Map::new();
    patch.insert("models".to_owned(), serde_json::json!({ "default": "x" }));
    assert!(remote_campaign_to_entry(CampaignOverride { id: None, patch }).is_none());
}

#[test]
fn campaign_id_json_alias_is_accepted_and_does_not_leak_into_patch() {
    for json in [
        r#"[{"campaign_id":"r1","models":{"default":"m"}}]"#,
        r#"[{"id":"r1","models":{"default":"m"}}]"#,
    ] {
        let list: Vec<CampaignOverride> = serde_json::from_str(json).unwrap();
        let entry = remote_campaign_to_entry(list.into_iter().next().unwrap())
            .expect("entry with id survives");
        assert_eq!("r1", entry.id);
        assert_eq!(models_default_patch("m"), entry.patch, "{json}");
    }
}
