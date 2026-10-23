// The pieces every presentation is built from. A device chooses where they go and how large they are; what the
// speed cluster, the street signs, a list row or the map looks like is decided here once, after the reference's
// HUD (web/index.html, web/css/style.css).
//
// The numbers in play (the speed, the height, the heading, the player on the map) change up to 30 times a second.
// Each is written straight to its node (`@pocketjs/framework/hot`) in a cell of fixed size, so a new value costs
// one native call and no layout. Everything else is a signal.
import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show, untrack, type JSX } from "solid-js";
import { useActions, type ActionsHandle } from "@pocketjs/framework/actions";
import { Image, Text, View } from "@pocketjs/framework/components";
import type { SurfaceId } from "@pocketjs/framework/display";
import { createGesture } from "@pocketjs/framework/gesture";
import { prop as hotProp, text as hotText } from "@pocketjs/framework/hot";
import { BTN } from "@pocketjs/framework/input";
import { onFrame } from "@pocketjs/framework/lifecycle";
import { modality } from "@pocketjs/framework/modality";
import type { NodeMirror } from "@pocketjs/framework/renderer";
import { modeOf, read, type City, type Row } from "./city.ts";
import { CAMERAS, HOURS, PLACES, type MapPicture } from "./generated/world.ts";
import type { Host } from "./host.ts";
import { T } from "./protocol.ts";
import { DIM, FAINT, GLASS, HAIRLINE, HEADING, INK, NIGHT, PANEL, PLATE, SIGN, TAXI, TAXI_INK, WASH } from "./theme.ts";

/**
 * A view a finger can tap, on the surface it is drawn on. It stays out of the focus order: a pad has its own button
 * for the same verb. While a finger is on it the view dims, or with `onHeld` its owner is told and draws the press
 * itself.
 */
export function Touchable(props: { surface?: SurfaceId; onTap?: () => void; onHeld?: (held: boolean) => void; class?: string; style?: Record<string, number | string>; children?: JSX.Element }) {
  let node: NodeMirror | undefined;
  const [down, setDown] = createSignal(false);
  const hold = (held: boolean) => (props.onHeld ? props.onHeld(held) : setDown(held));
  createGesture({
    surface: props.surface,
    region: { node: () => node },
    onDown: () => hold(!!props.onTap),
    onUp: () => hold(false),
    onCancel: () => hold(false),
    onTap: () => props.onTap?.(),
  });
  return <View ref={node} class={props.class} style={{ ...props.style, opacity: down() ? 0.55 : 1 }}>{props.children}</View>;
}

/** A pill of text over the scene. */
export function Chip(props: { text: string; color?: string }) {
  return (
    <View class="rounded px-2 py-1" style={{ bgColor: "#121316c0" }}>
      <Text class="text-xs font-bold" style={{ textColor: props.color ?? INK }}>{props.text}</Text>
    </View>
  );
}

/** The reference's mark: a taxi-yellow subway bullet. */
export function Bullet(props: { large?: boolean }) {
  return (
    <Show when={props.large} fallback={
      <View class="rounded-full w-[22] h-[22] items-center justify-center" style={{ bgColor: TAXI }}>
        <Text class="text-xs font-bold" style={{ textColor: TAXI_INK }}>NY</Text>
      </View>
    }>
      <View class="rounded-full w-[34] h-[34] items-center justify-center" style={{ bgColor: TAXI }}>
        <Text class="text-sm font-bold" style={{ textColor: TAXI_INK }}>NY</Text>
      </View>
    </Show>
  );
}

/** The name beside the bullet. */
export function Wordmark(props: { large?: boolean }) {
  return (
    <View class="flex-row items-center gap-2">
      <Bullet large={props.large} />
      <View class="flex-col">
        <Text class="text-xs font-bold tracking-wide" style={{ textColor: TAXI }}>POCKET</Text>
        <Text class={props.large ? "text-2xl font-bold" : "text-lg font-bold"} style={{ textColor: INK }}>New York</Text>
      </View>
    </View>
  );
}

