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
		if (visibleIds.length && !visibleIds.includes(view.selectedProvider)) view.selectedProvider = visibleIds[0];
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
			title.textContent = t("provider_details");
			body.appendChild(emptyState(t("pick_provider")));
			return;
		}
		const provider = state.models.providers[id];
		const auth = state.providerAuth?.[id];
		title.textContent = provider.name || id;
		actions.append(
			el("button", { class: "btn", type: "button", onclick: () => editProvider(id), text: t("edit") }),
			el("button", { class: "btn", type: "button", onclick: () => editProvider(null, id), text: t("clone") }),
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
			if (!query || [id, provider.name, provider.baseUrl].some(value=>String(value || "").toLowerCase().includes(query))) return true;
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
				el("button", { class: "btn small", type: "button", onclick: () => editModel(providerId, null, model), text: t("clone") }),
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


	async function putModels(doc, base = state.models) {
		if (state.modelsError) throw new Error(t("banner_models",{error:state.modelsError}));
		await writeOperation(async()=>{
			const latest = await api("/api/state");
			if (latest.modelsError) throw new Error(t("banner_models",{error:latest.modelsError}));
			if (!equal(latest.models,base)) throw new Error(t("conflict"));
			await api("/api/models",{method:"PUT",body:JSON.stringify(doc)});
			await refresh();
		});
	}
	async function putSettings(doc, {draft = false, base = state.settings} = {}) {
		if (view.settingsDirty && !draft) throw new Error(t("resolve_draft"));
		if (state.settingsError) throw new Error(t("banner_settings",{error:state.settingsError}));
		await writeOperation(async()=>{
			const latest = await api("/api/state");
			if (latest.settingsError) throw new Error(t("banner_settings",{error:latest.settingsError}));
			if (!equal(latest.settings,base)) throw new Error(t("conflict"));
			await api("/api/settings",{method:"PUT",body:JSON.stringify(doc)});
			await refresh();
		});
	}
	async function useModel(providerId,modelId) {
		try {
			await writeOperation(async()=>{
				await api("/api/use-model",{method:"POST",body:JSON.stringify({provider:providerId,modelId})});
				await refresh();
			});
			notify(t("toast_now_using",{provider:providerId,model:modelId}));
		} catch(error) { notify(error.message,true); }
	}
	async function setDefault(providerId,modelId) {
		try {
			await putSettings({...state.settings,defaultProvider:providerId,defaultModel:modelId});
			notify(t("toast_default_set",{provider:providerId,model:modelId}));
		} catch(error) { notify(error.message,true); }
	}
	function isUsed(provider,model) {
		return [state.current,state.defaults].some(value=>value?.provider === provider && (!model || value.modelId === model));
	}
	async function deleteProvider(id) {
		if (isUsed(id)) { notify(t("delete_active"),true); return; }
		if (!window.confirm(t("confirm_delete_provider",{id}))) return;
		try {
			const doc = clone(state.models); delete doc.providers[id];
			await putModels(doc); notify(t("toast_deleted_provider",{id}));
		} catch(error) { notify(error.message,true); }
	}
	async function deleteModel(providerId,modelId) {
		if (isUsed(providerId,modelId)) { notify(t("delete_active"),true); return; }
		if (!window.confirm(t("confirm_delete_model",{id:modelId}))) return;
		try {
			const doc = clone(state.models);
			doc.providers[providerId].models = (doc.providers[providerId].models || []).filter(model=>model.id !== modelId);
			await putModels(doc); notify(t("toast_deleted_model",{provider:providerId,model:modelId}));
		} catch(error) { notify(error.message,true); }
	}
	function booleanField(key,label,value) {
		return {key,label,type:"select",value:value === undefined ? "" : String(value), options:[{value:"",label:t("unset")},{value:"true",label:t("yes")},{value:"false",label:t("no")}]};
	}
	function jsonField(key,label,value) { return {key,label,type:"textarea",full:true,value:value === undefined ? "" : JSON.stringify(value,null,2)}; }
	function validateUrl(value) {
		if (!value) return;
		try { const url = new URL(value); if (!["http:","https:"].includes(url.protocol)) throw new Error(); }
		catch { throw new Error(t("err_url")); }
	}
	function advancedValues(values,withOverrides = false) {
		const headers = objectJson(values.headers,t("f_headers"));
		if (headers && Object.values(headers).some(value=>typeof value !== "string")) throw new Error(t("headers_error"));
		const out = {headers,compat:objectJson(values.compat,t("f_compat"))};
		if (withOverrides) out.modelOverrides = objectJson(values.modelOverrides,t("f_overrides"));
		return out;
	}
	function optionalBoolean(value) { return value === "" ? undefined : value === "true"; }
	function editProvider(id,cloneId = null) {
		const existing = Boolean(id);
		const baseDoc = clone(state.models);
		const provider = baseDoc.providers[id || cloneId] || {};
		openEditor({
			title:existing ? t("edit_provider",{id}) : t(cloneId ? "clone_provider" : "add_provider_title"),
			description:t("provider_form_hint"),
			fields:[
				{key:"id",label:t("f_id"),value:id || (cloneId ? cloneId+"-copy" : ""),disabled:existing,full:true},
				{key:"name",label:t("f_name"),value:provider.name || ""},
				{key:"api",label:t("f_api"),value:provider.api || "",list:"api-options"},
				{key:"baseUrl",label:t("f_baseurl"),value:provider.baseUrl || "",placeholder:"https://api.example.com/v1",full:true},
				{key:"apiKey",label:t("f_apikey"),type:"password",value:provider.apiKey || "",hint:t("api_key_hint"),full:true},
				booleanField("authHeader",t("f_authheader"),provider.authHeader),
				jsonField("headers",t("f_headers"),provider.headers),jsonField("compat",t("f_compat"),provider.compat),jsonField("modelOverrides",t("f_overrides"),provider.modelOverrides)
			],
			onSubmit:async values=>{
				const providerId = existing ? id : values.id;
				if (!providerId) throw new Error(t("err_provider_id"));
				if (!existing && Object.hasOwn(baseDoc.providers,providerId)) throw new Error(t("err_provider_exists",{id:providerId}));
				validateUrl(values.baseUrl);
				const doc = clone(baseDoc);
				Object.defineProperty(doc.providers,providerId,{value:withOptionalFields(provider,{
					name:values.name,api:values.api,baseUrl:values.baseUrl,apiKey:values.apiKey,authHeader:optionalBoolean(values.authHeader),...advancedValues(values,true)
				}),enumerable:true,writable:true,configurable:true});
				await putModels(doc,baseDoc);
				view.selectedProvider = providerId; renderModels();
				notify(t("toast_saved_provider",{id:providerId}));
			}
		});
	}
	function editModel(providerId,model,cloneModel = null) {
		const existing = Boolean(model);
		const baseDoc = clone(state.models);
		const source = model || cloneModel || {};
		const cost = source.cost || {};
		openEditor({
			title:existing ? t("edit_model",{id:model.id}) : cloneModel ? t("clone_model") : t("add_model_title",{provider:providerId}),
			description:t("model_form_hint"),
			fields:[
				{key:"id",label:t("f_model_id"),value:model?.id || (cloneModel ? cloneModel.id+"-copy" : ""),disabled:existing,full:true},
				{key:"name",label:t("f_name"),value:source.name || ""},
				{key:"api",label:t("f_api"),value:source.api || "",list:"api-options"},
				{key:"baseUrl",label:t("f_baseurl"),value:source.baseUrl || "",full:true},
				{key:"contextWindow",label:t("f_ctx"),type:"number",value:source.contextWindow,min:1},
				{key:"maxTokens",label:t("f_maxtokens"),type:"number",value:source.maxTokens,min:1},
				...["Input","Output","CacheRead","CacheWrite"].map(name=>({key:"cost"+name,label:t("f_cost_"+name.replace(/[A-Z]/g,(char,index)=>(index ? "_" : "")+char.toLowerCase())),type:"number",value:cost[name[0].toLowerCase()+name.slice(1)],min:0})),
				booleanField("reasoning",t("f_reasoning"),source.reasoning),
				booleanField("imageInput",t("f_images"),source.input === undefined ? undefined : source.input.includes("image")),
				jsonField("headers",t("f_headers"),source.headers),jsonField("compat",t("f_compat"),source.compat)
			],
			onSubmit:async values=>{
				const id = existing ? model.id : values.id;
				if (!id) throw new Error(t("err_model_id"));
				if (!existing && (baseDoc.providers[providerId].models || []).some(value=>value.id===id)) throw new Error(t("err_model_exists",{id}));
				validateUrl(values.baseUrl);
				for (const key of ["contextWindow","maxTokens"]) if (values[key] !== undefined && (!Number.isSafeInteger(values[key]) || values[key]<=0)) throw new Error(t("err_positive",{key:t(key==="contextWindow"?"f_ctx":"f_maxtokens")}));
				const entry = withOptionalFields(source,{name:values.name,api:values.api,baseUrl:values.baseUrl,contextWindow:values.contextWindow,maxTokens:values.maxTokens,reasoning:optionalBoolean(values.reasoning),...advancedValues(values)});
				entry.id = id;
				if (values.imageInput === "") delete entry.input;
				else { const input = new Set(source.input || ["text"]); if (values.imageInput==="true") input.add("image"); else input.delete("image"); entry.input=[...input]; }
				const nextCost = {};
				for (const name of ["Input","Output","CacheRead","CacheWrite"]) {
					const value = values["cost"+name];
					if (value !== undefined && (!Number.isFinite(value)||value<0)) throw new Error(t("err_nonnegative",{key:"cost"+name}));
					nextCost[name[0].toLowerCase()+name.slice(1)] = value ?? 0;
				}
				if (["costInput","costOutput","costCacheRead","costCacheWrite"].some(key=>values[key]!==undefined)) entry.cost={...cost,...nextCost};
				else {
					const remaining = {...cost};
					for (const key of ["input","output","cacheRead","cacheWrite"]) delete remaining[key];
					if (Object.keys(remaining).length) entry.cost=remaining; else delete entry.cost;
				}
				const doc = clone(baseDoc), models = doc.providers[providerId].models || [];
				const index = models.findIndex(value=>value.id===id);
				if (index>=0) models[index]=entry; else models.push(entry);
				doc.providers[providerId].models=models;
				await putModels(doc,baseDoc); notify(t("toast_saved_model",{provider:providerId,model:id}));
			}
		});
	}
