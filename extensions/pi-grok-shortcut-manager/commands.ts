import type { ExtensionAPI, ExtensionCommandContext } from "@earendil-works/pi-coding-agent";

import { loadConfig, saveConfig } from "./config.ts";
import {
  buildDiagnostics,
  formatKeyDisplay,
  refreshRegistry,
  shortExtName,
} from "./host.ts";
import { state } from "./shared.ts";
import type { ShortcutDiagnostic } from "./shared.ts";

export function registerShortcutCommand(pi: ExtensionAPI): void {
  pi.registerCommand("shortcuts", {
    description: "Manage extension shortcuts: /shortcuts [list|enable|disable|remap|diagnostics|on|off]",
    handler: async (args: string, ctx: ExtensionCommandContext) => {
      const parts = args.trim().split(/\s+/);
      const sub = parts[0] ?? "list";
      const key = parts[1];
      const newKey = parts[2];
      const config = loadConfig();

      switch (sub) {
        case "list": {
          refreshRegistry();
          const diagnostics = buildDiagnostics();
          if (diagnostics.length === 0) {
            ctx.ui.notify("No extension shortcuts registered.\nInstall extensions that call pi.registerShortcut() to see them here.", "info");
            return;
          }

          // Group by extension
          const byExt = new Map<string, ShortcutDiagnostic[]>();
          for (const d of diagnostics) {
            const name = shortExtName(d.extensionPath);
            if (!byExt.has(name)) byExt.set(name, []);
            byExt.get(name)!.push(d);
          }

          const sections: string[] = [];
          for (const [extName, items] of byExt) {
            const lines = items.map((d) => {
              const icon = d.enabled ? "●" : "○";
              const keyStr = formatKeyDisplay(d.key);
              const remap = d.remappedTo ? ` → ${formatKeyDisplay(d.remappedTo)}` : "";
              const conflict = d.conflictType !== "none" ? " ⚠" : "";
              return `  ${icon} ${keyStr}${remap}  ${d.description}${conflict}`;
            });
            sections.push(`${extName}\n${lines.join("\n")}`);
          }

          const globalStatus = config.globalEnabled ? "" : "\n⚠ Dispatch globally disabled (/shortcuts on)";
          ctx.ui.notify(
            `Extension shortcuts (${diagnostics.length})${globalStatus}\n\n${sections.join("\n\n")}`,
            "info",
          );
          return;
        }

        case "enable": {
          if (!key) { ctx.ui.notify("Usage: /shortcuts enable <key>\nExample: /shortcuts enable alt+t", "warning"); return; }
          const nk = key.toLowerCase();
          if (!state.shortcutRegistry.has(nk)) {
            ctx.ui.notify(`Unknown shortcut '${key}'. Run /shortcuts list to see registered shortcuts.`, "warning");
            return;
          }
          const existing = config.shortcuts[nk];
          config.shortcuts[nk] = {
            key: nk,
            description: existing?.description ?? state.shortcutRegistry.get(nk)?.description ?? "",
            extensionPath: existing?.extensionPath ?? state.shortcutRegistry.get(nk)?.extensionPath ?? "",
            enabled: true,
            remappedTo: existing?.remappedTo,
          };
          saveConfig(config);
          ctx.ui.notify(`● ${formatKeyDisplay(key)} enabled`, "info");
          return;
        }

        case "disable": {
          if (!key) { ctx.ui.notify("Usage: /shortcuts disable <key>\nExample: /shortcuts disable alt+t", "warning"); return; }
          const nk = key.toLowerCase();
          if (!state.shortcutRegistry.has(nk)) {
            ctx.ui.notify(`Unknown shortcut '${key}'. Run /shortcuts list to see registered shortcuts.`, "warning");
            return;
          }
          const existing = config.shortcuts[nk];
          config.shortcuts[nk] = {
            key: nk,
            description: existing?.description ?? state.shortcutRegistry.get(nk)?.description ?? "",
            extensionPath: existing?.extensionPath ?? state.shortcutRegistry.get(nk)?.extensionPath ?? "",
            enabled: false,
            remappedTo: existing?.remappedTo,
          };
          saveConfig(config);
          ctx.ui.notify(`○ ${formatKeyDisplay(key)} disabled`, "info");
          return;
        }

        case "remap": {
          if (!key || !newKey) {
            ctx.ui.notify("Usage: /shortcuts remap <old-key> <new-key>\nExample: /shortcuts remap alt+t alt+shift+t", "warning");
            return;
          }
          const nk = key.toLowerCase();
          if (!state.shortcutRegistry.has(nk)) {
            ctx.ui.notify(`Unknown shortcut '${key}'. Run /shortcuts list to see registered shortcuts.`, "warning");
            return;
          }
          // Check if new key conflicts with another extension shortcut
          const conflictTarget = [...state.shortcutRegistry.entries()].find(
            ([k]) => k === newKey.toLowerCase() && k !== nk,
          );
          if (conflictTarget) {
            ctx.ui.notify(
              `⚠ '${newKey}' is already used by ${shortExtName(conflictTarget[1].extensionPath)}. ` +
              `Remap anyway? Use /shortcuts remap ${key} ${newKey}! to force.`,
              "warning",
            );
            if (!parts.includes("!")) return;
          }
          const existing = config.shortcuts[nk];
          config.shortcuts[nk] = {
            key: nk,
            description: existing?.description ?? state.shortcutRegistry.get(nk)?.description ?? "",
            extensionPath: existing?.extensionPath ?? state.shortcutRegistry.get(nk)?.extensionPath ?? "",
            enabled: existing?.enabled ?? true,
            remappedTo: newKey.toLowerCase(),
          };
          saveConfig(config);
          ctx.ui.notify(`${formatKeyDisplay(key)} → ${formatKeyDisplay(newKey)}`, "info");
          return;
        }

        case "reset": {
          if (!key) { ctx.ui.notify("Usage: /shortcuts reset <key>\nRemoves remap and re-enables the shortcut.", "warning"); return; }
          const nk = key.toLowerCase();
          delete config.shortcuts[nk];
          saveConfig(config);
          ctx.ui.notify(`${formatKeyDisplay(key)} reset to default`, "info");
          return;
        }

        case "diagnostics": {
          refreshRegistry();
          const diagnostics = buildDiagnostics();
          const conflicts = diagnostics.filter((d) => d.conflictType !== "none");
          const disabled = diagnostics.filter((d) => !d.enabled);

          if (conflicts.length === 0 && disabled.length === 0) {
            ctx.ui.notify(`All ${diagnostics.length} extension shortcuts active, no conflicts.`, "info");
            return;
          }

          const lines: string[] = [];
          if (conflicts.length > 0) {
            lines.push("Conflicts:");
            for (const d of conflicts) {
              lines.push(`  ⚠ ${formatKeyDisplay(d.key)} — ${shortExtName(d.extensionPath)} conflicts with ${shortExtName(d.conflictWith ?? "")}`);
            }
          }
          if (disabled.length > 0) {
            lines.push("Disabled:");
            for (const d of disabled) {
              lines.push(`  ○ ${formatKeyDisplay(d.key)} — ${d.description}`);
            }
          }
          ctx.ui.notify(lines.join("\n"), "warning");
          return;
        }

        case "on":
          config.globalEnabled = true;
          saveConfig(config);
          ctx.ui.notify("Extension shortcut dispatch enabled", "info");
          return;

        case "off":
          config.globalEnabled = false;
          saveConfig(config);
          ctx.ui.notify("Extension shortcut dispatch disabled.\nAll extension shortcuts are inactive. Use /shortcuts on to re-enable.", "info");
          return;

        default:
          ctx.ui.notify(
            "Usage: /shortcuts <command>\n\n" +
            "  list          Show all extension shortcuts\n" +
            "  enable <key>  Enable a shortcut\n" +
            "  disable <key> Disable a shortcut\n" +
            "  remap <old> <new>  Remap a shortcut\n" +
            "  reset <key>   Remove remap, re-enable\n" +
            "  diagnostics   Show conflicts and issues\n" +
            "  on / off      Global enable/disable",
            "info",
          );
      }
    },
  });
}