/** The hour, as the reference's toolbar names it, with its time. */
export function HourChip(props: { host: Host }) {
  const hour = () => HOURS[props.host.hour()];
  return (
    <View class="flex-row items-center gap-2 rounded px-2 py-1" style={{ bgColor: GLASS, borderWidth: 1, borderColor: HAIRLINE }}>
      <View class="rounded-full w-[6] h-[6]" style={{ bgColor: TAXI }} />
      <Text class="text-xs font-bold" style={{ textColor: INK }}>{hour()?.name ?? ""}</Text>
      <Text class="text-xs" style={{ textColor: DIM }}>{hour()?.time ?? ""}</Text>
    </View>
  );
}

/** A Manhattan street-name sign: the green blade with its white inset border. */
function Sign(props: { name: string }) {
  return (
    <View class="rounded p-[2]" style={{ bgColor: SIGN }}>
      <View class="rounded-sm px-2 py-[3]" style={{ borderWidth: 1, borderColor: "#ffffffe8" }}>
        <Text class="text-sm font-bold" style={{ textColor: "#ffffff" }}>{props.name}</Text>
      </View>
    </View>
  );
}

/**
 * Where the player is, as the reference draws it: the place's name over a pair of street-name signs, the signs
 * after "near" when they are near the place rather than its corner. It changes at a corner, a few times a minute:
 * signals, laid out anew each time.
 */
export function Location(props: { host: Host }) {
  const host = props.host;
  const streets = createMemo(() => host.streets().filter(Boolean));
  return (
    <View class="flex-col gap-1">
      <Show when={host.place()}>
        <View class="flex-row">
          <View class="rounded px-2 py-[2]" style={{ bgColor: PLATE }}>
            <Text class="text-xs font-bold tracking-wide" style={{ textColor: INK }}>{host.place().toUpperCase()}</Text>
          </View>
        </View>
      </Show>
      <Show when={streets().length}>
        <View class="flex-row items-center gap-1">
          <Show when={host.near()}>
            <View class="rounded px-1" style={{ bgColor: PLATE }}>
              <Text class="text-xs font-bold tracking-wide" style={{ textColor: INK }}>NEAR</Text>
            </View>
          </Show>
          <For each={streets()}>{(name) => <Sign name={name} />}</For>
        </View>
      </Show>
    </View>
  );
}

/**
 * The reference's cluster: what the player is, its speed in miles an hour over a meter of the mode's top speed (taxi
 * yellow while it boosts), its height in feet when it flies, and the camera.
 */
