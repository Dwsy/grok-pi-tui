	function isScalar(value) {
		return ["boolean", "number", "string"].includes(typeof value);
	}

	function hostExtras() {
		if (!state?.host) return [];
		const covered = new Set((state.host.catalog || []).map((entry) => entry.key));
		return Object.entries(state.host.ui || {})
			.filter(([key, value]) => !covered.has(key) && isScalar(value))
			.sort(([a], [b]) => a.localeCompare(b))
			.map(([key, value]) => ({ key, default: value }));
	}

	function renderHost() {
		const host = state.host || { ui: {}, uiTables: {}, catalog: [] };
		renderBanner("host-banner", host.error ? t("host_error_banner", { error: host.error }) : null);
		const query = view.hostQuery.trim().toLowerCase();
		const matches = (entry) => !query || [entry.key, entry.label, entry.description, entry.section, entry.source].some((value) => String(value || "").toLowerCase().includes(query));
		const catalog = (host.catalog || []).filter(matches);
		const extras = hostExtras().filter(matches);
		const sections = new Map();
		for (const entry of catalog) {
			const section = entry.section || t("host_section_other");
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
		if (sections.size === 0 && tableKeys.length === 0) body.appendChild(emptyState(t("res_none")));
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
		const ui = host.ui || {};
		const configured = ui[entry.key] !== undefined;
		const current = configured ? ui[entry.key] : entry.default;
		const disabled = Boolean(host.error);
		let control;
		if (typeof current === "boolean" || entry.kind === "bool") {
			control = switchControl({
				checked: current === true,
				disabled,
				label: entry.label || entry.key,
				onchange: (value) => saveHostValue(entry, value),
			});
		} else {
			control = el("input", {
				type: typeof current === "number" ? "number" : "text",
				value: current ?? "",
				disabled,
				"aria-label": entry.label || entry.key,
			});
			control.addEventListener("change", () => {
				if (typeof current === "number") {
					const value = Number(control.value);
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
		return el("div", { class: "host-row" }, [
			el("div", {}, [
				el("div", { class: "host-label", text: entry.label || entry.key }),
				entry.description ? el("p", { class: "host-description", text: entry.description }) : null,
				el("div", { class: "host-meta" }, meta),
			]),
			el("div", { class: "host-control" }, control),
		]);
	}

	async function saveHostValue(entry, value) {
		try {
			await api("/api/host-ui", { method: "PUT", body: JSON.stringify({ [entry.key]: value }) });
			await refresh();
			notify(t(entry.restartRequired ? "toast_host_saved_restart" : "toast_host_saved", { key: entry.key }));
		} catch (error) {
			notify(error.message, true);
			await refresh();
		}
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
