/**
 * Pi-tui configuration primitive tests: keyboard and mouse paths for the
 * full-screen panels that back every subagent configuration surface.
 */

import assert from "node:assert/strict";
import test from "node:test";
import { fakeCommandContext, fakeCustomUi, settle, type FakeCustomUi } from "./test-support.ts";
import { piConfirm, piEditor, piInput, piMultiSelect, piSelect } from "./ui-primitives.ts";

function lineOf(fake: FakeCustomUi, needle: string): number {
  const index = fake.opened[0].renderText().split("\n").findIndex((line) => line.includes(needle));
  assert.notEqual(index, -1, `expected a rendered line containing ${needle}`);
  return index;
}

test("piSelect keyboard moves down and Enter selects", async () => {
  const fake = fakeCustomUi();
  const pending = piSelect(fakeCommandContext(fake) as never, "Pick one", ["alpha", "beta"]);
  await settle();
  assert.match(fake.opened[0].renderText(), /Pick one/);
  fake.opened[0].keys("\x1b[B");
  fake.opened[0].keys("\r");
  assert.equal(await pending, "beta");
});

test("piSelect mouse click selects the clicked row", async () => {
  const fake = fakeCustomUi();
  const pending = piSelect(fakeCommandContext(fake) as never, "Pick one", ["alpha", "beta"]);
  await settle();
  fake.opened[0].click(3, lineOf(fake, "beta"));
  assert.equal(await pending, "beta");
});

test("piSelect Escape cancels", async () => {
  const fake = fakeCustomUi();
  const pending = piSelect(fakeCommandContext(fake) as never, "Pick one", ["alpha", "beta"]);
  await settle();
  fake.opened[0].keys("\x1b");
  assert.equal(await pending, undefined);
});

test("piMultiSelect keyboard toggles, Done returns the selection", async () => {
  const fake = fakeCustomUi();
  const pending = piMultiSelect(
    fakeCommandContext(fake) as never,
    "Tools",
    [{ label: "Tools", items: [{ value: "read", label: "read" }, { value: "bash", label: "bash" }] }],
  );
  await settle();
  fake.opened[0].keys("\r"); // toggle read on
  fake.opened[0].keys("\x1b[B");
  fake.opened[0].keys("\r"); // toggle bash on
  const done = lineOf(fake, "✓ Done");
  fake.opened[0].keys("\x1b[B");
  fake.opened[0].keys("\r");
  assert.deepEqual(await pending, ["read", "bash"]);
  assert.ok(done > 0);
});

test("piMultiSelect enforces maxSelections and keeps the rest off", async () => {
  const fake = fakeCustomUi();
  const pending = piMultiSelect(
    fakeCommandContext(fake) as never,
    "Models",
    [{ label: "Models", items: [{ value: "m1", label: "m1" }, { value: "m2", label: "m2" }] }],
    { maxSelections: 1 },
  );
  await settle();
  fake.opened[0].keys("\r"); // m1 on
  fake.opened[0].keys("\x1b[B");
  fake.opened[0].keys("\r"); // m2 rejected
  assert.match(fake.opened[0].renderText(), /Select at most 1 items/);
  fake.opened[0].keys("\x1b[B"); // Done row
  fake.opened[0].keys("\r");
  assert.deepEqual(await pending, ["m1"]);
});

test("piMultiSelect mouse click toggles rows and the Done row confirms", async () => {
  const fake = fakeCustomUi();
  const pending = piMultiSelect(
    fakeCommandContext(fake) as never,
    "Tools",
    [{ label: "Tools", items: [{ value: "read", label: "read" }, { value: "bash", label: "bash" }] }],
  );
  await settle();
  fake.opened[0].click(3, lineOf(fake, "bash"));
  fake.opened[0].click(3, lineOf(fake, "✓ Done"));
  assert.deepEqual(await pending, ["bash"]);
});

test("piMultiSelect Escape cancels", async () => {
  const fake = fakeCustomUi();
  const pending = piMultiSelect(
    fakeCommandContext(fake) as never,
    "Tools",
    [{ label: "Tools", items: [{ value: "read", label: "read" }] }],
  );
  await settle();
  fake.opened[0].keys("\x1b");
  assert.equal(await pending, undefined);
});

test("piInput submits typed text and blocks invalid values", async () => {
  const fake = fakeCustomUi();
  const pending = piInput(fakeCommandContext(fake) as never, "Name", {
    validate: (value) => (value.trim() ? undefined : "Name is required."),
  });
  await settle();
  fake.opened[0].keys("\r"); // empty submit is rejected
  assert.match(fake.opened[0].renderText(), /Name is required/);
  fake.opened[0].keys("crew");
  fake.opened[0].keys("\r");
  assert.equal(await pending, "crew");
});

test("piInput Escape cancels without a value", async () => {
  const fake = fakeCustomUi();
  const pending = piInput(fakeCommandContext(fake) as never, "Name", { initial: "draft" });
  await settle();
  fake.opened[0].keys("\x1b");
  assert.equal(await pending, undefined);
});

test("piConfirm resolves true on the Confirm row click and false on Escape", async () => {
  const fake = fakeCustomUi();
  const pending = piConfirm(fakeCommandContext(fake) as never, "Remove", "Remove member?");
  await settle();
  fake.opened[0].click(3, lineOf(fake, "✓ Confirm"));
  assert.equal(await pending, true);

  const other = fakeCustomUi();
  const pendingCancel = piConfirm(fakeCommandContext(other) as never, "Remove", "Remove member?");
  await settle();
  other.opened[0].keys("\x1b");
  assert.equal(await pendingCancel, false);
});

test("piEditor Escape keeps the edited text", async () => {
  const fake = fakeCustomUi();
  const pending = piEditor(fakeCommandContext(fake) as never, "Instructions", "base");
  await settle();
  fake.opened[0].keys(" more");
  fake.opened[0].keys("\x1b");
  assert.equal(await pending, "base more");
});