export function Gauge(props: { host: Host; width: number }) {
  const host = props.host;
  let digits: NodeMirror | undefined, altitude: NodeMirror | undefined, fill: NodeMirror | undefined, boost: NodeMirror | undefined;
  const shown = [-1, -1, -1, -1];
  const flying = () => host.player() === "helicopter";
  const top = createMemo(() => modeOf(host.player()).topMph || 80);
  const inner = () => props.width - 24;
  onMount(() => onCleanup(host.onNumbers((t) => {
    const mph = t[T.mph], alt = t[T.altitude], on = t[T.boost];
    if (mph !== shown[0]) hotText(digits, (shown[0] = mph));
    if (alt !== shown[1]) hotText(altitude, (shown[1] = alt));
    const part = Math.round(Math.min(1, mph / untrack(top)) * 100) / 100;
    if (part !== shown[2]) {
      shown[2] = part;
      hotProp(fill, "scaleX", part);
      hotProp(boost, "scaleX", part);
    }
    if (on !== shown[3]) hotProp(boost, "opacity", (shown[3] = on));
  })));
  return (
    <View class="relative rounded-lg flex-col" style={{ width: props.width, height: 76, bgColor: GLASS, borderWidth: 1, borderColor: HAIRLINE, paddingL: 12, paddingT: 7 }}>
      <Text class="text-xs font-bold" style={{ width: inner(), height: 14, textColor: DIM }}>{modeOf(host.player()).label.toUpperCase()}</Text>
      <View class="relative flex-row items-end" style={{ width: inner(), height: 30 }}>
        <Text ref={digits} class="text-2xl font-bold" style={{ width: 44, height: 30, textColor: INK }}>0</Text>
        <Text class="text-xs font-bold tracking-wide" style={{ marginB: 4, textColor: FAINT }}>MPH</Text>
        {/* The height stands at the right of the speed, in a cell of fixed size, while the player flies. */}
        <View class="absolute flex-row items-end" style={{ insetR: 0, insetB: 4, opacity: flying() ? 1 : 0 }}>
          <Text ref={altitude} class="text-xs font-bold" style={{ width: 28, height: 14, textColor: INK }}>0</Text>
          <Text class="text-xs font-bold tracking-wide" style={{ textColor: FAINT }}>FT</Text>
        </View>
      </View>
      <View class="relative rounded" style={{ width: inner(), height: 4, marginT: 3, bgColor: "#ffffff1a" }}>
        <View ref={fill} class="absolute rounded" style={{ insetL: 0, insetT: 0, width: inner(), height: 4, originX: -0.5, scaleX: 0, bgColor: INK }} />
        <View ref={boost} class="absolute rounded" style={{ insetL: 0, insetT: 0, width: inner(), height: 4, originX: -0.5, scaleX: 0, opacity: 0, bgColor: TAXI }} />
      </View>
      <View class="relative flex-row items-center gap-1" style={{ width: inner(), height: 14, marginT: 5 }}>
        <Text class="text-xs font-bold tracking-wide" style={{ textColor: FAINT }}>CAMERA</Text>
        <Text class="text-xs" style={{ textColor: DIM }}>{CAMERAS[host.camera()]?.short ?? ""}</Text>
      </View>
    </View>
  );
}

/**
 * Manhattan from above, as the reference's own atlas draws it, grid north up, with the player on it: a taxi-yellow
 * mark with a line toward where it heads, moved and turned by the numbers in play. With `places` the teleports
 * stand on it.
 */
export function IslandMap(props: { host: Host; map: MapPicture; places?: boolean }) {
  const m = props.map;
  const px = (metres: number, from: number) => (metres - from) / m.metres;
  let mark: NodeMirror | undefined;
  const shown = [NaN, NaN, NaN];
  onMount(() => onCleanup(props.host.onNumbers((t) => {
    const x = Math.round(Math.max(0, Math.min(m.width, px(t[T.x], m.x0))));
    const y = Math.round(Math.max(0, Math.min(m.height, px(t[T.z], m.z0))));
    if (x !== shown[0]) hotProp(mark, "translateX", (shown[0] = x));
    if (y !== shown[1]) hotProp(mark, "translateY", (shown[1] = y));
    if (t[T.heading] !== shown[2]) hotProp(mark, "rotate", (shown[2] = t[T.heading]));
  })));
  return (
    <View class="relative" style={{ width: m.width, height: m.height, bgColor: NIGHT }}>
      <Image src={m.image} class="absolute" style={{ insetL: 0, insetT: 0, width: m.texture, height: m.texture }} />
      <Show when={props.places}>
        <For each={PLACES}>
          {(place) => <View class="absolute rounded-full w-[4] h-[4]" style={{ insetL: px(place.x, m.x0) - 2, insetT: px(place.z, m.z0) - 2, bgColor: "#f6f5f2c0" }} />}
        </For>
      </Show>
      <View ref={mark} class="absolute" style={{ insetL: -5, insetT: -5, width: 10, height: 10 }}>
        <View class="absolute" style={{ insetL: 4, insetT: -6, width: 2, height: 8, bgColor: INK }} />
        <View class="absolute rounded-full w-[8] h-[8]" style={{ insetL: 1, insetT: 1, bgColor: TAXI, borderWidth: 1, borderColor: NIGHT }} />
      </View>
    </View>
  );
}

