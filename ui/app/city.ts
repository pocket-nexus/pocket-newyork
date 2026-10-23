// The city as the interface sees it, on any device: which list is up, what its rows do, what the player is called
// and where it is, and what is kept between runs (the settings).
import { createEffect, createMemo, createSignal, on, onCleanup, untrack, type Accessor } from "solid-js";
import { after } from "@pocketjs/framework/clock";
import { glyph, surfaceHasTouch } from "@pocketjs/framework/modality";
import { CAMERAS, HOURS, MODES, PLACES } from "./generated/world.ts";
import type { Host } from "./host.ts";
import type { Mode, Player, Setting } from "./protocol.ts";

/** What the settings a renderer may offer are called. */
const LABELS: Record<string, string> = {
  traffic: "Traffic",
  invert: "Invert camera",
  stats: "Statistics",
};

/**
 * A row of a list. What can change while the row stands (a switch, a named value, the mark of the current choice,
 * a label that says what a press would do) is a function: a list is built once, kept, and its rows are written in
 * place.
 */
export interface Row {
  label: string | (() => string);
  /** A switch's state, or undefined for a row with a named value or none. */
  on?: () => boolean;
  /** The named value of a choice, or what stands at the right of the row. */
  value?: string | (() => string);
  /** The row is the current one of a choice (the player's mode, the hour, the camera). */
  active?: () => boolean;
  /** A row that only informs takes no press and no focus mark. */
  press?: () => void;
}

/** What a row's member reads now. */
export function read<T extends string | boolean>(member: T | (() => T)): T {
  return typeof member === "function" ? member() : member;
}

/** The list over the city: the mode's own, or one opened from it. */
export type Sheet = "menu" | "player" | "hour" | "camera" | "places" | "settings" | "controls" | "about";

const HEADINGS: Record<Sheet, string> = {
  menu: "MENU",
  player: "GO BY",
  hour: "HOUR",
  camera: "CAMERA",
  places: "TELEPORT TO",
  settings: "SETTINGS",
  controls: "CONTROLS",
  about: "ABOUT",
};

export interface City {
  host: Host;
  mode: Accessor<Mode>;
  /** True while a list is up and the pad is the interface's. */
  listing: Accessor<boolean>;
  sheet: Accessor<Sheet>;
  open(sheet: Sheet): void;
  heading: Accessor<string>;
  rows: Accessor<Row[]>;
  /** One level up: a list opened from another closes; the menu leaves play alone. */
  back(): void;
  /** Whether `back` does anything here, and what it is called. */
  backLabel: Accessor<string>;
  menu(): void;
  /** A line for the middle of the screen when the player, the hour or the camera changes, "" when none. */
  note: Accessor<string>;
  /** Where the player is, in one line ("Empire State Building · 5th Ave & W 34th St"). */
  where: Accessor<string>;
  /** What the strip under a list says at its left: the hour on the title, where the player is under a menu. It
   *  holds through play, so that a new corner writes no text while no list is up. */
  notice: Accessor<string>;
  /** The readouts stand over the city (a setting of the interface's own, the reference's clean view). */
  readouts: Accessor<boolean>;
  /** The title's own rows and the menu's: a presentation may build them before either is first shown. */
  lists: { title: Row[]; menu: Row[] };
}

/** The mode of the reference by its id. */
export function modeOf(player: Player) {
  return MODES.find((m) => m.id === player) ?? MODES[1];
}

/** How many fades are playing: while one is, a timer of the guest's own is pending. */
const [settling, setSettling] = createSignal(0);
/** Seconds a fade takes to play, and a turn more. */
const SETTLE = 0.4;
/** Each showing of a pulse has an id of its own, which the renderer says back when the showing is over. */
let wakes = 0;
const waking = new Map<number, () => void>();

/** The guest takes every turn it is offered until what just changed has faded in or out. */
function settle() {
  setSettling((count) => count + 1);
  after(SETTLE, () => setSettling((count) => count - 1));
}

/**
 * True for `seconds` after each `show()`. The seconds are counted by the renderer (`wake`), not by a timer here: a
 * timer of the guest's counts its turns, so one that is pending makes the device take every turn it offers, 30 a
 * second, for as long as a hint stands. Between the two fades nothing is scheduled.
 */
export function createPulse(host: Host, seconds: number): [Accessor<boolean>, () => void] {
  const [shown, setShown] = createSignal(false);
  let id = 0;
  onCleanup(() => waking.delete(id));
  return [shown, () => {
    waking.delete(id);
    id = ++wakes;
    waking.set(id, () => {
      setShown(false);
      settle();
    });
    host.send({ type: "wake", id, seconds });
    if (untrack(shown)) return;
    setShown(true);
    settle();
  }];
}

