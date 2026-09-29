	function hostLabel(entry) { return I18N[lang]["host_label_"+entry.key] || entry.label || entry.key; }
	function hostDescription(entry) { return I18N[lang]["host_desc_"+entry.key] || entry.description || ""; }
	function hostSectionLabel(value) { return I18N[lang]["host_section_"+value] || (value === "Pi features" ? t("host_features") : value || t("host_section_other")); }
	function hostCatalog() { return [...new Map([...(UI_CONFIG.host?.catalog || []), ...(state?.host?.catalog || [])].map(entry=>[entry.key,entry])).values()]; }
	function isScalar(value) {
		return ["boolean", "number", "string"].includes(typeof value);
	}

	function hostExtras() {
		if (!state?.host) return [];
		const covered = new Set(hostCatalog().map((entry) => entry.key));
		return Object.entries(state.host.ui || {})
			.filter(([key, value]) => !covered.has(key) && isScalar(value))
			.sort(([a], [b]) => a.localeCompare(b))
			.map(([key, value]) => ({ key, default: value }));
	}

	function renderHost() {
		const host = state.host || { ui: {}, uiTables: {}, catalog: [] };
		renderBanner("host-banner", host.error ? t("host_error_banner", { error: host.error }) : null);
		renderHostStatus();
		const query = view.hostQuery.trim().toLowerCase();
		const matches = (entry) => !query || [entry.key, hostLabel(entry), hostDescription(entry), hostSectionLabel(entry.section), entry.source].some((value) => String(value || "").toLowerCase().includes(query));
		const filtered = (entry) => matches(entry) && (!view.hostCustomized || host.ui?.[entry.key] !== undefined || Object.hasOwn(view.hostDraft,entry.key));
		const catalog = hostCatalog().filter(filtered);
		const extras = hostExtras().filter(filtered);
		const sections = new Map();
		for (const entry of catalog) {
			const section = hostSectionLabel(entry.section);
			if (!sections.has(section)) sections.set(section, []);
			sections.get(section).push(entry);
		}
		if (extras.length) {
			const name = t("host_section_other");
			if (!sections.has(name)) sections.set(name, []);
			sections.get(name).push(...extras);
		}

		const nav = $("#host-section-nav");
		const body = $("#host-body");
		nav.replaceChildren();
		body.replaceChildren();
		let index = 0;
		for (const [section, entries] of sections) {
			const id = `host-section-${index++}`;
			nav.appendChild(el("button", { type: "button", text: `${section} · ${entries.length}`, onclick: () => document.getElementById(id)?.scrollIntoView({ behavior: "smooth", block: "start" }) }));
			body.appendChild(hostSection(id, section, entries, host));
		}

		const tableKeys = Object.keys(host.uiTables || {}).filter((key) => !query || key.toLowerCase().includes(query));
		if (tableKeys.length > 0) body.appendChild(readonlyTables(host.uiTables, tableKeys));
		if (sections.size === 0 && tableKeys.length === 0) body.appendChild(emptyState(t("host_no_results")));
	}

	function renderHostStatus() {
		const pending = Object.keys(view.hostDraft).length;
		$("#host-draft-status").textContent = pending ? t("host_modified",{n:pending}) : t("draft_hint");
		$("#host-savebar").classList.toggle("dirty",pending>0);
		$("#btn-host-save").disabled = !pending || Boolean(state.host?.error) || writePending;
		$("#btn-host-discard").disabled = !pending || writePending;
	}

	function hostSection(id, section, entries, host) {
		return el("section", { class: "surface host-card", id }, [
			el("header", { class: "surface-header" }, [
				el("div", { class: "surface-title-group" }, [
					el("p", { class: "eyebrow", text: t("host_settings_count", { n: entries.length }) }),
					el("h2", { text: section }),
				]),
				host.configPath ? el("code", { class: "path", text: compactPath(host.configPath), title: host.configPath }) : null,
			]),
			el("div", { class: "surface-body host-rows" }, entries.map((entry) => hostRow(entry, host))),
		]);
	}

	function hostRow(entry, host) {
		const ui = {...(host.ui || {}), ...view.hostDraft};
		const configured = ui[entry.key] !== undefined;
		const current = configured ? ui[entry.key] : entry.default;
		const disabled = Boolean(host.error);
		let control;
		if (typeof current === "boolean" || entry.kind === "bool") {
			control = switchControl({
				checked: current === true,
				disabled,
				label: hostLabel(entry),
				onchange: (value) => saveHostValue(entry, value),
			});
		} else if (entry.options) {
			control = el("select", {"aria-label":hostLabel(entry),disabled}, entry.options.map(value=>el("option",{value,text:I18N[lang]["option_"+value] || value})));
			control.value = current;
			control.addEventListener("change",()=>saveHostValue(entry,control.value));
		} else {
			control = el("input", {
				type: typeof current === "number" ? "number" : "text",
				value: current ?? "",
				disabled,
				"aria-label": hostLabel(entry),
			});
			control.addEventListener("change", () => {
				if (typeof current === "number") {
					const value = control.value.trim() ? Number(control.value) : NaN;
					if (!Number.isFinite(value)) {
						notify(t("err_invalid_number", { key: entry.key }), true);
						return;
					}
					void saveHostValue(entry, value);
				} else {
					void saveHostValue(entry, control.value);
				}
			});
		}
		const meta = [badge(entry.key)];
		if (!configured && entry.default !== undefined) meta.push(badge(t("default_value", { value: formatValue(entry.default) })));
		if (entry.restartRequired) meta.push(badge(t("host_restart"), "warning"));
		if (entry.source) meta.push(badge(entry.source.split("/")[1] || entry.source));
		return el("div", { class: "host-row", "data-host-key":entry.key }, [
			el("div", {}, [
				el("div", { class: "host-label", text: hostLabel(entry) }),
				hostDescription(entry) ? el("p", { class: "host-description", text: hostDescription(entry) }) : null,
				el("div", { class: "host-meta" }, meta),
			]),
			el("div", { class: "host-control" }, [control, entry.default !== undefined ? el("button", {type:"button",class:"btn small reset-button",text:t("reset_default"),disabled:disabled || equal(current,entry.default),onclick:()=>saveHostValue(entry,entry.default)}) : null]),
		]);
	}


	function saveHostValue(entry,value) {
		if (!view.hostBase) view.hostBase = clone(state.host.ui || {});
		const original = state.host.ui?.[entry.key] ?? entry.default;
		if (equal(original,value)) delete view.hostDraft[entry.key];
		else view.hostDraft[entry.key] = value;
		const row = document.querySelector('[data-host-key="'+CSS.escape(entry.key)+'"]');
		const input = row?.querySelector("input,select");
		if (input?.type === "checkbox") input.checked=value === true; else if(input) input.value=value ?? "";
		const reset=row?.querySelector(".reset-button");if(reset)reset.disabled=equal(value,entry.default);
		renderHostStatus();
	}
	async function saveHostDraft() {
		if (!Object.keys(view.hostDraft).length || writePending) return;
		try {
			await writeOperation(async()=>{
				const latest = await api("/api/state");
				if (latest.host?.error) throw new Error(latest.host.error);
				for (const key of Object.keys(view.hostDraft)) {
					if (!equal(latest.host?.ui?.[key],view.hostBase?.[key])) throw new Error(t("conflict"));
				}
				await api("/api/host-ui",{method:"PUT",body:JSON.stringify(view.hostDraft)});
				view.hostDraft = {}; view.hostBase = null;
				await refresh();
			});
			notify(t("host_saved"));
		} catch(error) { notify(error.message,true); }
		renderHost();
	}

	function readonlyTables(tables, keys) {
		return el("section", { class: "surface readonly-tables" }, [
			el("div", { class: "surface-body" }, [
				el("details", {}, [
					el("summary", { text: `${t("host_readonly_tables")} · ${keys.length}` }),
					el("p", { class: "host-description", text: t("host_readonly_hint") }),
					...keys.map((key) => el("div", { class: "readonly-table" }, [
						badge(`ui.${key}`),
						el("pre", { text: JSON.stringify(tables[key], null, 2) }),
					])),
				]),
			]),
		]);
	}