/** The line in the middle of the screen when the player, the hour or the camera changes. */
export function Note(props: { city: City; width: number }) {
  return (
    <View class={props.city.note() ? "items-center justify-center opacity-100 transition-opacity duration-200" : "items-center justify-center opacity-0 transition-opacity duration-200"} style={{ width: props.width }}>
      <View class="rounded px-3 py-1" style={{ bgColor: "#12131690" }}>
        <Text class="text-lg font-bold" style={{ textColor: INK }}>{props.city.note() || " "}</Text>
      </View>
    </View>
  );
}

/** The strip along the bottom of a screen with buttons: a notice at the left, what the buttons do at the right. */
export function Legend(props: { width: number; left?: string; legend: string }) {
  return (
    <View class="relative" style={{ width: props.width, height: 24, bgColor: GLASS }}>
      <View class="absolute" style={{ insetL: 0, insetT: 0, width: props.width, height: 1, bgColor: HAIRLINE }} />
      <Text class="absolute text-xs" style={{ insetL: 10, insetT: 5, textColor: DIM }}>{props.left ?? ""}</Text>
      <Text class="absolute text-xs" style={{ insetR: 10, insetT: 5, textColor: INK }}>{props.legend}</Text>
    </View>
  );
}

/** The renderer's statistics line, while its setting is on. */
export function Stats(props: { host: Host }) {
  return (
    <Show when={props.host.stats()}>
      <Chip text={props.host.stats()} color={DIM} />
    </Show>
  );
}

/** A list's focus, and the buttons that drive it. One for the life of a presentation: it watches the pad through
 *  play too, so a button held when a list opens is not taken for a press. */
export interface Menu {
  focus: () => number;
  press(index: number): void;
  actions: ActionsHandle;
}

export function createMenu(city: City): Menu {
  const [focus, setFocus] = createSignal(0);
  const pressable = (row: Row | undefined) => !!row?.press;
  // A list starts on the current choice, or its first row; a switch that changes under the focus keeps it.
  createEffect(on(() => `${city.mode()} ${city.sheet()}`, () => {
    const rows = untrack(city.rows);
    setFocus(Math.max(0, rows.findIndex((row) => row.active?.())));
  }));
  const press = (index: number) => {
    const row = city.rows()[index];
    if (!pressable(row)) return;
    setFocus(index);
    row!.press!();
  };
  const step = (by: number) => {
    const rows = city.rows();
    for (let at = focus() + by; at >= 0 && at < rows.length; at += by) {
      if (pressable(rows[at])) return setFocus(at);
    }
  };
  let previous = ~0;
  onFrame((buttons) => {
    const pressed = buttons & ~previous;
    previous = buttons;
    if (!city.listing()) return;
    if (pressed & BTN.UP) step(-1);
    if (pressed & BTN.DOWN) step(1);
  });
  // The bindings are read every turn: built again only when the mode or the list changes.
  const actions = useActions(createMemo(() => ({
    confirm: { label: "select", run: () => press(focus()), when: () => city.listing() && pressable(city.rows()[focus()]) },
    back: { label: city.backLabel(), run: city.back, when: () => city.listing() && !!city.backLabel() },
    // START opens the menu over play, and closes it.
    media: { label: city.mode() === "play" ? "menu" : undefined, run: () => (city.mode() === "menu" ? city.back() : city.menu()), when: () => city.mode() === "play" || (city.mode() === "menu" && city.sheet() === "menu") },
  })));
  return { focus, press, actions };
}

/** A switch: its knob at the right when on. */
function Switch(props: { on: boolean }) {
  return (
    <View class="relative rounded-full w-[34] h-[18]" style={{ bgColor: props.on ? TAXI : "#ffffff30" }}>
      <View class="absolute rounded-full w-[14] h-[14] transition-transform duration-150" style={{ insetL: 2, insetT: 2, translateX: props.on ? 16 : 0, bgColor: props.on ? TAXI_INK : INK }} />
    </View>
  );
}

/** What a row that acts shows: its label (taxi yellow for the current choice), at the right its switch or its named
 *  value, and a rule under it. */
