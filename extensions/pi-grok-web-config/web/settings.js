
	function settingsLabel(key) { return t("settings_field_" + key.replaceAll(".", "_")); }
	function getSetting(doc, path) { return path.split(".").reduce((value, key) => value?.[key], doc); }
	function setSetting(doc, path, value) {
		const keys = path.split(".");
		let parent = doc;
		for (const key of keys.slice(0, -1)) {
			if (!parent[key] || typeof parent[key] !== "object" || Array.isArray(parent[key])) parent[key] = {};
			parent = parent[key];
		}
		if (value === undefined) delete parent[keys.at(-1)]; else parent[keys.at(-1)] = value;
	}
	function renderSettings() {
		renderBanner("settings-banner", state.settingsError ? t("banner_settings", { error: state.settingsError }) : null);
		if (!view.settingsDirty) {
			view.settingsDraft = clone(state.settings || {});
			view.settingsBase = clone(state.settings || {});
			$("#settings-json").value = JSON.stringify(view.settingsDraft, null, 2);
		}
		const doc = view.settingsDraft || state.settings || {};
		const query = view.settingsQuery.trim().toLocaleLowerCase();
		const matches = (key, label, description = "") => !query || (key + " " + label + " " + description).toLocaleLowerCase().includes(query);
		const toggles = $("#settings-toggles");
		toggles.replaceChildren();
		for (const field of SETTINGS_TOGGLES) {
			const title = t(field.labelKey), description = t(field.descriptionKey);
			if (!matches(field.key, title, description)) continue;
			toggles.appendChild(el("div", { class: "setting-toggle", "data-setting-key":field.key }, [
				el("div", { class: "setting-toggle-copy" }, [el("strong", { text: title }), el("span", { text: description }), el("code", {class:"path",text:field.key})]),
				switchControl({ checked: getSetting(doc, field.key) ?? field.default, disabled: Boolean(state.settingsError), label: title, onchange: (value) => stageSetting(field.key, value) }),
			]));
		}
		$("#quick-settings-card").classList.toggle("hidden", !toggles.childElementCount);
		const groups = $("#settings-groups");
		groups.replaceChildren();
		for (const group of UI_CONFIG.settings.groups) {
			const fields = group.fields.filter(field => matches(field.key, settingsLabel(field.key), t("settings_group_" + group.key)));
			if (!fields.length) continue;
			groups.appendChild(el("section", {class:"settings-section"}, [
				el("div", {class:"settings-section-heading"}, [el("h2", {text:t("settings_group_"+group.key)}),el("p",{text:t("settings_group_"+group.key+"_desc")})]),
				el("div",{class:"settings-fields"}, fields.map(field=>settingField(field, doc)))
			]));
		}
		if (!groups.childElementCount && !toggles.childElementCount) groups.appendChild(emptyState(t("host_no_results")));
		$("#settings-path").textContent = state.paths?.settings || "";
		$("#settings-path").title = state.paths?.settings || "";
		markSettingsState();
	}
	function settingField(field, doc) {
		const stored = getSetting(doc, field.key), current = stored ?? field.default ?? "";
		const label = settingsLabel(field.key);
		let control;
		if (field.type === "boolean") {
			control = switchControl({checked:current === true, disabled:Boolean(state.settingsError),label,onchange:value=>stageSetting(field.key,value)});
		} else {
			control = el(field.type === "select" ? "select" : "input", {
				type:field.type === "select" ? undefined : field.type,
				value:current, min:field.min, step:field.type === "number" ? "1" : undefined,
				"aria-label":label, disabled:Boolean(state.settingsError), autocomplete:"off",
				placeholder:field.default !== undefined ? String(field.default) : t("unset")
			});
			if (field.type === "select") {
				const options = field.options.includes(current) ? field.options : [current, ...field.options];
				control.replaceChildren(...options.map(value=>el("option",{value,text:value ? (I18N[lang]["option_"+value] || value) : t("unset")})));
				control.value = current;
			}
			control.addEventListener("change",()=>{
				let value = control.value.trim() || undefined;
				if (field.type === "number" && value !== undefined) {
					value = Number(value);
					if (!Number.isSafeInteger(value) || value < (field.min ?? 0)) { control.setCustomValidity(t("err_nonnegative",{key:label})); control.reportValidity(); return; }
				}
				control.setCustomValidity(""); stageSetting(field.key,value);
			});
			control.addEventListener("input",()=>control.setCustomValidity(""));
		}
		return el("div",{class:"setting-field", "data-setting-key":field.key}, [
			el("div",{class:"setting-field-copy"},[el("strong",{text:label}),el("code",{text:field.key}),el("small",{text:stored === undefined ? t("inherited_value") : t("configured_value")})]),
			el("div",{class:"setting-field-control"},[control,el("button",{class:"btn small reset-button",type:"button",text:t("reset_default"),disabled:stored === undefined || Boolean(state.settingsError),onclick:()=>stageSetting(field.key,undefined)})])
		]);
	}
	function stageSetting(key,value) {
		try {
			const doc = view.settingsDirty ? parseSettingsArea() : clone(state.settings);
			setSetting(doc,key,value);
			view.settingsDraft = doc;
			view.settingsDirty = !equal(doc,view.settingsBase);
			$("#settings-json").value = JSON.stringify(doc,null,2);
			refreshSettingControls(doc); markSettingsState();
		} catch(error) { notify(error.message,true); refreshSettingControls(view.settingsDraft || state.settings); }
	}

	function refreshSettingControls(doc) {
		for (const field of [...SETTINGS_TOGGLES,...UI_CONFIG.settings.groups.flatMap(group=>group.fields)]) {
			const row = document.querySelector('[data-setting-key="'+CSS.escape(field.key)+'"]');
			if (!row) continue;
			const input = row.querySelector("input,select"), stored = getSetting(doc,field.key), value = stored ?? field.default ?? "";
			if (input?.type === "checkbox") input.checked = value === true; else if (input) input.value = value;
			const reset = row.querySelector(".reset-button"); if (reset) reset.disabled = stored === undefined || Boolean(state.settingsError);
			const status = row.querySelector(".setting-field-copy small"); if(status) status.textContent=t(stored === undefined ? "inherited_value":"configured_value");
		}
	}
	function markSettingsState() {
		const node = $("#settings-state");
		node.textContent = view.settingsDirty ? t("settings_unsaved") : t("settings_saved");
		node.classList.toggle("warning",view.settingsDirty);
		$("#settings-savebar").classList.toggle("dirty",view.settingsDirty);
		$("#settings-draft-status").textContent = t(view.settingsDirty ? "settings_unsaved" : "draft_hint");
		$("#btn-settings-save").disabled = !view.settingsDirty || Boolean(state?.settingsError) || writePending;
		$("#btn-settings-discard").disabled = !view.settingsDirty || writePending;
	}
	function parseSettingsArea() {
		const text = $("#settings-json").value;
		if (!text.trim()) throw new Error(t("err_settings_empty"));
		return objectJson(text,"settings.json");
	}
	function validateSettingsForm(doc) {
		for (const group of UI_CONFIG.settings.groups) for (const field of group.fields) {
			const value = getSetting(doc,field.key);
			if (value === undefined) continue;
			if (field.type === "number" && (!Number.isSafeInteger(value) || value < (field.min ?? 0))) throw new Error(t("err_nonnegative",{key:settingsLabel(field.key)}));
			if (field.type === "boolean" && typeof value !== "boolean") throw new Error(t("err_invalid_type",{key:settingsLabel(field.key),type:"boolean"}));
		}
	}
	async function saveSettingsDraft() {
		if (!view.settingsDirty || writePending) return;
		try {
			const doc = parseSettingsArea();
			validateSettingsForm(doc);
			await putSettings(doc,{draft:true,base:view.settingsBase});
			view.settingsDirty = false;
			renderSettings();
			notify(t("toast_settings_saved"));
		} catch(error) { markSettingsState(); notify(error.message,true); }
	}
	function discardSettingsDraft() {
		if (!view.settingsDirty || !window.confirm(t("confirm_discard"))) return;
		view.settingsDirty = false;
		renderSettings();
	}
