	function filterEntries(entries, query) {
		if (!query) return entries;
		const needle = query.toLowerCase();
		return entries.filter((entry) => [entry.name, entry.path, entry.description, entry.source].some((value) => String(value || "").toLowerCase().includes(needle)));
	}

	function renderResources() {
		renderBanner("resources-banner", state.resources?.error ? t("res_scan_warning", { error: state.resources.error }) : null);
		const stats = $("#resource-stats");
		stats.replaceChildren(...RESOURCE_KEYS.map((key) => metric(t(RESOURCE_LABELS[key]), state.resources?.[key]?.length || 0)));

		const kinds = $("#resource-kinds");
		kinds.replaceChildren();
		for (const key of RESOURCE_KEYS) {
			kinds.appendChild(el("button", {
				type: "button",
				class: key === view.resourceKind ? "active" : "",
				onclick: () => {
					view.resourceKind = key;
					view.resourcePage = 0;
					renderResources();
				},
				text: `${t(RESOURCE_LABELS[key])} · ${state.resources?.[key]?.length || 0}`,
			}));
		}

		const body = $("#resources-body");
		body.replaceChildren();
		const allEntries = state.resources?.[view.resourceKind] || [];
		const entries = filterEntries(allEntries, view.resourceQuery.trim());
		const pages = Math.max(1, Math.ceil(entries.length / RESOURCE_PAGE_SIZE));
		view.resourcePage = Math.min(view.resourcePage, pages - 1);
		const start = view.resourcePage * RESOURCE_PAGE_SIZE;
		const visible = entries.slice(start, start + RESOURCE_PAGE_SIZE);

		const headerChildren = [
			el("div", {}, [
				el("h2", { text: t(RESOURCE_LABELS[view.resourceKind]) }),
				el("p", { text: view.resourceKind === "extensions" ? t("res_extensions_hint") : t("res_readonly_hint") }),
				el("p", { text: t("res_visible", { visible: entries.length, total: allEntries.length }) }),
			]),
		];
		if (view.resourceKind === "extensions") headerChildren.push(resourceAddControl());
		body.appendChild(el("div", { class: "resource-section-head" }, headerChildren));

		if (visible.length === 0) {
			body.appendChild(emptyState(t("res_none")));
		} else {
			const list = el("div", { class: "resource-list" });
			for (const entry of visible) list.appendChild(resourceRow(view.resourceKind, entry));
			body.appendChild(list);
		}
		if (pages > 1) body.appendChild(pagination(pages));
	}

	function metric(label, count) {
		return el("div", { class: "metric" }, [el("span", { text: label }), el("strong", { text: String(count) })]);
	}

	function resourceAddControl() {
		const input = el("input", { type: "text", placeholder: t("res_path_placeholder"), "aria-label": t("res_path_placeholder") });
		const submit = () => addResourcePath("extensions", input.value);
		input.addEventListener("keydown", (event) => {
			if (event.key === "Enter") {
				event.preventDefault();
				submit();
			}
		});
		return el("div", { class: "resource-add" }, [
			input,
			el("button", { class: "btn primary", type: "button", onclick: submit, text: t("add") }),
		]);
	}

	function resourceRow(key, entry) {
		const sourceKey = entry.source === "settings" ? "source_settings" : entry.source === "cli" ? "source_cli" : "source_discovered";
		const tone = entry.source === "settings" ? "accent" : entry.source === "discovered" ? "success" : "";
		const main = el("div", { class: "resource-main" }, [
			el("div", { class: "resource-name-line" }, [
				el("span", { class: "resource-name", text: entry.name || entry.path }),
				badge(t(sourceKey), tone),
			]),
			entry.description ? el("p", { class: "resource-description", text: entry.description, title: entry.description }) : null,
			el("code", { class: "path", text: entry.path, title: entry.path }),
		]);
		const action = key === "extensions" && entry.source === "settings"
			? el("button", { class: "btn small danger", type: "button", onclick: () => removeResourcePath("extensions", entry.path), text: t("remove") })
			: null;
		return el("article", { class: "resource-row" }, [main, action]);
	}

	function pagination(pages) {
		return el("div", { class: "pagination" }, [
			el("button", { class: "btn small", type: "button", disabled: view.resourcePage === 0, onclick: () => {
				view.resourcePage -= 1;
				renderResources();
				$("#resources-body").scrollIntoView({ block: "start" });
			}, text: t("previous") }),
			el("span", { text: t("page_of", { page: view.resourcePage + 1, pages }) }),
			el("button", { class: "btn small", type: "button", disabled: view.resourcePage >= pages - 1, onclick: () => {
				view.resourcePage += 1;
				renderResources();
				$("#resources-body").scrollIntoView({ block: "start" });
			}, text: t("next") }),
		]);
	}

	async function addResourcePath(key, rawValue) {
		const path = rawValue.trim();
		if (!path) return;
		const current = Array.isArray(state.settings?.[key]) ? state.settings[key] : [];
		if (current.includes(path)) {
			notify(t("toast_already"));
			return;
		}
		try {
			await putSettings({ ...state.settings, [key]: [...current, path] });
			notify(t("toast_added", { key }));
		} catch (error) {
			notify(error.message, true);
		}
	}

	async function removeResourcePath(key, path) {
		const current = Array.isArray(state.settings?.[key]) ? state.settings[key] : [];
		try {
			await putSettings({ ...state.settings, [key]: current.filter((entry) => entry !== path) });
			notify(t("toast_removed", { key }));
		} catch (error) {
			notify(error.message, true);
		}
	}
