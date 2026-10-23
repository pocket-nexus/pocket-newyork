// Drives one device's bundle through the title, play and the menu against the mock renderer and checks what it
// asked for.
//   bun ui/test/flow.ts <vita|psp>     (after `bun tools/ui.ts <device>`)
import assert from "node:assert/strict";
import { BTN } from "../../vendor/pocketjs/contracts/spec/spec.ts";
import type { Command } from "../app/protocol.ts";
import { boot, type Device } from "./harness.ts";

const device = (process.argv[2] ?? "vita") as Device;
const rig = await boot(device);
const mock = rig.mock;
/** The commands since the last call, without the streams and the guest's pacing. */
let read = 0;
const asked = (): Command[] => {
  const all = mock.log.slice(read).filter((command) => !["drive", "look", "idle", "wake"].includes(command.type));
  read = mock.log.length;
  return all;
};
/** Chooses row `index` of the list that is up with the d-pad (from its first row, or from the current choice). */
const choose = (index: number, from = 0) => {
  for (let i = from; i < index; i++) rig.press(BTN.DOWN);
  rig.press(BTN.CIRCLE);
  rig.step(4);
};

// What was stored comes back: the settings are told again.
mock.state.prefs = JSON.stringify({ options: { invert: 1 } });
rig.step(10);
assert.deepEqual(asked(), [{ type: "option", key: "invert", value: 1 }]);
assert.equal(mock.state.options[1].value, 1);

// The hours, from the title: night is chosen, and the list stays to show it.
choose(5);
choose(3, 0);
assert.deepEqual(asked(), [{ type: "hour", index: 3 }]);
assert.equal(mock.state.hour, 3);
rig.press(BTN.CROSS);
rig.step(6);

// A place from the title is where play begins, in the taxi the title last showed.
choose(4);
choose(1);
assert.deepEqual(asked(), [{ type: "place", index: 1 }, { type: "start", player: "car" }]);
assert.equal(mock.state.mode, "play");
assert.equal(mock.state.camera, 0);
// The hint fades in, stands with nothing scheduled (the renderer counts its seconds), and fades out.
assert.equal(mock.idle, false);
rig.step(60);
assert.equal(mock.idle, true);
assert.equal(mock.state.woke, 0);
rig.step(368);
assert.notEqual(mock.state.woke, 0);
assert.equal(mock.idle, false);
rig.step(60);
assert.equal(mock.idle, true);
// A new camera is said for a moment.
mock.state.camera = 1;
rig.step(2);
assert.equal(mock.idle, false);
rig.step(200);
assert.equal(mock.idle, true);
// Numbers and a new corner come and go without a turn of their own being scheduled.
mock.move(31, 0, 90, -581, -330);
mock.at("", "7th Ave", "W 45th St");
rig.step(30);
assert.equal(mock.idle, true);
assert.deepEqual(asked(), []);

// The menu: another way through the city, which goes back to it.
rig.press(BTN.START);
rig.step(8);
assert.deepEqual(asked(), [{ type: "menu", on: true }]);
assert.equal(mock.state.mode, "menu");
choose(1);
// The list opens on the current mode (the taxi, the second row); the helicopter is two below.
choose(3, 1);
assert.deepEqual(asked(), [{ type: "player", id: "helicopter" }, { type: "menu", on: false }]);
assert.equal(mock.state.mode, "play");
assert.equal(mock.state.player, "helicopter");

// A camera from the menu is seen behind the list, which stays; ✕ goes back, and again resumes.
rig.press(BTN.START);
rig.step(8);
choose(3);
choose(4, 1);
assert.deepEqual(asked(), [{ type: "menu", on: true }, { type: "camera", index: 4 }]);
assert.equal(mock.state.camera, 4);
rig.press(BTN.CROSS);
rig.step(6);
rig.press(BTN.CROSS);
rig.step(6);
assert.deepEqual(asked(), [{ type: "menu", on: false }]);
assert.equal(mock.state.mode, "play");

// Settings: the renderer's own, and the readouts, which are the interface's; both are kept.
rig.press(BTN.START);
rig.step(8);
choose(5);
choose(3);
assert.deepEqual(asked(), [{ type: "menu", on: true }, { type: "option", key: "stats", value: 1 }, { type: "prefs", value: JSON.stringify({ options: { invert: 1, stats: 1 } }) }]);
assert.ok(mock.state.stats.length > 0);
rig.press(BTN.UP);
rig.press(BTN.UP);
rig.press(BTN.UP);
rig.press(BTN.CIRCLE);
rig.step(4);
assert.deepEqual(asked(), [{ type: "prefs", value: JSON.stringify({ options: { invert: 1, stats: 1 }, hud: 0 }) }]);
rig.press(BTN.CROSS);
rig.step(6);

// START closes the menu it opened, and the title is its last row.
rig.press(BTN.START);
rig.step(8);
assert.deepEqual(asked(), [{ type: "menu", on: false }]);
rig.press(BTN.START);
rig.step(8);
choose(7);
rig.step(6);
assert.deepEqual(asked(), [{ type: "menu", on: true }, { type: "title" }]);
assert.equal(mock.state.mode, "title");
// From the title the helicopter goes up again where it was, on the chase camera: play had a scenic one.
choose(3);
assert.deepEqual(asked(), [{ type: "start", player: "helicopter" }]);
assert.equal(mock.state.camera, 0);
console.log(`${device}: the interface asked for what the flow expects`);
