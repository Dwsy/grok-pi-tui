# eval-pi-mcp: binding-scoped Eval v2 MCP access

## Contract

`[ui].pi_eval_mcp` (default false, restart required) takes effect **only** when
the host-applied `[ui].pi_eval_v2_only` policy is active. CLI tool overrides or
disabled bridge extensions do not accidentally enable remote Eval. The existing
Pi runtime remains authoritative: MCP invokes the actual registered Eval v2
execute implementation, sharing its kernels, tool capture, skills, timeouts and
background-task semantics. Pi core and Grok Pager are not replaced.

On Pi `session_start`, a loopback-only Streamable HTTP MCP server is started via
the official TypeScript `@modelcontextprotocol/sdk`. A new random binding ID is
atomically reserved in `$GROK_HOME/eval-pi-mcp/issued-ids.json` under an exclusive
local `bindings.lock` file (`open(..., 'wx')`, private modes, bounded wait, atomic
tmp/rename). **Issued IDs are never recycled.** The URL's independent random
secret authorizes the HTTP connection; each tool call additionally requires
the exact `binding_id`. Secret comparison is constant-time. `Origin` is rejected
unless it matches a loopback origin. No remote interface is bound, no secret is
written to the ID registry, and MCP is disabled by default. Treat the URL as a
credential; any recipient can read session context and run Eval code.

Tools: `get_context(binding_id, limit?)` returns a bounded active-branch
transcript snapshot (not a claim to reproduce the exact private LLM prompt),
`get_desc(binding_id)` returns the Eval usage guide and captured Pi tool/skill
metadata, and `execute_eval(binding_id, language, code, title?, timeout?,
reset?, is_background?)` returns the real Eval result. A Pi `info` notification
prints the binding ID, authenticated MCP URL, and delivery guidance on entry.
Because that notification races the host's bootstrap window, the live binding is
additionally persisted to `$GROK_HOME/eval-pi-mcp/binding.json` (`bindingId`,
`url`, `pid`, `issuedAtMs`; 0600 in the 0700 binding dir) as the recoverable
source of truth, and cleared when the binding closes. Session shutdown closes
the server and invalidates the URL. A session switch rotates the binding and
secret; existing clients must be re-bound.

Any Eval image result, including ImageContent returned by nested Pi `read`
**without an explicit `display(image)` call**, is collected on the active Eval
cell and converted into a `resource_link` rather than placing its image block
base64 in tool text.
`resources/list` and `resources/read` expose bounded in-memory image blobs under
binding-scoped opaque URIs. Resources are private to the running Pi child and
disappear when the binding ends (at most 48 images / 64 MiB overall, 16 MiB per
image). No filesystem paths are accepted as resource
URIs, and all resource reads pass the same URL authentication.

## Token accounting with npm `tokenizers`

The MCP mode uses the actual `tokenizers` package's `Tokenizer.fromFile()`
and `Encoding.getIds().length`; it does **not** estimate counts from character
length. The tokenizer must match the intended model. To provision it in Pi's
npm tree and point it at the appropriate locally supplied tokenizer.json:

```sh
npm install --prefix ~/.pi/agent/npm tokenizers
export PI_GROK_EVAL_MCP_TOKENIZER_FILE=/absolute/path/to/tokenizer.json
```

Without the environment variable, the default file is
`$GROK_HOME/eval-pi-mcp/tokenizer.json` (normally
`~/.grok-pi/eval-pi-mcp/tokenizer.json`). Every MCP tool result exposes
`structuredContent.token_usage` and a text accounting footer with
`input_tokens`, `output_tokens`, and `total_tokens`. For `execute_eval`, input
is the serialized Eval language/code/options (excluding ID and URL secret) and
output is the Eval textual output (excluding image blobs and the accounting
footer). These are per-tool-call text payload counts, **not** provider-billed
chat/system/image tokens. When the package or JSON model is missing or encoding
fails, counts are `null`, `status=unavailable`, and `note` explains why.

## Acceptance

- Disabled / not Eval-v2-only never binds the port.
- Real SDK initialize/list/call/resources flows work with key and ID. Wrong
  secret, wrong ID, non-loopback origin and outdated binding fail closed.
- Concurrent processes reserve unique IDs under file lock; no recycled IDs.
- Result text and images preserve original Eval output semantics; image `read`
  returns MCP resource contents (base64/mimeType) via URI.
- Pi extension shutdown cleans up listener; existing Eval v2 regression remains green.

## Implementation notes

The SDK is resolved through Pi's installed npm runtime (or grok-pi's documented
Pi npm extension tree); this is a host dependency of the optional MCP mode, not
a change to Pi core or a vendored SDK. The extension is bundled into the Bash/Eval
injector so it has access to the same tool's execute closure and cannot drift.
