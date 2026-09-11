	function providerIds() {
		return Object.keys(state?.models?.providers || {}).sort((a, b) => a.localeCompare(b));
	}

	function providerMatches(id, query) {
		if (!query) return true;
		const provider = state.models.providers[id];
		const fields = [id, provider.name, provider.baseUrl, ...(provider.models || []).flatMap((model) => [model.id, model.name])];
		return fields.some((value) => String(value || "").toLowerCase().includes(query));
	}

	function renderModels() {
		renderBanner("models-banner", state.modelsError ? t("banner_models", { error: state.modelsError }) : null);
		const query = view.modelQuery.trim().toLowerCase();
		const ids = providerIds();
		const visibleIds = ids.filter((id) => providerMatches(id, query));
		$("#provider-summary").textContent = t("providers_visible", { visible: visibleIds.length, total: ids.length });

		const list = $("#provider-list");
		list.replaceChildren();
		if (visibleIds.length === 0) {
			list.appendChild(el("li", {}, emptyState(t("no_provider_match"))));
		} else {
			for (const id of visibleIds) {
				const provider = state.models.providers[id];
				const isCurrent = state.current?.provider === id;
				const isDefault = state.defaults?.provider === id;
				const button = el("button", {
					class: `provider-option${id === view.selectedProvider ? " active" : ""}`,
					type: "button",
					onclick: () => {
						view.selectedProvider = id;
						renderModels();
					},
				}, [
					el("span", { class: "provider-line" }, [
						el("span", { class: "provider-name", text: provider.name || id }),
						isCurrent ? badge(t("current"), "accent") : null,
						isDefault ? badge(t("is_default"), "success") : null,
					]),
					el("span", { class: "provider-subline" }, [
						el("code", { text: id }),
						el("span", { text: `· ${(provider.models || []).length}` }),
					]),
				]);
				list.appendChild(el("li", {}, button));
			}
		}
		renderProviderDetail(query);
	}

	function renderProviderDetail(query) {
		const title = $("#provider-title");
		const actions = $("#provider-actions");
		const body = $("#provider-detail");
		actions.replaceChildren();
		body.replaceChildren();

		const id = view.selectedProvider;
		if (!id || !state.models.providers[id]) {
			title.textContent = "Provider";
			body.appendChild(emptyState(t("pick_provider")));
			return;
		}
		const provider = state.models.providers[id];
		const auth = state.providerAuth?.[id];
		title.textContent = provider.name || id;
		actions.append(
			el("button", { class: "btn", type: "button", onclick: () => editProvider(id), text: t("edit") }),
			el("button", { class: "btn danger", type: "button", onclick: () => deleteProvider(id), text: t("delete") }),
		);

		const authText = auth?.configured ? t("auth_configured", { what: auth.label || auth.source || "configured" }) : t("auth_missing");
		const endpoint = provider.baseUrl || t("inherited");
		body.appendChild(el("div", { class: "detail-meta" }, [
			metaCard(t("provider_auth"), authText, auth?.configured ? "success" : "warning"),
			metaCard(t("provider_api"), provider.api || t("inherited")),
			metaCard(t("provider_endpoint"), endpoint, "", true),
		]));

		const header = el("div", { class: "section-heading" }, [
			el("h3", { text: t("models_count", { n: (provider.models || []).length }) }),
			el("button", { class: "btn primary", type: "button", onclick: () => editModel(id, null), text: t("add_model") }),
		]);
		body.appendChild(header);

		const models = (provider.models || []).filter((model) => {
			if (!query) return true;
			return [model.id, model.name].some((value) => String(value || "").toLowerCase().includes(query));
		});
		if (models.length === 0) {
			body.appendChild(emptyState(t("no_models")));
		} else {
			const list = el("div", { class: "model-list" });
			for (const model of models) list.appendChild(renderModelRow(id, provider, model));
			body.appendChild(list);
		}

		const overrideKeys = Object.keys(provider.modelOverrides || {});
		if (overrideKeys.length > 0) body.appendChild(el("p", { class: "page-description", text: t("overrides_badge", { keys: overrideKeys.join(", ") }) }));
	}

	function metaCard(label, value, tone = "", mono = false) {
		return el("div", { class: "meta-card" }, [
			el("span", { text: label }),
			tone ? badge(value, tone) : el(mono ? "code" : "strong", { text: value, title: value }),
		]);
	}

	function renderModelRow(providerId, provider, model) {
		const isCurrent = state.current?.provider === providerId && state.current?.modelId === model.id;
		const isDefault = state.defaults?.provider === providerId && state.defaults?.modelId === model.id;
		const facts = [];
		if (typeof model.contextWindow === "number") facts.push(t("context", { value: fmtTokens(model.contextWindow) }));
		if (typeof model.maxTokens === "number") facts.push(t("max_tokens", { value: fmtTokens(model.maxTokens) }));
		if (model.reasoning) facts.push(t("reasoning"));
		if ((model.input || []).includes("image")) facts.push(t("images"));
		const nameLine = [el("code", { text: model.id, title: model.id })];
		if (isCurrent) nameLine.push(badge(t("current"), "accent"));
		if (isDefault) nameLine.push(badge(t("is_default"), "success"));

		return el("article", { class: "model-row" }, [
			el("div", { class: "model-primary" }, [
				el("div", { class: "resource-name-line" }, nameLine),
				model.name ? el("span", { text: model.name }) : null,
			]),
			el("div", { class: "model-facts" }, facts.length ? facts.map((fact) => el("span", { text: fact })) : [el("span", { text: provider.api || "—" })]),
			el("div", { class: "model-actions" }, [
				el("button", { class: "btn small", type: "button", disabled: isCurrent, onclick: () => useModel(providerId, model.id), text: isCurrent ? t("current") : t("use") }),
				el("button", { class: "btn small", type: "button", disabled: isDefault, onclick: () => setDefault(providerId, model.id), text: isDefault ? t("is_default") : t("set_default") }),
				el("button", { class: "btn small", type: "button", onclick: () => editModel(providerId, model), text: t("edit") }),
				el("button", { class: "btn small danger", type: "button", onclick: () => deleteModel(providerId, model.id), text: t("delete") }),
			]),
		]);
	}

	function withOptionalFields(base, fields) {
		const out = { ...base };
		for (const [key, value] of Object.entries(fields)) {
			if (value === undefined || value === null || value === "") delete out[key];
			else out[key] = value;
		}
		return out;
	}

	async function putModels() {
		if (state.modelsError) throw new Error(t("banner_models", { error: state.modelsError }));
		await api("/api/models", { method: "PUT", body: JSON.stringify(state.models) });
		await refresh();
	}

	async function putSettings(doc) {
		if (state.settingsError) throw new Error(t("banner_settings", { error: state.settingsError }));
		await api("/api/settings", { method: "PUT", body: JSON.stringify(doc) });
		await refresh();
	}

	async function useModel(providerId, modelId) {
		try {
			await api("/api/use-model", { method: "POST", body: JSON.stringify({ provider: providerId, modelId }) });
			await refresh();
			notify(t("toast_now_using", { provider: providerId, model: modelId }));
		} catch (error) {
			notify(error.message, true);
		}
	}

	async function setDefault(providerId, modelId) {
		try {
			await putSettings({ ...state.settings, defaultProvider: providerId, defaultModel: modelId });
			notify(t("toast_default_set", { provider: providerId, model: modelId }));
		} catch (error) {
			notify(error.message, true);
		}
	}

	async function deleteProvider(id) {
		if (!window.confirm(t("confirm_delete_provider", { id }))) return;
		const previous = state.models;
		const providers = { ...state.models.providers };
		delete providers[id];
		state.models = { ...state.models, providers };
		if (view.selectedProvider === id) view.selectedProvider = null;
		try {
			await putModels();
			notify(t("toast_deleted_provider", { id }));
		} catch (error) {
			state.models = previous;
			notify(error.message, true);
			await refresh();
		}
	}

	async function deleteModel(providerId, modelId) {
		if (!window.confirm(t("confirm_delete_model", { id: modelId }))) return;
		const provider = state.models.providers[providerId];
		const previous = provider.models;
		provider.models = (provider.models || []).filter((model) => model.id !== modelId);
		try {
			await putModels();
			notify(t("toast_deleted_model", { provider: providerId, model: modelId }));
		} catch (error) {
			provider.models = previous;
			notify(error.message, true);
			await refresh();
		}
	}

	function editProvider(id) {
		const existing = Boolean(id);
		const provider = existing ? state.models.providers[id] : {};
		openEditor({
			title: existing ? t("edit_provider", { id }) : t("add_provider_title"),
			description: t("provider_form_hint"),
			fields: [
				{ key: "id", label: t("f_id"), value: id || "", disabled: existing, full: true },
				{ key: "name", label: t("f_name"), value: provider.name || "" },
				{ key: "api", label: t("f_api"), value: provider.api || "", list: "api-options" },
				{ key: "baseUrl", label: t("f_baseurl"), value: provider.baseUrl || "", placeholder: "https://api.example.com/v1", full: true },
				{ key: "apiKey", label: t("f_apikey"), value: provider.apiKey || "", full: true },
				{ key: "authHeader", label: t("f_authheader"), type: "checkbox", value: provider.authHeader !== false },
			],
			onSubmit: async (values) => {
				const providerId = existing ? id : values.id;
				if (!providerId) throw new Error(t("err_provider_id"));
				if (!existing && state.models.providers[providerId]) throw new Error(t("err_provider_exists", { id: providerId }));
				const base = existing ? state.models.providers[providerId] : {};
				state.models.providers[providerId] = {
					...withOptionalFields(base, {
						name: values.name,
						baseUrl: values.baseUrl,
						api: values.api,
						apiKey: values.apiKey,
					}),
					authHeader: values.authHeader,
				};
				view.selectedProvider = providerId;
				await putModels();
				notify(t("toast_saved_provider", { id: providerId }));
			},
		});
	}

	function editModel(providerId, model) {
		const existing = Boolean(model);
		const cost = model?.cost || {};
		openEditor({
			title: existing ? t("edit_model", { id: model.id }) : t("add_model_title", { provider: providerId }),
			description: t("model_form_hint"),
			fields: [
				{ key: "id", label: t("f_model_id"), value: model?.id || "", disabled: existing, full: true },
				{ key: "name", label: t("f_name"), value: model?.name || "" },
				{ key: "api", label: t("f_api"), value: model?.api || "", list: "api-options" },
				{ key: "baseUrl", label: t("f_baseurl"), value: model?.baseUrl || "", full: true },
				{ key: "contextWindow", label: t("f_ctx"), type: "number", value: model?.contextWindow },
				{ key: "maxTokens", label: t("f_maxtokens"), type: "number", value: model?.maxTokens },
				{ key: "costInput", label: t("f_cost_input"), type: "number", value: cost.input },
				{ key: "costOutput", label: t("f_cost_output"), type: "number", value: cost.output },
				{ key: "costCacheRead", label: t("f_cost_cache_read"), type: "number", value: cost.cacheRead },
				{ key: "costCacheWrite", label: t("f_cost_cache_write"), type: "number", value: cost.cacheWrite },
				{ key: "reasoning", label: t("f_reasoning"), type: "checkbox", value: Boolean(model?.reasoning) },
				{ key: "imageInput", label: t("f_images"), type: "checkbox", value: (model?.input || []).includes("image") },
			],
			onSubmit: async (values) => {
				const modelId = existing ? model.id : values.id;
				if (!modelId) throw new Error(t("err_model_id"));
				const provider = state.models.providers[providerId];
				const models = [...(provider.models || [])];
				const previous = existing ? models.find((candidate) => candidate.id === modelId) || {} : {};
				const entry = withOptionalFields(previous, {
					name: values.name,
					api: values.api,
					baseUrl: values.baseUrl,
					contextWindow: values.contextWindow,
					maxTokens: values.maxTokens,
				});
				entry.id = modelId;
				entry.reasoning = values.reasoning;
				entry.input = values.imageInput ? ["text", "image"] : ["text"];
				const costValues = [values.costInput, values.costOutput, values.costCacheRead, values.costCacheWrite];
				if (costValues.some((value) => typeof value === "number")) {
					entry.cost = {
						input: values.costInput ?? 0,
						output: values.costOutput ?? 0,
						cacheRead: values.costCacheRead ?? 0,
						cacheWrite: values.costCacheWrite ?? 0,
					};
				} else {
					delete entry.cost;
				}
				const index = models.findIndex((candidate) => candidate.id === modelId);
				if (index >= 0) models[index] = entry;
				else models.push(entry);
				provider.models = models;
				await putModels();
				notify(t("toast_saved_model", { provider: providerId, model: modelId }));
			},
		});
	}
