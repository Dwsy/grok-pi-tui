	function renderSettings() {
		renderBanner("settings-banner", state.settingsError ? t("banner_settings", { error: state.settingsError }) : null);
		const toggles = $("#settings-toggles");
		toggles.replaceChildren();
		for (const { key, labelKey, descriptionKey } of SETTINGS_TOGGLES) {
			const title = t(labelKey);
			const description = t(descriptionKey);
			toggles.appendChild(el("div", { class: "setting-toggle" }, [
				el("div", { class: "setting-toggle-copy" }, [el("strong", { text: title }), el("span", { text: description })]),
				switchControl({
					checked: state.settings?.[key] === true,
					disabled: Boolean(state.settingsError),
					label: title,
					onchange: (checked) => toggleSetting(key, checked),
				}),
			]));
		}
		$("#settings-path").textContent = state.paths?.settings || "";
		$("#settings-path").title = state.paths?.settings || "";
		const area = $("#settings-json");
		if (!view.settingsDirty) area.value = JSON.stringify(state.settings || {}, null, 2);
		markSettingsState();
	}

	function markSettingsState() {
		const node = $("#settings-state");
		node.textContent = view.settingsDirty ? t("settings_unsaved") : t("settings_saved");
		node.classList.toggle("warning", view.settingsDirty);
	}

	function parseSettingsArea() {
		const text = $("#settings-json").value;
		if (!text.trim()) throw new Error(t("err_settings_empty"));
		const parsed = JSON.parse(text);
		if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) throw new Error(t("err_settings_object"));
		return parsed;
	}

	async function toggleSetting(key, value) {
		const wasDirty = view.settingsDirty;
		try {
			const doc = wasDirty ? parseSettingsArea() : { ...state.settings };
			doc[key] = value;
			view.settingsDirty = false;
			await putSettings(doc);
			notify(t("toast_key_set", { key, value }));
		} catch (error) {
			view.settingsDirty = wasDirty;
			markSettingsState();
			notify(error.message, true);
			if (!wasDirty) await refresh();
		}
	}