export function Face(props: { row: Row; width: number; height: number }) {
  const active = () => !!props.row.active?.();
  return (
    <>
      <Show when={props.row.active}>
        <View class="absolute rounded-full w-[6] h-[6]" style={{ insetL: 12, insetT: props.height / 2 - 3, opacity: active() ? 1 : 0, bgColor: TAXI }} />
      </Show>
      <Text class="absolute text-sm font-bold" style={{ insetL: props.row.active ? 24 : 14, insetT: props.height / 2 - 9, textColor: active() ? TAXI : INK }}>{read(props.row.label)}</Text>
      <Show when={props.row.on}>
        <View class="absolute" style={{ insetR: 14, insetT: props.height / 2 - 9 }}><Switch on={props.row.on!()} /></View>
      </Show>
      <Show when={props.row.value !== undefined}>
        <Text class="absolute text-xs" style={{ insetR: 14, insetT: props.height / 2 - 8, textColor: DIM }}>{read(props.row.value!)}</Text>
      </Show>
      <View class="absolute" style={{ insetL: 0, insetB: 0, width: props.width, height: 1, bgColor: HAIRLINE }} />
    </>
  );
}

/** The bar at the left of a list's mark; the mark itself is a wash a row tall. */
export function Mark(props: { height: number }) {
  return <View class="absolute" style={{ insetL: 0, insetT: 0, width: 3, height: props.height, bgColor: TAXI }} />;
}

/**
 * The rows of the list that is up. A row that acts takes a tap and the list's mark: one bar, so moving it changes
 * one node. Where the device has buttons the mark is the focus and slides to its row; where it has none the mark
 * stands under the finger while a row is pressed. A row that only informs is a label and what it means, in a
 * shorter line.
 *
 * A list longer than `visible` rows slides under its panel's edge so that the focus keeps a row below it in view
 * (the panel clips it).
 *
 * Every list this view has shown stays built, hidden: showing one again builds nothing, and a switch that turns
 * writes its own row. `warm` lists are built with the view, before they are first shown.
 */
export function Rows(props: { city: City; menu: Menu; width: number; rowHeight: number; infoHeight?: number; visible?: number; surface?: SurfaceId; active?: () => boolean; warm?: Row[][] }) {
  const info = () => props.infoHeight ?? 22;
  const live = () => props.active?.() ?? true;
  const rows = createMemo<Row[]>((before) => (live() ? props.city.rows() : before), props.warm?.[0] ?? []);
  const focus = createMemo<number>((before) => (live() ? props.menu.focus() : before), 0);
  const built = createMemo<Row[][]>((before) => (!rows().length || before.includes(rows()) ? before : [...before, rows()]), props.warm ?? []);
  // The first row in view.
  const first = createMemo(() => {
    const window = props.visible ?? Infinity;
    if (rows().length <= window) return 0;
    return Math.max(0, Math.min(focus() - window + 2, rows().length - window));
  });
  const [held, setHeld] = createSignal(-1);
  const marked = () => (modality.buttons ? focus() : held());
  return (
    <View class="relative flex-col" style={{ width: props.width }}>
      <Show when={rows().some((row) => row.press)}>
        <View class={modality.buttons ? "absolute transition-transform duration-100 ease-out" : "absolute"} style={{ insetL: 0, insetT: 0, width: props.width, height: props.rowHeight, translateY: Math.max(0, marked() - first()) * props.rowHeight, display: marked() < 0 ? 1 : 0, bgColor: WASH }}>
          <Mark height={props.rowHeight} />
        </View>
      </Show>
      <For each={built()}>
        {(list) => {
          // A list keeps the row height it was built with: another list's height does not reach it.
          const height = untrack(() => props.rowHeight);
          return (
            <View class="flex-col" style={{ width: props.width, display: list === rows() ? 0 : 1, translateY: list === rows() ? -first() * height : 0 }}>
              <For each={list}>
                {(row, index) => (
                  <Show
                    when={row.press}
                    fallback={
                      <View class="relative" style={{ width: props.width, height: info() }}>
                        <Text class="absolute text-xs font-bold" style={{ insetL: 14, insetT: info() / 2 - 8, textColor: TAXI }}>{read(row.label)}</Text>
                        <Text class="absolute text-xs" style={{ insetL: 14 + Math.min(96, props.width * 0.3), insetT: info() / 2 - 8, textColor: DIM }}>{read(row.value ?? "")}</Text>
                      </View>
                    }
                  >
                    <Touchable surface={props.surface} class="relative" style={{ width: props.width, height }} onTap={() => props.menu.press(index())} onHeld={modality.buttons ? undefined : (on) => setHeld(on ? index() : -1)}>
                      <Face row={row} width={props.width} height={height} />
                    </Touchable>
                  </Show>
                )}
              </For>
            </View>
          );
        }}
      </For>
    </View>
  );
}

