(() => {
	"use strict";

	const UI_CONFIG = __PI_GROK_WEB_CONFIG_UI_CONFIG__;
	const I18N = __PI_GROK_WEB_CONFIG_I18N__;
	const PAGE_KEYS = UI_CONFIG.pages;
	const RESOURCE_KEYS = UI_CONFIG.resources.kinds.map((item) => item.key);
	const RESOURCE_LABELS = Object.fromEntries(UI_CONFIG.resources.kinds.map((item) => [item.key, item.labelKey]));
	const RESOURCE_PAGE_SIZE = UI_CONFIG.resources.pageSize;
	const SETTINGS_TOGGLES = UI_CONFIG.settings.quickToggles;
	const LANGUAGE_STORAGE_KEY = UI_CONFIG.language.storageKey;
	const SUPPORTED_LANGS = UI_CONFIG.language.supported;
	const THEME_STORAGE_KEY = UI_CONFIG.theme.storageKey;
	const THEME_MODES = UI_CONFIG.theme.modes;

	const INJECTED_TOKEN = "__PI_GROK_WEB_CONFIG_TOKEN__";
	const TOKEN = INJECTED_TOKEN.startsWith("__PI_GROK")
		? new URLSearchParams(location.search).get("token") || ""
		: INJECTED_TOKEN;
	const VALID_TABS = Object.keys(PAGE_KEYS);

	let state = null;
	let statusTimer = null;
	let editorContext = null;
	let lang = detectLang();
	let theme = detectTheme();
	const view = {
		tab: initialTab(),
		selectedProvider: null,
		modelQuery: "",
		resourceKind: "extensions",
		resourceQuery: "",
		resourcePage: 0,
		hostQuery: "",
		settingsDirty: false,
	};

	const $ = (selector) => document.querySelector(selector);

	function el(tag, attrs = {}, children = []) {
		const node = document.createElement(tag);
		for (const [key, value] of Object.entries(attrs)) {
			if (value === undefined || value === null || value === false) continue;
			if (key === "class") node.className = value;
			else if (key === "text") node.textContent = String(value);
			else if (key === "checked" || key === "disabled") node[key] = Boolean(value);
			else if (key === "value") node.value = String(value);
			else if (key.startsWith("on") && typeof value === "function") node.addEventListener(key.slice(2), value);
			else node.setAttribute(key, String(value));
		}
		for (const child of Array.isArray(children) ? children : [children]) {
			if (child === null || child === undefined || child === false) continue;
			node.appendChild(typeof child === "string" ? document.createTextNode(child) : child);
		}
		return node;
	}

	function detectLang() {
		const stored = localStorage.getItem(LANGUAGE_STORAGE_KEY);
		if (SUPPORTED_LANGS.includes(stored)) return stored;
		const browserLang = (navigator.language || "en").toLowerCase().startsWith("zh") ? "zh" : "en";
		return SUPPORTED_LANGS.includes(browserLang) ? browserLang : SUPPORTED_LANGS[0] || "en";
	}

	function detectTheme() {
		const stored = localStorage.getItem(THEME_STORAGE_KEY);
		return THEME_MODES.includes(stored) ? stored : UI_CONFIG.theme.default;
	}

	function applyTheme() {
		if (theme === "system") document.documentElement.removeAttribute("data-theme");
		else document.documentElement.dataset.theme = theme;
		const button = $("#btn-theme");
		if (button) button.textContent = t(`theme_${theme}`);
	}

	function initialTab() {
		const name = location.hash.replace(/^#/, "");
		return VALID_TABS.includes(name) ? name : "models";
	}

	function t(key, vars) {
		let text = I18N[lang][key] ?? I18N.en[key] ?? key;
		if (vars) {
			for (const [name, value] of Object.entries(vars)) text = text.replaceAll(`{${name}}`, String(value));
		}
		return text;
	}

	function applyI18n() {
		document.documentElement.lang = lang === "zh" ? "zh-CN" : "en";
		for (const node of document.querySelectorAll("[data-i18n]")) node.textContent = t(node.dataset.i18n);
		for (const node of document.querySelectorAll("[data-i18n-placeholder]")) node.placeholder = t(node.dataset.i18nPlaceholder);
		$("#btn-lang").textContent = t("lang_label");
		$("#editor-close").setAttribute("aria-label", t("close"));
		applyTheme();
	}

	function compactPath(value) {
		if (!value) return "—";
		const parts = String(value).split("/").filter(Boolean);
		return parts.length > 2 ? `…/${parts.slice(-2).join("/")}` : String(value);
	}

	function fmtTokens(value) {
		if (typeof value !== "number" || !Number.isFinite(value)) return "—";
		if (value >= 1_000_000) return `${Number((value / 1_000_000).toFixed(1))}M`;
		if (value >= 1_000) return `${Math.round(value / 1_000)}k`;
		return String(value);
	}

	function formatValue(value) {
		if (value === undefined) return "—";
		if (typeof value === "string") return value || "\"\"";
		return JSON.stringify(value);
	}

	function badge(text, tone = "") {
		return el("span", { class: `badge${tone ? ` ${tone}` : ""}`, text });
	}

	function emptyState(title, description) {
		return el("div", { class: "empty-state" }, [el("strong", { text: title }), description ? el("span", { text: description }) : null]);
	}

	function switchControl({ checked, disabled = false, label, onchange }) {
		const input = el("input", { type: "checkbox", checked, disabled, "aria-label": label });
		input.addEventListener("change", () => onchange(input.checked));
		return el("label", { class: "switch-control" }, [input, el("span", { class: "switch-track", "aria-hidden": "true" })]);
	}

	function notify(message, isError = false) {
		const node = $("#global-status");
		clearTimeout(statusTimer);
		node.textContent = message;
		node.classList.remove("hidden");
		node.classList.toggle("error", isError);
		statusTimer = setTimeout(() => node.classList.add("hidden"), 5000);
	}

	function renderBanner(id, message) {
		const node = document.getElementById(id);
		node.replaceChildren();
		node.classList.toggle("hidden", !message);
		if (message) node.appendChild(el("div", { class: "inline-alert", text: message }));
	}

	function showFatal(message) {
		document.body.replaceChildren(
			el("main", { style: "max-width:720px;margin:64px auto;padding:24px" }, [
				el("section", { class: "surface" }, [
					el("div", { class: "surface-body" }, [
						el("p", { class: "eyebrow", text: "grok-pi" }),
						el("h1", { text: t("fatal_title") }),
						el("p", { class: "page-description", text: message }),
					]),
				]),
			]),
		);
	}

	async function api(path, options = {}) {
		let response;
		try {
			response = await fetch(path, {
				...options,
				headers: {
					...(options.headers || {}),
					"content-type": "application/json",
					"x-pi-token": TOKEN,
				},
			});
		} catch (error) {
			throw new Error(t("fatal_load", { error: error instanceof Error ? error.message : String(error) }));
		}
		let payload = {};
		try {
			payload = await response.json();
		} catch {
			// Empty response bodies are allowed.
		}
		if (response.status === 401) {
			showFatal(t("fatal_401"));
			throw new Error(t("fatal_401"));
		}
		if (!response.ok) throw new Error(payload.error || `HTTP ${response.status}`);
		return payload;
	}

	async function refresh({ announce = false } = {}) {
		$("#sync-state").textContent = t("loading");
		state = await api("/api/state");
		const ids = providerIds();
		if (!view.selectedProvider || !state.models.providers?.[view.selectedProvider]) {
			view.selectedProvider = state.current?.provider || state.defaults?.provider || ids[0] || null;
		}
		renderAll();
		$("#sync-state").textContent = t("synced_now");
		if (announce) notify(t("toast_refreshed"));
	}

	function renderAll() {
		applyI18n();
		if (!state) return;
		renderChrome();
		renderModels();
		renderResources();
		renderHost();
		renderSettings();
	}

	function renderChrome() {
		const [kickerKey, titleKey, descriptionKey] = PAGE_KEYS[view.tab];
		$("#page-kicker").textContent = t(kickerKey);
		$("#page-title").textContent = t(titleKey);
		$("#page-description").textContent = t(descriptionKey);

		for (const button of document.querySelectorAll("[data-tab]")) {
			const active = button.dataset.tab === view.tab;
			button.classList.toggle("active", active);
			button.setAttribute("aria-current", active ? "page" : "false");
		}
		for (const name of VALID_TABS) $(`#panel-${name}`).classList.toggle("hidden", name !== view.tab);

		const providerCount = providerIds().length;
		const resourceCount = RESOURCE_KEYS.reduce((sum, key) => sum + (state.resources?.[key]?.length || 0), 0);
		const hostCount = (state.host?.catalog?.length || 0) + hostExtras().length;
		$("#nav-models-count").textContent = String(providerCount);
		$("#nav-resources-count").textContent = String(resourceCount);
		$("#nav-host-count").textContent = String(hostCount);
		$("#nav-settings-count").textContent = String(Object.keys(state.settings || {}).length);

		$("#meta-model").textContent = state.current?.provider && state.current?.modelId
			? `${state.current.provider}/${state.current.modelId}`
			: "grok-pi";
		$("#meta-workspace").textContent = compactPath(state.cwd || state.agentDir);
		$("#meta-workspace").title = state.cwd || state.agentDir || "";
		$("#meta-config").textContent = compactPath(state.paths?.settings);
		$("#meta-config").title = state.paths?.settings || "";
	}

	__PI_GROK_WEB_CONFIG_MODELS__

	function openEditor({ title, description, fields, onSubmit }) {
		editorContext = { fields, onSubmit };
		$("#editor-title").textContent = title;
		$("#editor-description").textContent = description || "";
		$("#editor-error").classList.add("hidden");
		const body = $("#editor-fields");
		body.replaceChildren();

		for (const field of fields) {
			if (field.type === "checkbox") {
				body.appendChild(el("div", { class: "form-check" }, switchField(field)));
				continue;
			}
			const input = el("input", {
				type: field.type || "text",
				name: field.key,
				value: field.value ?? "",
				placeholder: field.placeholder || "",
				list: field.list,
				disabled: field.disabled,
				autocomplete: "off",
			});
			body.appendChild(el("label", { class: `form-field${field.full ? " full" : ""}` }, [
				el("span", { text: field.label }),
				input,
			]));
		}
		const dialog = $("#editor-dialog");
		dialog.showModal();
		requestAnimationFrame(() => body.querySelector("input:not(:disabled)")?.focus());
	}

	function switchField(field) {
		const input = el("input", { type: "checkbox", name: field.key, checked: Boolean(field.value), "aria-label": field.label });
		return el("label", { class: "switch-control" }, [
			input,
			el("span", { class: "switch-track", "aria-hidden": "true" }),
			el("span", { text: field.label }),
		]);
	}

	function closeEditor() {
		if ($("#editor-dialog").open) $("#editor-dialog").close();
		editorContext = null;
	}

	function collectEditorValues(fields) {
		const values = {};
		for (const field of fields) {
			const input = $("#editor-fields").querySelector(`[name="${CSS.escape(field.key)}"]`);
			if (!input) continue;
			if (field.type === "checkbox") values[field.key] = input.checked;
			else if (field.type === "number") values[field.key] = input.value === "" ? undefined : Number(input.value);
			else values[field.key] = input.value.trim();
		}
		return values;
	}

	__PI_GROK_WEB_CONFIG_RESOURCES__

	__PI_GROK_WEB_CONFIG_HOST__

	__PI_GROK_WEB_CONFIG_SETTINGS__

	function showTab(name, updateHash = true) {
		if (!VALID_TABS.includes(name)) return;
		view.tab = name;
		if (updateHash && location.hash !== `#${name}`) location.hash = name;
		if (state) {
			renderChrome();
			return;
		}
		const [kickerKey, titleKey, descriptionKey] = PAGE_KEYS[view.tab];
		$("#page-kicker").textContent = t(kickerKey);
		$("#page-title").textContent = t(titleKey);
		$("#page-description").textContent = t(descriptionKey);
		for (const button of document.querySelectorAll("[data-tab]")) button.classList.toggle("active", button.dataset.tab === view.tab);
		for (const panelName of VALID_TABS) $(`#panel-${panelName}`).classList.toggle("hidden", panelName !== view.tab);
	}

	function bindEvents() {
		$("#tabs").addEventListener("click", (event) => {
			const button = event.target.closest("[data-tab]");
			if (button) showTab(button.dataset.tab);
		});
		window.addEventListener("hashchange", () => {
			const name = initialTab();
			view.tab = name;
			if (state) renderChrome();
		});
		window.addEventListener("beforeunload", (event) => {
			if (!view.settingsDirty) return;
			event.preventDefault();
			event.returnValue = "";
		});

		$("#model-search").addEventListener("input", (event) => {
			view.modelQuery = event.target.value;
			renderModels();
		});
		$("#resource-filter").addEventListener("input", (event) => {
			view.resourceQuery = event.target.value;
			view.resourcePage = 0;
			renderResources();
		});
		$("#host-filter").addEventListener("input", (event) => {
			view.hostQuery = event.target.value;
			renderHost();
		});
		$("#settings-json").addEventListener("input", () => {
			view.settingsDirty = true;
			markSettingsState();
		});

		$("#btn-add-provider").addEventListener("click", () => editProvider(null));
		$("#btn-refresh").addEventListener("click", async () => {
			try {
				await refresh({ announce: true });
			} catch (error) {
				notify(error.message, true);
			}
		});
		$("#btn-reload").addEventListener("click", async () => {
			try {
				await api("/api/reload", { method: "POST" });
				await refresh();
				notify(t("toast_pi_reloaded"));
			} catch (error) {
				notify(error.message, true);
			}
		});
		$("#btn-stop").addEventListener("click", async () => {
			if (!window.confirm(t("confirm_stop"))) return;
			try {
				await api("/api/shutdown", { method: "POST" });
				showFatal(t("stopped"));
			} catch (error) {
				notify(error.message, true);
			}
		});
		$("#btn-lang").addEventListener("click", () => {
			const index = SUPPORTED_LANGS.indexOf(lang);
			lang = SUPPORTED_LANGS[(index + 1) % SUPPORTED_LANGS.length] || SUPPORTED_LANGS[0] || "en";
			localStorage.setItem(LANGUAGE_STORAGE_KEY, lang);
			renderAll();
		});
		$("#btn-theme").addEventListener("click", () => {
			const index = THEME_MODES.indexOf(theme);
			theme = THEME_MODES[(index + 1) % THEME_MODES.length] || UI_CONFIG.theme.default;
			localStorage.setItem(THEME_STORAGE_KEY, theme);
			applyTheme();
		});

		$("#btn-settings-validate").addEventListener("click", () => {
			try {
				parseSettingsArea();
				notify(t("toast_valid"));
			} catch (error) {
				notify(error.message, true);
			}
		});
		$("#btn-settings-save").addEventListener("click", async () => {
			try {
				const doc = parseSettingsArea();
				view.settingsDirty = false;
				await putSettings(doc);
				notify(t("toast_settings_saved"));
			} catch (error) {
				view.settingsDirty = true;
				markSettingsState();
				notify(error.message, true);
			}
		});

		$("#editor-close").addEventListener("click", closeEditor);
		$("#editor-cancel").addEventListener("click", closeEditor);
		$("#editor-dialog").addEventListener("cancel", () => {
			editorContext = null;
		});
		$("#editor-form").addEventListener("submit", async (event) => {
			event.preventDefault();
			if (!editorContext) return;
			const errorNode = $("#editor-error");
			const submit = $("#editor-submit");
			errorNode.classList.add("hidden");
			submit.disabled = true;
			try {
				const values = collectEditorValues(editorContext.fields);
				await editorContext.onSubmit(values);
				closeEditor();
			} catch (error) {
				errorNode.textContent = error instanceof Error ? error.message : String(error);
				errorNode.classList.remove("hidden");
			} finally {
				submit.disabled = false;
			}
		});
	}

	async function init() {
		applyI18n();
		bindEvents();
		showTab(view.tab, false);
		try {
			await refresh();
		} catch (error) {
			showFatal(error instanceof Error ? error.message : String(error));
		}
	}

	void init();
})();
