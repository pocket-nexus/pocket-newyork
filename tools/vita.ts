// Pocket New York on PS Vita: build the native runtime, replace the running development container's binary over
// PocketJS's wired debug transport (vendor/pocketjs), sync the pack and the interface, steer and measure.
//
//   bun tools/vita.ts build   [--title P25BFE5E2] [--debug]
//   bun tools/vita.ts sync                        # pack and compiled interface → host0:newyork/
//   bun tools/vita.ts native                      # sync + build + USB SELF replacement
//   bun tools/vita.ts serve                       # USB host (keep running, detached)
//   bun tools/vita.ts status | capture [--out f.png]
//   bun tools/vita.ts ctl '{"hour":3}'            # host0:newyork/control.json
//   bun tools/vita.ts boot '{"title":false}'      # host0:newyork/boot.json, read at start
//
// `--share DIR` (or NY_SHARE) uses an already-running USB host's root directory instead of this repository's
// `.pocket-build/vita-usb/share`. The default title is Pocket Devkit (PocketJS apps/devkit), the development
// container installed on the console (title P25BFE5E2): its native slots accept replacement SELFs.

import { $ } from "bun";
import { createHash, randomBytes } from "node:crypto";
import { cpSync, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { packageVitaVpk } from "../vendor/pocketjs/tools/vita-package.ts";
import { prepareVitaUsb } from "../vendor/pocketjs/tools/vita-usb.ts";

const ROOT = resolve(import.meta.dir, "..");
const POCKETJS = resolve(ROOT, "vendor/pocketjs");
const APP_DIR = resolve(ROOT, "vita");
const OUT_DIR = resolve(ROOT, "dist/vita");
const home = process.env.HOME ?? "";
const vitasdk = process.env.VITASDK || `${home}/vitasdk`;
const rustup = Bun.which("rustup") ?? `${home}/.cargo/bin/rustup`;
const BIN = "pocket-newyork-vita";
const DEVKIT = "P25BFE5E2";
const INTERFACE = ["newyork.js", "newyork.pak"];

function value(argv: string[], flag: string, fallback: string): string {
  const i = argv.indexOf(flag);
  return i >= 0 && argv[i + 1] ? argv[i + 1]! : fallback;
}

export function packPath(argv: string[]): string {
  return resolve(ROOT, ".pocket-build/scene", value(argv, "--profile", "vita60"), "city.pack");
}

export function context(argv: string[]) {
  const standalone = argv.includes("--standalone");
  const title = value(argv, "--title", standalone ? "PKNY00001" : DEVKIT);
  const share = resolve(value(argv, "--share", process.env.NY_SHARE ?? resolve(ROOT, ".pocket-build/vita-usb/share")));
  return { title, share, appShare: resolve(share, "newyork"), output: `pocket-newyork-${title}`, release: !argv.includes("--debug"), standalone };
}

export async function build(argv: string[], assets?: string): Promise<string> {
  const c = context(argv);
  if (!existsSync(`${vitasdk}/bin/vita-pack-vpk`)) throw new Error(`VitaSDK not found at ${vitasdk}`);
  const usb = c.standalone ? undefined : await prepareVitaUsb();
  const nativeBuild = randomBytes(16).toString("hex");
  const env = {
    ...process.env,
    PATH: `${vitasdk}/bin:${home}/.cargo/bin:${process.env.PATH ?? ""}`,
    VITASDK: vitasdk,
    VITA_DEFAULT_TITLE_ID: c.title,
    POCKETJS_VITA_TITLE_ID: c.title,
    POCKETJS_NATIVE_BUILD: nativeBuild,
    POCKETJS_EMBED_APP: "0",
    TARGET_AR: "arm-vita-eabi-ar",
    AR_armv7_sony_vita_newlibeabihf: "arm-vita-eabi-ar",
    TARGET_CC: "arm-vita-eabi-gcc",
    CC_armv7_sony_vita_newlibeabihf: "arm-vita-eabi-gcc",
    TARGET_CXX: "arm-vita-eabi-g++",
    CXX_armv7_sony_vita_newlibeabihf: "arm-vita-eabi-g++",
  };
  const cargoArgs = [...(c.release ? ["--release"] : []), ...(c.standalone ? ["--no-default-features"] : [])];
  console.log(`newyork: cargo vita build vpk (title ${c.title}, ${c.release ? "release" : "debug"})`);
  await $`${rustup} run nightly-2026-05-28 cargo vita build vpk -- ${cargoArgs}`.cwd(APP_DIR).env(env);

  const target = `${APP_DIR}/target/armv7-sony-vita-newlibeabihf/${c.release ? "release" : "debug"}`;
  const eboot = `${target}/${BIN}.self`;
  const sfo = `${target}/${BIN}.sfo`;
  const vpk = `${target}/${BIN}.vpk`;
  // Unsafe-homebrew SELF: loading the USB driver and writing the inactive native slot need the standard permissions.
  await $`${vitasdk}/bin/vita-make-fself ${target}/${BIN}.velf ${eboot}`;
  await $`${vitasdk}/bin/vita-mksfoex -d ATTRIBUTE2=12 -s TITLE_ID=${c.title} ${"Pocket New York"} ${sfo}`;
  // The bubble's icon is Pocket3D's (skills/pocket3d-brand); the tree holds no icon of its own.
  const iconModule: string = `${POCKETJS}/tools/pocket3d-icon.ts`;
  const { POCKET3D_ICON } = (await import(iconModule)) as { POCKET3D_ICON: { vita: string } };
  await packageVitaVpk({ tool: `${vitasdk}/bin/vita-pack-vpk`, sfo, eboot, output: vpk, usbDriver: usb?.driver, applicationAssets: assets ?? (existsSync(`${APP_DIR}/assets`) ? `${APP_DIR}/assets` : undefined), icon: POCKET3D_ICON.vita });

  mkdirSync(OUT_DIR, { recursive: true });
  cpSync(vpk, `${OUT_DIR}/${c.output}.vpk`);
  cpSync(eboot, `${OUT_DIR}/${c.output}.self`);
  const selfSha256 = createHash("sha256").update(readFileSync(eboot)).digest("hex");
  const runtime = `${OUT_DIR}/${c.output}.runtime.json`;
  await Bun.write(runtime, JSON.stringify({ version: 1, titleId: c.title, applicationId: "dev.pocket-nexus.newyork", output: c.output, nativeBuild, plan: null, self: `${c.output}.self`, usbDebug: !c.standalone, usbDriver: usb?.fingerprint ?? null, selfSha256 }, null, 2) + "\n");
  console.log(`newyork: ${OUT_DIR}/${c.output}.self (native build ${nativeBuild})`);
  return runtime;
}

/** PocketJS's wired debug tool, pointed at the chosen USB share. */
export async function dev(argv: string[], ...args: string[]): Promise<void> {
  const c = context(argv);
  mkdirSync(c.share, { recursive: true });
  const runtime = `${OUT_DIR}/${c.output}.runtime.json`;
  const r = existsSync(runtime) ? ["--runtime", runtime] : [];
  await $`bun ${POCKETJS}/tools/vita-dev.ts ${args} ${r} --title ${c.title} --dir ${c.share}`.cwd(POCKETJS);
}

/**
 * Copies to the share what the device reads there: the pack (with `city.json` beside it, which the device compares
 * with the record on its memory card before copying the pack across) and the compiled interface. A file is copied
 * when its bytes differ from the share's copy.
 */
export async function sync(argv: string[]): Promise<void> {
  const c = context(argv);
  // The device never creates directories on host0:; every directory it writes into exists up front.
  for (const dir of ["", "gxp", "outbox"]) mkdirSync(`${c.appShare}/${dir}`, { recursive: true });
  const sha = (p: string) => createHash("sha256").update(readFileSync(p)).digest("hex");
  const pack = packPath(argv);
  if (!existsSync(pack)) throw new Error(`no pack at ${pack}: run \`bun tools/newyork.ts cook\` first`);
  const id = JSON.stringify({ sha256: sha(pack), bytes: statSync(pack).size });
  const record = `${c.appShare}/city.json`;
  if (!existsSync(record) || readFileSync(record, "utf8") !== id || !existsSync(`${c.appShare}/city.pack`)) {
    cpSync(pack, `${c.appShare}/city.pack`);
    writeFileSync(record, id);
    console.log(`newyork: synced city.pack (${(statSync(pack).size / 1e6).toFixed(1)} MB)`);
  }
  await $`bun ${ROOT}/tools/ui.ts vita`.cwd(ROOT).quiet();
  for (const name of INTERFACE) {
    const [src, dst] = [resolve(ROOT, ".pocket-build/ui/vita", name), `${c.appShare}/${name}`];
    if (existsSync(dst) && sha(dst) === sha(src)) continue;
    cpSync(src, dst);
    console.log(`newyork: synced ${name}`);
  }
}

export function ctl(argv: string[], json: string): void {
  const c = context(argv);
  mkdirSync(c.appShare, { recursive: true });
  JSON.parse(json);
  writeFileSync(`${c.appShare}/control.json`, json);
}

export function boot(argv: string[], json: string): void {
  const c = context(argv);
  mkdirSync(c.appShare, { recursive: true });
  JSON.parse(json);
  writeFileSync(`${c.appShare}/boot.json`, json);
}

/** The running process's status receipt, straight from the share. */
export function status(argv: string[]): any {
  const c = context(argv);
  // The device replaces the file every few frames; a read can land between the old one and the new one.
  for (let i = 0; ; i++) {
    try {
      return JSON.parse(readFileSync(`${c.share}/pocket-vita/${c.title}/status.json`, "utf8"));
    } catch (e) {
      if (i >= 20) throw e;
      Bun.sleepSync(25);
    }
  }
}

if (import.meta.main) {
  const [command, ...argv] = process.argv.slice(2);
  switch (command) {
    case "build":
      await build(argv);
      break;
    case "sync":
      await sync(argv);
      break;
    case "native":
      await sync(argv);
      await build(argv);
      await dev(argv, "native");
      break;
    case "serve":
      await dev(argv, "serve");
      break;
    case "status":
      console.log(JSON.stringify(status(argv), null, 1));
      break;
    case "capture": {
      const out = value(argv, "--out", resolve(ROOT, ".pocket-build/validation/vita", `${new Date().toISOString().replace(/[:.]/g, "-")}.png`));
      mkdirSync(resolve(out, ".."), { recursive: true });
      await dev(argv, "capture", "--out", resolve(out));
      break;
    }
    case "ctl":
      ctl(argv, argv.find((a) => a.startsWith("{")) ?? "{}");
      break;
    case "boot":
      boot(argv, argv.find((a) => a.startsWith("{")) ?? "{}");
      break;
    default:
      console.error("usage: bun tools/vita.ts build | sync | native | serve | status | capture | ctl JSON | boot JSON");
      process.exit(1);
  }
}