/**
 * A screen that is built the first time `when` holds (at once with `eager`) and then stays, shown or hidden.
 * Showing it again builds nothing.
 */
export function Keep(props: { when: boolean; eager?: boolean; width: number; height: number; children: JSX.Element }) {
  const [built, setBuilt] = createSignal(!!props.eager);
  createEffect(() => {
    if (props.when) setBuilt(true);
  });
  return (
    <View class="absolute" style={{ insetL: 0, insetT: 0, width: props.width, height: props.height, display: props.when ? 0 : 1, hitPass: 1 }}>
      <Show when={built()}>{props.children}</Show>
    </View>
  );
}

/** A list's heading over a rule, on the panel's own ground: a list that slides goes under it. On a surface a finger
 *  reaches, `back` stands at its left as the way out. */
export function Heading(props: { text: string; width: number; height: number; back?: string; onBack?: () => void; surface?: SurfaceId }) {
  const way = () => (props.back ? 88 : 0);
  return (
    <View class="relative" style={{ width: props.width, height: props.height, bgColor: HEADING }}>
      <Show when={props.back}>
        <Touchable surface={props.surface} class="absolute flex-row items-center" style={{ insetL: 0, insetT: 0, width: 96, height: props.height }} onTap={props.onBack}>
          <Text class="text-sm font-bold" style={{ marginL: 14, textColor: TAXI }}>{`‹ ${props.back}`}</Text>
        </Touchable>
      </Show>
      <Text class="absolute text-xs font-bold tracking-wide" style={{ insetL: 14 + way(), insetT: props.height / 2 - 8, textColor: DIM }}>{props.text}</Text>
      <View class="absolute" style={{ insetL: 0, insetB: 0, width: props.width, height: 1, bgColor: HAIRLINE }} />
    </View>
  );
}

/** A view that fades with `shown`. */
export function Fade(props: { shown: boolean; children: JSX.Element }) {
  return <View class={props.shown ? "opacity-100 transition-opacity duration-300" : "opacity-0 transition-opacity duration-300"}>{props.children}</View>;
}

/** A panel over a scrim, in the middle of a surface. */
export function Panel(props: { width: number; height: number; panelWidth: number; panelHeight: number; children: JSX.Element }) {
  return (
    <View class="absolute" style={{ insetL: 0, insetT: 0, width: props.width, height: props.height, bgColor: "#00000080" }}>
      <View class="absolute rounded-lg overflow-hidden flex-row" style={{ insetL: (props.width - props.panelWidth) / 2, insetT: Math.max(4, (props.height - props.panelHeight) / 2), width: props.panelWidth, height: props.panelHeight, bgColor: PANEL, borderWidth: 1, borderColor: HAIRLINE }}>
        {props.children}
      </View>
    </View>
  );
}

/** The screen while the city loads, and when it could not. */
export function Loading(props: { host: Host; width: number; height: number }) {
  return (
    <View class="items-center justify-center flex-col gap-3" style={{ width: props.width, height: props.height, bgColor: "#0c0d10" }}>
      <Wordmark large />
      <Text class="text-xs" style={{ textColor: props.host.mode() === "error" ? TAXI : DIM }}>{props.host.message() || "Building the city"}</Text>
    </View>
  );
}