/** The location as one line, as the reference's label reads. */
export function locate(place: string, streets: [string, string], near: boolean): string {
  const corner = streets.filter(Boolean).join(" & ");
  if (!corner) return place;
  const at = near ? `near ${corner}` : corner;
  return place ? `${place} · ${at}` : at;
}

/** What moves the player on this device, each control with what it does. */
export function controls(): [string, string][] {
  const vita = surfaceHasTouch();
  return [
    [vita ? "Left stick" : "Stick", "Go ahead or back, turn"],
    ...(vita ? [["Right stick", "Turn the camera"] as [string, string]] : []),
    [`${glyph("cross")}`, "Hold to boost"],
    [`${glyph("rtrigger")} / ${glyph("ltrigger")}`, "Climb, descend; R jumps on foot"],
    ["D-pad", "Left, right: the hour"],
    [glyph("select"), "Next camera"],
    [glyph("start"), "Menu"],
  ];
}

interface Prefs {
  options?: Record<string, number>;
  /** 0 hides the readouts. */
  hud?: number;
}

export function createCity(host: Host, touch: boolean): City {
  const [sheet, setSheet] = createSignal<Sheet>("menu");
  const [prefs, setPrefs] = createSignal<Prefs>({});
  const mode = host.mode;
  const listing = createMemo(() => mode() === "title" || mode() === "menu");
  const keep = (next: Prefs) => {
    setPrefs(next);
    host.send({ type: "prefs", value: JSON.stringify(next) });
  };

  // Each mode opens on its own list.
  createEffect(on(mode, () => setSheet("menu")));

  // A change of the player, the hour or the camera is said once, in the middle of the screen.
  const [noted, showNote] = createPulse(host, 1.8);
  const [said, setSaid] = createSignal("");
  // What changes in the same line as the mode (play beginning, the title coming back) is not said.
  createEffect(on(() => [mode(), host.player(), host.hour(), host.camera()] as const, (now, before) => {
    if (!before || now[0] !== before[0] || now[0] !== "play") return;
    const text = now[3] !== before[3] ? CAMERAS[now[3]]?.name : now[2] !== before[2] ? HOURS[now[2]]?.name : now[1] !== before[1] ? modeOf(now[1]).label : undefined;
    if (!text) return;
    setSaid(text);
    showNote();
  }));
  // With no pulse showing nothing is scheduled here: the renderer may skip turns until it has news.
  createEffect(() => host.ready() && host.send({ type: "idle", on: settling() === 0 }));
  // A showing of a pulse is over when the renderer says its id.
  createEffect(on(host.woke, (id) => {
    const over = waking.get(id);
    waking.delete(id);
    over?.();
  }, { defer: true }));

  // What the renderer stored for us last time: the settings, which it is told again once it has listed them.
  let restored = false;
  createEffect(() => {
    if (!host.ready() || restored || !host.options().length) return;
    restored = true;
    let stored: Prefs = {};
    try {
      stored = JSON.parse(untrack(host.prefs) || "{}") as Prefs;
    } catch {
      // A damaged file starts over.
    }
    setPrefs(stored);
    for (const setting of untrack(host.options)) {
      const value = stored.options?.[setting.key];
      if (typeof value === "number" && value !== setting.value) host.send({ type: "option", key: setting.key, value });
    }
  });
  const set = (setting: Setting, value: number) => {
    host.send({ type: "option", key: setting.key, value });
    keep({ ...prefs(), options: { ...prefs().options, [setting.key]: value } });
  };
  // A setting's row stands while the renderer offers the setting: its state is read where it is shown.
  const row = (key: string, choice: boolean): Row => {
    const label = LABELS[key] ?? key;
    const now = () => host.options().find((setting) => setting.key === key);
    if (!choice) {
      return { label, on: () => !!now()?.value, press: () => {
        const setting = now();
        if (setting) set(setting, setting.value ? 0 : 1);
      } };
    }
    return { label, value: () => now()?.choices?.[now()!.value] ?? "", press: () => {
      const setting = now();
      if (setting?.choices) set(setting, (setting.value + 1) % setting.choices.length);
    } };
  };
  const readouts = () => prefs().hud !== 0;
  // Which settings the renderer offers, each with whether it is a choice: the list is built again only when this
  // changes, not when a setting's value does.
  const offered = createMemo(() => host.options().map((setting) => `${setting.key}${setting.choices ? ":" : ""}`).join(" "));
  const settings = createMemo<Row[]>(() => [
    { label: "Readouts", on: readouts, press: () => keep({ ...prefs(), hud: readouts() ? 0 : 1 }) },
    ...offered().split(" ").filter(Boolean).map((key) => row(key.replace(":", ""), key.endsWith(":"))),
  ]);

  const leave = () => host.send({ type: "menu", on: false });
  // The lists of the reference's choices. Choosing the player or a place goes back to the city; the hour and the
  // camera are seen changing behind the list, which stays.
  const players: Row[] = MODES.map((m) => ({
    label: m.name, value: m.label, active: () => host.player() === m.id,
    press: () => (host.send({ type: "player", id: m.id }), leave()),
  }));
  const hours: Row[] = HOURS.map((h, index) => ({
    label: h.name, value: h.time, active: () => host.hour() === index,
    press: () => host.send({ type: "hour", index }),
  }));
  const cameras: Row[] = CAMERAS.map((c, index) => ({
    label: c.name, value: c.category === "follow" ? "follow" : "", active: () => host.camera() === index,
    press: () => host.send({ type: "camera", index }),
  }));
  // From the title a place is where play starts, as the player the title last showed.
  const places: Row[] = PLACES.map((p, index) => ({
    label: p.name, value: p.sub,
    press: () => {
      host.send({ type: "place", index });
      if (untrack(mode) === "title") host.send({ type: "start", player: untrack(host.player) });
      else leave();
    },
  }));

  // Each mode's own list is built once: showing it again changes no row. The title begins with the four ways
  // through the city, the taxi first: the reference starts in it.
  const order: Player[] = ["car", "person", "cyclist", "helicopter"];
  const title: Row[] = [
    ...order.map((id): Row => ({ label: modeOf(id).name, value: modeOf(id).label, press: () => host.send({ type: "start", player: id }) })),
    { label: "Places", press: () => setSheet("places") },
    { label: "Hour", value: () => HOURS[host.hour()]?.name ?? "", press: () => setSheet("hour") },
    { label: "Settings", press: () => setSheet("settings") },
    { label: "About", press: () => setSheet("about") },
  ];
  const menu: Row[] = [
    // On a touch panel the way back stands in the heading.
    ...(touch ? [] : [{ label: "Resume", press: leave }]),
    { label: "Go by", value: () => modeOf(host.player()).name, press: () => setSheet("player") },
    { label: "Hour", value: () => HOURS[host.hour()]?.name ?? "", press: () => setSheet("hour") },
    { label: "Camera", value: () => CAMERAS[host.camera()]?.short ?? "", press: () => setSheet("camera") },
    { label: "Places", press: () => setSheet("places") },
    { label: "Settings", press: () => setSheet("settings") },
    { label: touch ? "How to move" : "Controls", press: () => setSheet("controls") },
    { label: "Back to the title", press: () => host.send({ type: "title" }) },
  ];
  const explained: Row[] = controls().map(([label, value]) => ({ label, value }));
  const about: Row[] = [
    { label: "Model", value: "Slop New York, mrdoob (MIT)" },
    { label: "Map data", value: "NYC Open Data" },
    { label: "Streets", value: "© OpenStreetMap contributors (ODbL)" },
    { label: "Built with", value: "three.js, WebGPU, TSL" },
    { label: "Engine", value: "Pocket3D" },
  ];
  // During play no list is up: the rows stay what they were, so nothing is rebuilt under the readouts.
  const rows = createMemo<Row[]>((before) => {
    if (!listing()) return before;
    switch (sheet()) {
      case "player": return players;
      case "hour": return hours;
      case "camera": return cameras;
      case "places": return places;
      case "settings": return settings();
      case "controls": return explained;
      case "about": return about;
      default: return mode() === "title" ? title : menu;
    }
  }, []);

  const where = createMemo(() => locate(host.place(), host.streets(), host.near()));
  const hourLine = () => {
    const h = HOURS[host.hour()];
    return h ? `${h.name} · ${h.time}` : "";
  };
  // On the title: where play would begin, or the hour.
  const notice = createMemo<string>((before) => (mode() === "title" ? where() || hourLine() : mode() === "menu" ? where() : before), "");

  return {
    host, mode, listing, sheet, rows, where, notice, readouts,
    lists: { title, menu },
    open: setSheet,
    // A note belongs to play: it does not stand under a list.
    note: () => (mode() === "play" && noted() ? said() : ""),
    heading: () => (sheet() === "menu" ? (mode() === "menu" ? HEADINGS.menu : "") : HEADINGS[sheet()]),
    back() {
      if (sheet() !== "menu") setSheet("menu");
      else if (mode() === "menu") leave();
    },
    backLabel: () => (sheet() !== "menu" ? "back" : mode() === "menu" ? "resume" : ""),
    menu: () => host.send({ type: "menu", on: true }),
  };
}
