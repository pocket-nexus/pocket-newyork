// Runs a compiled interface bundle on the host: PocketJS's wasm core and
// software rasterizer, with a renderer that exists only as state (`Mock`).
// It answers commands the way a device's renderer does, so a test can press
// buttons and touch the panel and read what the interface drew and asked for.
import { existsSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { createCanvas, ImageData, loadImage, type Canvas } from "../../vendor/pocketjs/node_modules/@napi-rs/canvas";
import { PROP } from "../../vendor/pocketjs/contracts/spec/spec.ts";
import { __packTouch, __packTouchWide, createTouchHitFacts } from "../../vendor/pocketjs/framework/src/touch.ts";
import { createWasmUi } from "../../vendor/pocketjs/hosts/web/wasm-ops.js";
import type { Command, HostState } from "../app/protocol.ts";

const root = resolve(import.meta.dir, "../..");
export type Device = "vita" | "psp";

/** A device's renderer, reduced to the state the interface sees. */
export class Mock {
  state: HostState = {
    mode: "title", message: "", player: "car", hour: 0, camera: 10, place: "", streets: ["", ""], near: false,
    options: [{ key: "traffic", value: 1 }, { key: "invert", value: 0 }, { key: "stats", value: 0 }],
    stats: "", prefs: "", woke: 0, t: [0, 0, 0, 3.6, 380, 0],
  };
  /** Every command received, oldest first. */
  log: Command[] = [];
  drive = { mx: 0, my: 0, b: 0 };
  /** The interface said it has nothing scheduled. */
  idle = false;
  looked = { dx: 0, dy: 0 };
  /** The camera play had when the title took the screen (the title shows the city through the drone's). */
  private playCamera = 0;
  /** Seconds this renderer has run, and what the interface asked to be woken for. */
  private seconds = 0;
  private wakes: { id: number; due: number }[] = [];
  private sent: Partial<HostState> = {};

  /** One frame of the renderer's clock: a wake that is due is said back. */
  tick(dt: number) {
    this.seconds += dt;
    const at = this.wakes.findIndex((wake) => wake.due <= this.seconds);
    if (at >= 0) this.state.woke = this.wakes.splice(at, 1)[0].id;
  }

  receive(command: Command) {
    this.log.push(command);
    const s = this.state;
    switch (command.type) {
      case "start":
        if (s.mode === "title") Object.assign(s, { mode: "play", player: command.player, camera: this.playCamera < 3 ? this.playCamera : 0 });
        break;
      case "menu":
        if (s.mode === "play" && command.on) s.mode = "menu";
        else if (s.mode === "menu" && !command.on) s.mode = "play";
        break;
      case "player": Object.assign(s, { player: command.id, camera: s.camera < 3 || s.mode === "title" ? s.camera : 0 }); break;
      case "hour": s.hour = command.index; break;
      case "camera": if (s.mode === "title") this.playCamera = command.index; else s.camera = command.index; break;
      case "place": if (s.mode !== "title") s.camera = 0; else this.playCamera = 0; break;
      case "title":
        if (s.mode !== "title") this.playCamera = s.camera;
        Object.assign(s, { mode: "title", camera: 10 });
        break;
      case "option":
        s.options = s.options.map((setting) => (setting.key === command.key ? { ...setting, value: command.value } : setting));
        s.stats = s.options.find((setting) => setting.key === "stats")?.value ? "60.0 fps · 16.7 ms · late 0 · 412 draws · 168k tris" : "";
        break;
      case "drive": this.drive = { mx: command.mx, my: command.my, b: command.b }; break;
      case "look": this.looked = { dx: this.looked.dx + command.dx, dy: this.looked.dy + command.dy }; break;
      case "prefs": s.prefs = command.value; break;
      case "idle": this.idle = command.on; break;
      case "wake": this.wakes.push({ id: command.id, due: this.seconds + command.seconds }); break;
    }
  }

  /** The player in play: the numbers of one moment. */
  move(mph: number, altitude: number, heading: number, x: number, z: number, boost = 0) {
    this.state.t = [mph, altitude, heading, x, z, boost];
  }

  /** Where the player is, as the device names it. */
  at(place: string, avenue: string, street: string, near = false) {
    Object.assign(this.state, { place, streets: [avenue, street], near });
  }

  /** The line of state members to deliver, or undefined while nothing changed. */
  poll(): string | undefined {
    const changed: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(this.state)) {
      const text = JSON.stringify(value);
      if (JSON.stringify((this.sent as Record<string, unknown>)[key]) === text) continue;
      changed[key] = (this.sent as Record<string, unknown>)[key] = JSON.parse(text);
    }
    return Object.keys(changed).length ? JSON.stringify({ type: "state", value: changed }) : undefined;
  }
}

const VIEW: Record<Device, { w: number; h: number; density: number; aux?: [number, number] }> = {
  vita: { w: 480, h: 272, density: 2 },
  psp: { w: 480, h: 272, density: 1 },
};

