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
	let writePending = false;
	let refreshGeneration = 0;
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
		settingsDraft: null,
		settingsBase: null,
		settingsQuery: "",
		hostDraft: {},
		hostBase: null,
		hostCustomized: false,
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

	function storageGet(key) { try { return localStorage.getItem(key); } catch { return null; } }
	function storageSet(key, value) { try { localStorage.setItem(key, value); } catch { /* Private browsing may disable storage. */ } }
	function clone(value) { return JSON.parse(JSON.stringify(value)); }
	function equal(a, b) { return JSON.stringify(a) === JSON.stringify(b); }
	function objectJson(text, key) {
		if (!text.trim()) return undefined;
		let value;
		try { value = JSON.parse(text); } catch (error) { throw new Error(t("invalid_json", { error: error.message })); }
		if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(t("json_object_error", { key }));
		return value;
	}
	async function writeOperation(action) {
		if (writePending) throw new Error(t("busy"));
		writePending = true;
		const blocked = ["#main-content","#tabs","#editor-fields","#btn-refresh","#btn-reload","#btn-stop","#btn-lang","#btn-theme"].map(selector=>$(selector)).filter(Boolean);
		for (const node of blocked) node.inert = true;
		document.body.dataset.saving = "true";
		$("#sync-state").textContent = t("saving");
		let succeeded = false;
		try { const result = await action(); succeeded = true; return result; }
		finally {
			writePending = false;
			for (const node of blocked) node.inert = false;
			delete document.body.dataset.saving;
			if ($("#sync-state")) $("#sync-state").textContent = t(succeeded ? "synced_now" : "save_failed");
			if (state && $("#settings-state")) markSettingsState();
			if ($("#btn-host-save")) {
				$("#btn-host-save").disabled = !Object.keys(view.hostDraft).length || Boolean(state?.host?.error);
				$("#btn-host-discard").disabled = !Object.keys(view.hostDraft).length;
			}
		}
	}

	function detectLang() {
		const stored = storageGet(LANGUAGE_STORAGE_KEY);
		if (SUPPORTED_LANGS.includes(stored)) return stored;
		const browserLang = (navigator.language || "en").toLowerCase().startsWith("zh") ? "zh" : "en";
		return SUPPORTED_LANGS.includes(browserLang) ? browserLang : SUPPORTED_LANGS[0] || "en";
	}

	function detectTheme() {
		const stored = storageGet(THEME_STORAGE_KEY);
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
		document.title = `${t("app_title")} · grok-pi`;
		for (const node of document.querySelectorAll("[data-i18n-aria]")) node.setAttribute("aria-label", t(node.dataset.i18nAria));
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
						el("p", { text: "grok-pi" }),
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
		const generation = ++refreshGeneration;
		let loaded;
		try { loaded = await api("/api/state"); } catch (error) { $("#sync-state").textContent = t("offline"); throw error; }
		if (generation !== refreshGeneration) return;
		state = loaded;
		const ids = providerIds();
		if (!view.selectedProvider || !state.models.providers?.[view.selectedProvider]) {
			view.selectedProvider = [state.current?.provider, state.defaults?.provider, ...ids].find((id) => state.models.providers?.[id]) || null;
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

		$("#session-summary").replaceChildren(
			el("div", {}, [el("span", {text:t("session_model")}), el("strong", {text:state.current?.modelId ? `${state.current.provider} / ${state.current.modelId}` : t("not_set")})]),
			el("div", {}, [el("span", {text:t("startup_model")}), el("strong", {text:state.defaults?.modelId ? `${state.defaults.provider || "—"} / ${state.defaults.modelId}` : t("not_set")})])
		);
		const providerCount = providerIds().length;
		const resourceCount = RESOURCE_KEYS.reduce((sum, key) => sum + resourceEntries(key).length, 0);
		const hostCount = hostCatalog().length + hostExtras().length;
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
		editorContext = { fields, onSubmit, dirty: false };
		$("#editor-submit").classList.remove("hidden");
		$("#editor-title").textContent = title;
		$("#editor-description").textContent = description || "";
		$("#editor-error").classList.add("hidden");
		const body = $("#editor-fields");
		body.replaceChildren();

		let advancedBody = null;
		for (const field of fields) {
			let fieldParent = body;
			if (field.type === "textarea" && ["headers","compat","modelOverrides"].includes(field.key)) {
				if (!advancedBody) { advancedBody = el("div",{class:"advanced-fields"}); body.appendChild(el("details",{class:"advanced-editor full"},[el("summary",{text:t("advanced")}),advancedBody])); }
				fieldParent = advancedBody;
			}
			if (field.type === "checkbox") {
				body.appendChild(el("div", { class: "form-check" }, switchField(field)));
				continue;
			}
			const input = el(field.type === "textarea" ? "textarea" : field.type === "select" ? "select" : "input", {
				type: ["textarea", "select"].includes(field.type) ? undefined : field.type || "text",
				name: field.key,
				value: field.value ?? "",
				placeholder: field.placeholder || "",
				list: field.list,
				step: field.type === "number" ? "any" : undefined,
				min: field.min,
				rows: field.type === "textarea" ? 5 : undefined,
				spellcheck: field.type === "textarea" ? "false" : undefined,
				disabled: field.disabled,
				autocomplete: "off",
			});
			if (field.type === "select") { input.replaceChildren(...field.options.map((option) => el("option", {value:option.value,text:option.label}))); input.value = field.value ?? ""; }
			fieldParent.appendChild(el("label", { class: `form-field${field.full ? " full" : ""}` }, [
				el("span", { text: field.label }),
				input,
				field.type === "password" ? el("button",{type:"button",class:"btn small",text:t("show_secret"),onclick:(event)=>{input.type=input.type === "password"?"text":"password";event.currentTarget.textContent=t(input.type==="password"?"show_secret":"hide_secret");}}) : null,
				field.hint ? el("small", {text:field.hint}) : null,
			]));
		}
		const dialog = $("#editor-dialog");
		dialog.showModal();
		body.querySelector("input:not(:disabled),select,textarea")?.focus();
	}

	function switchField(field) {
		const input = el("input", { type: "checkbox", name: field.key, checked: Boolean(field.value), "aria-label": field.label });
		return el("label", { class: "switch-control" }, [
			input,
			el("span", { class: "switch-track", "aria-hidden": "true" }),
			el("span", { text: field.label }),
		]);
	}

	function closeEditor(force = false) {
		if (writePending && !force) return;
		if (!force && editorContext?.dirty && !window.confirm(t("confirm_editor_discard"))) return;
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
			if (!view.settingsDirty && Object.keys(view.hostDraft).length === 0 && !editorContext?.dirty) return;
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
			try { view.settingsDraft = parseSettingsArea(); } catch { /* Keep invalid raw text for correction. */ }
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
			storageSet(LANGUAGE_STORAGE_KEY, lang);
			renderAll();
		});
		$("#btn-theme").addEventListener("click", () => {
			const index = THEME_MODES.indexOf(theme);
			theme = THEME_MODES[(index + 1) % THEME_MODES.length] || UI_CONFIG.theme.default;
			storageSet(THEME_STORAGE_KEY, theme);
			applyTheme();
		});

		$("#btn-settings-discard").addEventListener("click", discardSettingsDraft);
		$("#btn-settings-format").addEventListener("click", () => {
			try { $("#settings-json").value = JSON.stringify(parseSettingsArea(), null, 2); }
			catch (error) { notify(error.message, true); }
		});
		$("#settings-filter").addEventListener("input", (event) => { view.settingsQuery = event.target.value; renderSettings(); });
		$("#btn-host-save").addEventListener("click", saveHostDraft);
		$("#btn-host-discard").addEventListener("click", () => {
			if (Object.keys(view.hostDraft).length && !window.confirm(t("confirm_discard"))) return;
			view.hostDraft = {}; view.hostBase = null; renderHost();
		});
		$("#host-customized").addEventListener("change", (event) => { view.hostCustomized = event.target.checked; renderHost(); });
		$("#btn-search").addEventListener("click", openSearch);
		$("#global-search").addEventListener("input", renderSearch);
		$("#search-close").addEventListener("click", () => $("#search-dialog").close());
		document.addEventListener("keydown", (event) => {
			if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") { event.preventDefault(); openSearch(); }
			if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
				event.preventDefault();
				if ($("#editor-dialog").open) $("#editor-form").requestSubmit();
				else if (view.tab === "settings") void saveSettingsDraft();
				else if (view.tab === "host") void saveHostDraft();
			}
		});
		$("#btn-settings-validate").addEventListener("click", () => {
			try {
				parseSettingsArea();
				notify(t("toast_valid"));
			} catch (error) {
				notify(error.message, true);
			}
		});
		$("#btn-settings-save").addEventListener("click", saveSettingsDraft);

		$("#editor-close").addEventListener("click", () => closeEditor());
		$("#editor-cancel").addEventListener("click", () => closeEditor());
		$("#editor-dialog").addEventListener("cancel", (event) => { event.preventDefault(); closeEditor(); });
		$("#editor-form").addEventListener("input", () => { if (editorContext) editorContext.dirty = true; });
		$("#editor-form").addEventListener("submit", async (event) => {
			event.preventDefault();
			if (!editorContext || writePending) return;
			const errorNode = $("#editor-error");
			const submit = $("#editor-submit");
			errorNode.classList.add("hidden");
			submit.disabled = true;
			try {
				const values = collectEditorValues(editorContext.fields);
				await editorContext.onSubmit(values);
				closeEditor(true);
			} catch (error) {
				errorNode.textContent = error instanceof Error ? error.message : String(error);
				errorNode.classList.remove("hidden");
			} finally {
				submit.disabled = false;
			}
		});
	}

	function openSearch() {
		if (!state || writePending || $("#editor-dialog").open) return;
		$("#search-dialog").showModal();
		$("#global-search").focus(); renderSearch();
	}
	function searchItems() {
		const rows = [];
		for (const id of providerIds()) {
			rows.push({tab:"models",label:id,detail:t("provider_details"),query:id,provider:id});
			for (const model of state.models.providers[id].models || []) rows.push({tab:"models",label:model.name || model.id,detail:id+" / "+model.id,query:model.id,provider:id});
		}
		for (const kind of RESOURCE_KEYS) for (const entry of resourceEntries(kind)) rows.push({tab:"resources",label:entry.name || entry.path,detail:entry.path,query:entry.path,kind});
		for (const entry of [...hostCatalog(), ...hostExtras()]) rows.push({tab:"host",label:hostLabel(entry),detail:entry.key+" "+hostDescription(entry),query:entry.key});
		for (const field of [...SETTINGS_TOGGLES, ...UI_CONFIG.settings.groups.flatMap(group=>group.fields)]) rows.push({tab:"settings",label:field.labelKey ? t(field.labelKey) : settingsLabel(field.key),detail:field.key,query:field.key});
		return rows;
	}
	function renderSearch() {
		const query = $("#global-search").value.trim().toLocaleLowerCase();
		const all = query ? searchItems().filter(item=>(item.label+" "+item.detail).toLocaleLowerCase().includes(query)) : [];
		$("#search-results").replaceChildren(...(all.length ? all.slice(0,50).map(item=>el("button", {type:"button",class:"search-result",onclick:()=>{
			$("#search-dialog").close();
			if (item.tab === "models") { view.modelQuery=item.query; view.selectedProvider=item.provider; $("#model-search").value=item.query; renderModels(); }
			if (item.tab === "resources") { view.resourceKind=item.kind; view.resourceQuery=item.query; view.resourcePage=0; $("#resource-filter").value=item.query; renderResources(); }
			if (item.tab === "host") { view.hostQuery=item.query; view.hostCustomized=false; $("#host-filter").value=item.query; $("#host-customized").checked=false; renderHost(); }
			if (item.tab === "settings") { view.settingsQuery=item.query; $("#settings-filter").value=item.query; renderSettings(); }
			showTab(item.tab);
			requestAnimationFrame(()=>$("#panel-"+item.tab+" input")?.focus());
		}}, [el("span",{text:item.label}),el("small",{text:t("tab_"+item.tab)+" · "+item.detail})])) : [emptyState(t(query ? "search_empty" : "search_start"))]));
		$("#search-limit").textContent=all.length>50?t("search_more"):"";
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
