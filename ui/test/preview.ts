// Writes pictures of the interface on one device to .pocket-build/ui/preview/, over captures of the reference.
//   bun ui/test/preview.ts <vita|psp>     (after `bun tools/ui.ts <device>`)
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { BTN } from "../../vendor/pocketjs/contracts/spec/spec.ts";
import { boot, type Device } from "./harness.ts";

const device = (process.argv[2] ?? "vita") as Device;
const build = resolve(import.meta.dir, "../../.pocket-build/ui");
const out = join(build, "preview");
mkdirSync(out, { recursive: true });
const rig = await boot(device);
const backdrop = (name: string) => join(build, "backdrops", `${name}.png`);
const names: string[] = [];
const save = async (name: string, behind = "scene") => {
  const file = join(out, `${device}-${name}.png`);
  writeFileSync(file, (await rig.shot(backdrop(behind))).toBuffer("image/png"));
  names.push(file);
};
/** Chooses row `index` of the list that is up with the d-pad. */
const choose = (index: number) => {
  for (let i = 0; i < index; i++) rig.press(BTN.DOWN);
  rig.press(BTN.CIRCLE);
};
const back = () => rig.press(BTN.CROSS);

rig.step(20);
await save("title", "drone");
// The teleports, then the hours and what the game is made of, from the title.
choose(4);
rig.step(12);
await save("title-places", "drone");
back();
rig.step(6);
choose(5);
rig.step(12);
await save("title-hour", "drone");
back();
rig.step(6);
choose(7);
rig.step(10);
await save("about", "drone");
back();
rig.step(6);
// The taxi: the readouts as play begins, with the hint.
choose(0);
rig.mock.move(27, 0, 358, 4, 470);
rig.mock.at("Empire State Building", "5th Ave", "W 34th St");
rig.step(20);
await save("drive-start");
// Later, at night in Times Square, boosting.
rig.mock.state.hour = 3;
rig.step(460);
rig.mock.move(64, 0, 2, -581, -330, 1);
rig.mock.at("One Times Square & Duffy Sq", "7th Ave", "W 46th St");
rig.step(130);
await save("drive-night", "night");
// The helicopter over Midtown at blue hour, near a corner.
rig.mock.state.player = "helicopter";
rig.mock.state.hour = 2;
rig.mock.move(48, 315, 24, 30, 620);
rig.mock.at("Empire State Building", "5th Ave", "W 34th St", true);
rig.step(12);
await save("fly-note", "fly");
rig.step(150);
await save("fly", "fly");
// The menu over play, and the lists it opens.
rig.press(BTN.START);
rig.step(14);
await save("menu", "fly");
choose(1);
rig.step(10);
await save("menu-player", "fly");
back();
rig.step(6);
choose(3);
rig.step(6);
for (let i = 0; i < 8; i++) rig.press(BTN.DOWN);
rig.step(10);
await save("menu-camera", "fly");
back();
rig.step(6);
choose(5);
rig.step(10);
await save("settings", "fly");
back();
rig.step(6);
choose(6);
rig.step(10);
await save("controls", "fly");

const sheet = join(out, `sheet-${device}.png`);
Bun.spawnSync(["magick", "montage", ...names, "-tile", "3x", "-geometry", `${rig.view.density === 2 ? "50%x50%+4+4" : "+4+4"}`, "-background", "#333333", sheet]);
console.log(sheet);
console.log(JSON.stringify(rig.mock.log.filter((command) => !["drive", "look", "idle", "wake"].includes(command.type))));