export interface Rig {
  mock: Mock;
  view: (typeof VIEW)[Device];
  /** Advance `frames`, holding `buttons`; `touch` is the contacts on the touch surface. */
  step(frames?: number, input?: { buttons?: number; touch?: { x: number; y: number; id?: number }[] }): void;
  /** Press and release. */
  press(buttons: number): void;
  tap(x: number, y: number): void;
  /** The primary screen (and the auxiliary one under it) over `backdrop`, at the device's density. */
  shot(backdrop?: string): Promise<Canvas>;
}

/** Boots the bundle `bun tools/ui.ts <device>` wrote. One per process: the bundle owns `globalThis.frame`. */
export async function boot(device: Device): Promise<Rig> {
  const view = VIEW[device];
  const directory = join(root, ".pocket-build/ui", device);
  const wasmPath = join(root, "vendor/pocketjs/hosts/web/pocketjs.wasm");
  if (!existsSync(wasmPath)) throw new Error("run `bun tools/wasm.ts` in vendor/pocketjs first");
  const wasm = await createWasmUi(readFileSync(wasmPath), { width: view.w, height: view.h, rasterDensity: view.density });
  if (view.aux) wasm.createAuxiliarySurface(view.aux[0], view.aux[1]);
  const mock = new Mock();
  const globals = globalThis as Record<string, any>;
  const pending: string[] = [];
  Object.assign(wasm.ops, {
    svcOpen: (name: string) => name === "pocket.overlay",
    svcPoll: () => mock.poll(),
    svcSend: (line: string) => pending.push(line),
  });
  globals.ui = wasm.ops;
  globals.__pak = readFileSync(join(directory, "newyork.pak")).buffer;
  (0, eval)(readFileSync(join(directory, "newyork.js"), "utf8"));

  const ops = wasm.ops as any;
  const facts = createTouchHitFacts((x, y) => (view.aux ? ops.hitTestBoundsAuxiliary(x, y) : ops.hitTestBounds(x, y)));
  const frame = (buttons: number, touch: { x: number; y: number; id?: number }[]) => {
    mock.tick(1 / 60);
    // (a contact beyond 511 logical pixels travels in the wide form, as PocketJS's C runtime packs it)
    const packed = touch.map((t) => (t.x > 511 || t.y > 511 ? __packTouchWide : __packTouch)(t.id ?? 1, t.x, t.y));
    const surface = view.aux ? 1 : 0;
    globals.frame(buttons, undefined, packed, facts(packed), packed.map(() => surface));
    for (const line of pending.splice(0)) mock.receive(JSON.parse(line));
    wasm.tick();
  };

  // The rasterizer has no alpha; render over black and over white and take
  // the difference (the root's own colour is the only thing that changes).
  const matte = (render: () => Uint8Array, rootNode: number, w: number, h: number, scale: number): Canvas => {
    ops.setProp(rootNode, PROP.bgColor, 0xff000000);
    const black = render().slice();
    ops.setProp(rootNode, PROP.bgColor, 0xffffffff);
    const white = render().slice();
    ops.setProp(rootNode, PROP.bgColor, 0);
    const pixels = new Uint8ClampedArray(black.length);
    for (let i = 0; i < black.length; i += 4) {
      const alpha = 255 - (white[i + 1] - black[i + 1]);
      pixels[i + 3] = alpha;
      for (let c = 0; c < 3; c++) pixels[i + c] = alpha ? Math.min(255, (black[i + c] * 255) / alpha) : 0;
    }
    const canvas = createCanvas(w * scale, h * scale);
    canvas.getContext("2d").putImageData(new ImageData(pixels, w * scale, h * scale), 0, 0);
    return canvas;
  };

  return {
    mock, view,
    step(frames = 1, input = {}) {
      for (let i = 0; i < frames; i++) frame(input.buttons ?? 0, input.touch ?? []);
    },
    press(buttons) {
      frame(buttons, []);
      frame(buttons, []);
      frame(0, []);
    },
    tap(x, y) {
      frame(0, [{ x, y }]);
      frame(0, [{ x, y }]);
      frame(0, []);
      frame(0, []);
    },
    async shot(backdrop) {
      const scale = view.density;
      const top = matte(() => wasm.renderScaled(scale), 1, view.w, view.h, scale);
      const auxH = view.aux ? view.aux[1] : 0;
      const canvas = createCanvas(view.w * scale, (view.h + auxH) * scale);
      const ctx = canvas.getContext("2d");
      ctx.fillStyle = "#000000";
      ctx.fillRect(0, 0, canvas.width, canvas.height);
      if (backdrop && existsSync(backdrop)) {
        // The scene behind the interface: the capture cropped to this screen's shape.
        const image = await loadImage(backdrop);
        const width = Math.min(image.width, (image.height * view.w) / view.h), height = (width * view.h) / view.w;
        ctx.drawImage(image, (image.width - width) / 2, (image.height - height) / 2, width, height, 0, 0, view.w * scale, view.h * scale);
      }
      ctx.drawImage(top, 0, 0);
      if (view.aux) {
        const bottom = matte(() => wasm.renderAuxiliary(), ops.__auxiliarySurface.root, view.aux[0], view.aux[1], 1);
        ctx.drawImage(bottom, ((view.w - view.aux[0]) / 2) * scale, view.h * scale, view.aux[0] * scale, view.aux[1] * scale);
      }
      return canvas;
    },
  };
}
