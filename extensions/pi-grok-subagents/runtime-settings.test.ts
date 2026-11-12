/**
 * Runtime settings store tests: validation, persistence, default fallback, and
 * the Pi TUI editor for the background concurrency.
 */

import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import {
  backgroundConcurrency,
  editBackgroundConcurrency,
  loadRuntimeSettings,
  MAX_BACKGROUND_CONCURRENCY_LIMIT,
  normalizeBackgroundConcurrency,
  runtimeSettingsPath,
  saveRuntimeSettings,
} from "./runtime-settings.ts";
import { MAX_BACKGROUND_CONCURRENCY } from "./shared.ts";
import { fakeCommandContext, fakeCustomUi, settle } from "./test-support.ts";

test("normalizeBackgroundConcurrency accepts bounded integers and rejects junk", () => {
  assert.equal(normalizeBackgroundConcurrency(undefined), undefined);
  assert.equal(normalizeBackgroundConcurrency(""), undefined);
  assert.equal(normalizeBackgroundConcurrency(0), undefined);
  assert.equal(normalizeBackgroundConcurrency(-3), undefined);
  assert.equal(normalizeBackgroundConcurrency(1.5), undefined);
  assert.equal(normalizeBackgroundConcurrency("abc"), undefined);
  assert.equal(normalizeBackgroundConcurrency(10), 10);
  assert.equal(normalizeBackgroundConcurrency("10"), 10);
  assert.equal(normalizeBackgroundConcurrency(10_000), MAX_BACKGROUND_CONCURRENCY_LIMIT);
});

test("settings round-trip, default fallback, and corrupt-file recovery", () => {
  const previous = process.env.GROK_HOME;
  const home = mkdtempSync(join(tmpdir(), "pi-grok-settings-"));
  process.env.GROK_HOME = home;
  try {
    assert.deepEqual(loadRuntimeSettings(), {});
    assert.equal(backgroundConcurrency(), MAX_BACKGROUND_CONCURRENCY);

    saveRuntimeSettings({ backgroundConcurrency: 7 });
    assert.deepEqual(loadRuntimeSettings(), { backgroundConcurrency: 7 });
    assert.equal(backgroundConcurrency(), 7);

    writeFileSync(runtimeSettingsPath(), "{not json");
    assert.deepEqual(loadRuntimeSettings(), {});
    assert.equal(backgroundConcurrency(), MAX_BACKGROUND_CONCURRENCY);
  } finally {
    if (previous === undefined) delete process.env.GROK_HOME;
    else process.env.GROK_HOME = previous;
    rmSync(home, { recursive: true, force: true });
  }
});

test("editBackgroundConcurrency persists a validated value through the Pi TUI input", async () => {
  const previous = process.env.GROK_HOME;
  const home = mkdtempSync(join(tmpdir(), "pi-grok-settings-"));
  process.env.GROK_HOME = home;
  try {
    const fake = fakeCustomUi();
    const pending = editBackgroundConcurrency(fakeCommandContext(fake) as never);
    await settle();

    fake.opened[0].keys("\x15"); // clear the initial value
    fake.opened[0].keys("abc");
    fake.opened[0].keys("\r"); // rejected by validation
    assert.match(fake.opened[0].renderText(), /integer between/);
    assert.equal(fake.opened[0].resolved, false);

    fake.opened[0].keys("\x15");
    fake.opened[0].keys("7");
    fake.opened[0].keys("\r");
    await pending;
    assert.deepEqual(loadRuntimeSettings(), { backgroundConcurrency: 7 });
    assert.deepEqual(fake.notifications.at(-1), {
      message: "Background concurrency set to 7.",
      kind: "info",
    });
  } finally {
    if (previous === undefined) delete process.env.GROK_HOME;
    else process.env.GROK_HOME = previous;
    rmSync(home, { recursive: true, force: true });
  }
});

test("cancelling the concurrency editor leaves settings untouched", async () => {
  const previous = process.env.GROK_HOME;
  const home = mkdtempSync(join(tmpdir(), "pi-grok-settings-"));
  process.env.GROK_HOME = home;
  try {
    const fake = fakeCustomUi();
    const pending = editBackgroundConcurrency(fakeCommandContext(fake) as never);
    await settle();
    fake.opened[0].keys("\x1b");
    await pending;
    assert.deepEqual(loadRuntimeSettings(), {});
  } finally {
    if (previous === undefined) delete process.env.GROK_HOME;
    else process.env.GROK_HOME = previous;
    rmSync(home, { recursive: true, force: true });
  }
});
