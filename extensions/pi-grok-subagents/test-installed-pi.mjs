/**
 * Resolve extension tests against an explicitly selected installed Pi package.
 * This keeps missing generated data in the optional pi-main checkout out of the
 * test process without changing repository dependencies or production loading.
 *
 * PI_TEST_NODE_MODULES=/absolute/release/node_modules \
 *   node --import ./test-installed-pi.mjs --test *.test.ts
 */
import { existsSync } from "node:fs";
import { registerHooks } from "node:module";
import { dirname, isAbsolute, join, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const modulesRoot = process.env.PI_TEST_NODE_MODULES;
if (!modulesRoot || !isAbsolute(modulesRoot)
  || !existsSync(join(modulesRoot, "@earendil-works/pi-coding-agent/package.json"))) {
  throw new Error("Set PI_TEST_NODE_MODULES to the absolute node_modules directory of an installed Pi release.");
}
const sourceRoot = dirname(fileURLToPath(import.meta.url)) + sep;
const installedParentURL = pathToFileURL(join(dirname(modulesRoot), "__pi_subagents_test__.mjs")).href;

registerHooks({
  resolve(specifier, context, nextResolve) {
    const piPackage = specifier.startsWith("@earendil-works/") || specifier === "typebox";
    const localCaller = context.parentURL?.startsWith("file:")
      && fileURLToPath(context.parentURL).startsWith(sourceRoot);
    if (piPackage && localCaller) {
      return nextResolve(specifier, { ...context, parentURL: installedParentURL });
    }
    return nextResolve(specifier, context);
  },
});
