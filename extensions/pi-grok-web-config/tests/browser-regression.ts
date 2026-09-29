
import assert from "node:assert/strict";
import { copyFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fixture } from "./fixture.ts";
const root = resolve(import.meta.dir,"../../.."), output = resolve(root,".impeccable/review");
mkdirSync(output,{recursive:true});
const f = await fixture();
const session = `pi-web-regression-${process.pid}-${Date.now()}`;
const passed:string[] = [];
async function command(...args:string[]) {
 const child = Bun.spawn(["agent-browser","--session",session,"--json",...args],{stdout:"pipe",stderr:"pipe"});
 const [out,err,code] = await Promise.all([new Response(child.stdout).text(),new Response(child.stderr).text(),child.exited]);
 if(code) throw new Error("Browser command failed: "+args[0]+" "+err+" "+out);
 const response=JSON.parse(out); if(!response.success)throw new Error(response.error);
 return response.data;
}
async function evaluate(js:string) {return (await command("eval",js)).result;}
async function check(js:string,label:string) {assert.equal(await evaluate(js),true,label);passed.push(label);console.log("PASS "+label);}
async function until(predicate:()=>boolean,label:string) {for(let i=0;i<60;i++){if(predicate())return;await Bun.sleep(50);}throw new Error(label+" "+JSON.stringify(await evaluate("({error:document.querySelector(\"#editor-error\")?.textContent,invalid:[...document.querySelectorAll(\":invalid\")].map(e=>({name:e.name,message:e.validationMessage})),status:document.querySelector(\"#global-status\")?.textContent})")));}
async function snap(name:string) {const data=await command("screenshot");copyFileSync(data.path,resolve(output,name+".png"));}
try {
 const response=await fetch(f.server.url), html=await response.text();
 assert.equal(response.status,200);assert(!/__PI_GROK_WEB_CONFIG_[A-Z_]+__/.test(html));passed.push("Real server assembles every embedded asset");
 assert.equal((await fetch(new URL("/api/state",f.server.url))).status,401);passed.push("Missing token is rejected");
 await command("open",f.server.url);
 await command("wait","#provider-title");
 await evaluate("localStorage.setItem('piWebLang','en');location.reload();true");
 await command("wait","#provider-title");
 await check("document.title === 'Configuration studio · grok-pi'","English page and title");
 await command("click","[data-tab=settings]");
 const before=f.writes;
 await command("click",".setting-toggle:first-child .switch-control");
 assert.equal(f.writes,before);
 await check("!document.querySelector('#btn-settings-save').disabled","Quick settings create a draft without writing");
 await command("click","#raw-settings-details summary");
 await check("JSON.parse(document.querySelector('#settings-json').value).hideThinkingBlock === true","Form and JSON share one draft");
 await command("fill","#settings-json","{ invalid");
 await command("click","#btn-settings-save");
 assert.equal(f.writes,before);
 await check("document.querySelector('#settings-state').textContent === 'Unsaved changes'","Invalid JSON preserves draft and blocks write");
 const doc={...f.state.settings,hideThinkingBlock:true,compaction:{...f.state.settings.compaction,keepRecentTokens:24000}};
 await command("fill","#settings-json",JSON.stringify(doc,null,2));
 await command("click","#btn-settings-save");
 await until(()=>f.state.settings.hideThinkingBlock===true,"settings persisted");
 await check("document.querySelector('#settings-state').textContent === 'Saved'","Explicit save commits settings");
 assert.equal(f.state.settings.customFuture.keep,"yes");passed.push("Unknown settings preserved");
 await command("click","[data-tab=host]");
 await command("fill","#host-filter","pi_loop");
 await command("click",".host-row .switch-control");
 const beforeHost=f.writes;
 await check("!document.querySelector('#btn-host-save').disabled","Host settings stage changes");
 assert.equal(f.writes,beforeHost);
 await command("click","#btn-host-save");
 await until(()=>f.state.host.ui.pi_loop===true,"host setting persisted");
 passed.push("Host batch save persists staged scalar");
 await command("click","[data-tab=models]");
 await command("click","#provider-actions .btn:nth-child(2)");
 await command("fill","#editor-fields [name=id]","openai-copy");
 await command("click",".advanced-editor summary");
 await command("fill","#editor-fields [name=headers]",'{"X-Test":"workbench"}');
 await check("document.querySelector(\"[name=id]\").value === \"openai-copy\" && document.querySelector(\"[name=headers]\").value.includes(\"workbench\")","Advanced editor retains correct field focus");
 await evaluate("document.querySelector('#editor-form').requestSubmit(document.querySelector('#editor-submit'));true");
 await until(()=>Boolean(f.state.models.providers["openai-copy"]),"provider duplicated");
 assert.equal(f.state.models.providers["openai-copy"].customOption,"preserve");
 assert.equal(f.state.models.providers["openai-copy"].headers["X-Test"],"workbench");
 assert.equal(f.state.models.customTopLevel.preserve,true);
 passed.push("Duplicate provider edits headers and preserves unknown fields");
 await command("click",".model-actions .btn:nth-child(4)");
 await command("fill","#editor-fields [name=id]","gpt-5");
 await evaluate("document.querySelector('#editor-form').requestSubmit(document.querySelector('#editor-submit'));true");
 await check("!document.querySelector('#editor-error').classList.contains('hidden')","Duplicate model id rejected");
 await command("fill","#editor-fields [name=id]","gpt-copy");
 await evaluate("document.querySelector('#editor-form').requestSubmit(document.querySelector('#editor-submit'));true");
 await until(()=>f.state.models.providers["openai-copy"].models.length===2,"model duplicated");
 passed.push("Duplicate model preserves optional parameters");
 await command("click","[data-tab=resources]");
 await command("click","#resource-kinds button:nth-child(2)");
 await command("fill",".resource-add input","/demo/new-skill");
 await command("click",".resource-add button");
 await until(()=>f.state.settings.skills.includes("/demo/new-skill"),"resource persisted");
 passed.push("Skills paths editable without deleting resource files");
 await command("click","[data-tab=settings]");
 await command("click",".setting-toggle:first-child .switch-control");
 f.state.settings.externalChange="concurrent";
 await command("click","#btn-settings-save");
 for (let i=0; i<60 && !(await evaluate("document.querySelector('#global-status').textContent.includes('changed outside')")); i++) await Bun.sleep(50);
 await check("document.querySelector('#global-status').textContent.includes('changed outside')","External edits produce a conflict instead of overwrite");
 assert.equal(f.state.settings.externalChange,"concurrent");
 await evaluate("window.confirm=()=>true;true");
 await command("click","#btn-settings-discard");
 await command("click","#btn-refresh");
 await command("click","#btn-lang");
 await check("document.documentElement.lang === 'zh-CN'","Chinese language switch");
 await command("click","#btn-search");
 await command("fill","#global-search","自动压缩");
 await check("document.querySelector('#search-results').textContent.includes('自动压缩')","Global search indexes translated settings");
 await command("click","#search-results button");
 await check("location.hash === '#settings' && document.querySelector('#settings-groups').textContent.includes('自动压缩')","Search navigates and filters correct section");
 await command("fill","#settings-filter","");
 await command("set","viewport","1440","1000");
 await command("click","[data-tab=models]");
 await command("fill","#model-search","");
 await snap("desktop");
 await command("click","[data-tab=host]");
 await command("fill","#host-filter","");
 await snap("host");
 await command("click","[data-tab=settings]");
 await snap("settings");
 for(const width of ["390","768","1440"]) {
  await command("set","viewport",width,"900");
  for(const tab of ["models","resources","host","settings"]){
   await command("click","[data-tab="+tab+"]");
   await check("document.documentElement.scrollWidth <= innerWidth","No horizontal overflow: "+tab+" at "+width+"px");
  }
 }
 await command("set","viewport","390","844");
 await snap("mobile");
 await command("click","#btn-theme");await command("click","#btn-theme");
 await check("document.documentElement.dataset.theme === 'dark'","Dark theme applies");
 await snap("mobile-dark");
 // Re-open the authenticated fixture URL; CLI reload can lose a local ephemeral tab.
 await command("open",f.server.url);
 await command("wait","#provider-title");
 await check("document.documentElement.dataset.theme === 'dark' && document.documentElement.lang === 'zh-CN'","Theme and language persist on reload");
 const errors=await command("errors");
 assert(!errors.errors?.length,JSON.stringify(errors));
 passed.push("No browser runtime errors");
 writeFileSync(resolve(output,"regression.json"),JSON.stringify({passed,synthetic:true},null,2));
 console.log("PASS "+passed.length+" checks");
} finally { await command("close").catch(()=>{});await f.server.close(); }
