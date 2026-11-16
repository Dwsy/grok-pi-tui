import type { IncomingMessage, ServerResponse } from "node:http";

export type McpevalWebDeps = {
  descriptor: () => Record<string, unknown>;
  discover: () => unknown[];
  oauth?: () => Record<string, unknown>;
  getEventQueueSize?: () => number;
};

function json(response: ServerResponse, status: number, value: unknown): void {
  response.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
    "x-content-type-options": "nosniff",
  });
  response.end(JSON.stringify(value));
}

function html(response: ServerResponse, body: string): void {
  response.writeHead(200, {
    "content-type": "text/html; charset=utf-8",
    "cache-control": "no-store",
    "content-security-policy": "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'",
    "x-content-type-options": "nosniff",
  });
  response.end(body);
}

function esc(value: unknown): string {
  return String(value ?? "").replace(/[&<>"']/g, (char) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  }[char] ?? char));
}

export function renderMcpevalDashboard(): string {
  return `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width">
<title>mcpeval daemon</title>
<style>
:root{color-scheme:dark;font:15px/1.5 ui-sans-serif,system-ui,sans-serif;background:#101318;color:#edf2f7}
body{max-width:1080px;margin:0 auto;padding:28px}h1{margin:0 0 4px;font-size:28px}
.muted{color:#9aa7b4}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(220px,1fr));gap:12px;margin:20px 0}
.card{border:1px solid #2d3748;border-radius:10px;padding:14px;background:#171c24}
code,pre{font-family:ui-monospace,SFMono-Regular,monospace}code{color:#9ae6b4}
table{width:100%;border-collapse:collapse}th,td{text-align:left;padding:8px;border-bottom:1px solid #2d3748;font-size:13px}
.online{color:#9ae6b4}.empty{padding:16px 0}.error{color:#feb2b2}
</style></head><body>
<h1>mcpeval daemon</h1><div class="muted">Resident MCP relay for Pi Eval sessions</div>
<section class="grid">
<div class="card"><div class="muted">PID</div><strong id="pid">loading…</strong></div>
<div class="card"><div class="muted">Protocol</div><strong id="protocol">loading…</strong></div>
<div class="card"><div class="muted">Registered Pis</div><strong id="count">loading…</strong></div>
<div class="card"><div class="muted">Queued events</div><strong id="events">loading…</strong></div>
</section>
<div class="card"><h2>Registrations</h2><div id="registrations" class="muted">loading…</div></div>
<div class="card"><h2>Connection</h2><p class="muted">This dashboard is loopback-only and inherits the daemon key from the URL. MCP clients should use the <code>/mcp</code> URL from daemon.json or OAuth discovery.</p><pre id="oauth" class="muted">loading…</pre></div>
<script>
const esc=(v)=>String(v??"").replace(/[&<>"]/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;"}[c]));
async function refresh(){
 try{
  const data=await fetch("./api/status"+location.search,{cache:"no-store"}).then(r=>{if(!r.ok)throw new Error("HTTP "+r.status);return r.json()});
  document.querySelector("#pid").textContent=data.daemon?.pid??"—";
  document.querySelector("#protocol").textContent=data.daemon?.protocol??"—";
  document.querySelector("#count").textContent=(data.registrations??[]).length;
  document.querySelector("#events").textContent=data.queued_events??0;
  const rows=(data.registrations??[]).map(r=>"<tr><td><code>"+esc(r.binding_id)+"</code></td><td class=\"online\">"+esc(r.state??"online")+"</td><td>"+esc(r.session_id??"")+"</td><td>"+esc(r.cwd??"")+"</td></tr>").join("");
  document.querySelector("#registrations").innerHTML=rows? "<table><thead><tr><th>Binding</th><th>State</th><th>Session</th><th>CWD</th></tr></thead><tbody>"+rows+"</tbody></table>":"<div class=\"empty muted\">No Pi is currently registered.</div>";
  document.querySelector("#oauth").textContent=JSON.stringify(data.oauth??{enabled:false},null,2);
}catch(error){document.querySelector("#registrations").innerHTML="<div class=\"error\">"+esc(error.message)+"</div>"}
}
refresh();setInterval(refresh,3000);
</script></body></html>`;
}

export async function handleMcpevalWebRequest(
  request: IncomingMessage,
  response: ServerResponse,
  deps: McpevalWebDeps,
): Promise<boolean> {
  const pathname = new URL(request.url ?? "/", "http://127.0.0.1").pathname;
  if (request.method !== "GET") return false;
  const payload = {
    daemon: deps.descriptor(),
    registrations: deps.discover(),
    queued_events: deps.getEventQueueSize?.() ?? 0,
    oauth: deps.oauth?.() ?? { enabled: false },
  };
  if (pathname === "/api/status") {
    json(response, 200, payload);
    return true;
  }
  if (pathname === "/" || pathname === "/index.html") {
    html(response, renderMcpevalDashboard());
    return true;
  }
  return false;
}
