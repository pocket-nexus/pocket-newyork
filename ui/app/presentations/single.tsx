// One 480×272 screen with a pad: the PS Vita, whose panel also takes taps (and the PSP's shape). During play the
// city has the screen and the interface is the readouts at its corners, where the reference keeps them: the hour
// at the top right, where the player is and the island at the bottom left, the speed cluster at the bottom right.
// A list comes up over it for the title and the menu.
import { createEffect, on, Show, type JSX } from "solid-js";
import { Text, View } from "@pocketjs/framework/components";
import { glyph } from "@pocketjs/framework/modality";
import { createCity, createPulse, type City } from "../city.ts";
import { MAP, SMALL_MAP } from "../generated/world.ts";
import { connectHost } from "../host.ts";
import { Chip, createMenu, Fade, Gauge, Heading, HourChip, IslandMap, Keep, Legend, Loading, Location, type Menu, Note, Panel, Rows, Stats, Wordmark } from "../parts.tsx";
import { DIM, GLASS, HAIRLINE } from "../theme.ts";

const W = 480, H = 272;
const FOOTER = 24;
/** Rows of a list, and how many stand in a panel before it slides. */
const ROW = 26;
const PANEL_H = 240;
const HEAD = 28;
const VISIBLE = Math.floor((PANEL_H - HEAD) / ROW);

export default function SingleScreen() {
  const host = connectHost();
  const city = createCity(host, false);
  const menu = createMenu(city);
  return (
    <View class="relative w-full h-full">
      <Show when={host.mode() === "loading" || host.mode() === "error"}><Loading host={host} width={W} height={H} /></Show>
      <Keep when={host.mode() === "title"} eager width={W} height={H}><Title city={city} menu={menu} /></Keep>
      <Keep when={host.mode() === "play"} eager width={W} height={H}><Readouts city={city} /></Keep>
      <Keep when={host.mode() === "menu"} eager width={W} height={H}><Over city={city} menu={menu} /></Keep>
      {/* The strip stays built and is shown with a list: a list coming up builds nothing. */}
      <View class="absolute" style={{ insetL: 0, insetB: 0, display: city.listing() ? 0 : 1 }}>
        <Legend width={W} left={city.notice()} legend={menu.actions.legend()} />
      </View>
    </View>
  );
}

function Title(props: { city: City; menu: Menu }) {
  const city = props.city;
  return (
    <View class="relative w-full h-full">
      <View class="absolute bg-gradient-to-r from-[#000000c8] to-[#00000000]" style={{ insetL: 0, insetT: 0, width: 300, height: H }} />
      <View class="absolute" style={{ insetL: 16, insetT: 6 }}><Wordmark /></View>
      <View class="absolute" style={{ insetR: 12, insetT: 10 }}><HourChip host={city.host} /></View>
      {/* The title's own rows, and the panel a list opened from them comes up in. Both stay built: the rows from the
          start, the panel from the first time it is shown. */}
      <View class="absolute" style={{ insetL: 6, insetT: 40, display: city.sheet() === "menu" ? 0 : 1 }}>
        <Rows city={city} menu={props.menu} width={220} rowHeight={ROW} active={() => city.mode() === "title" && city.sheet() === "menu"} warm={[city.lists.title]} />
      </View>
      <Keep when={city.sheet() !== "menu"} width={W} height={H - FOOTER}>
        <Panel width={W} height={H - FOOTER} panelWidth={360} panelHeight={PANEL_H}>
          <List width={360}>
            <Rows city={city} menu={props.menu} width={360} rowHeight={ROW} visible={VISIBLE} active={() => city.mode() === "title" && city.sheet() !== "menu"} />
            <Heading text={city.heading()} width={360} height={HEAD} />
          </List>
        </Panel>
      </Keep>
    </View>
  );
}

/** Play's readouts: the hour above, where the player is and the island below at the left, the cluster at the right. */
function Readouts(props: { city: City }) {
  const city = props.city;
  const host = city.host;
  // What the buttons do shows when play begins, then leaves the view clear.
  const [hint, showHint] = createPulse(host, 7);
  createEffect(on(host.mode, (mode, before) => mode === "play" && before === "title" && showHint()));
  return (
    <View class="relative w-full h-full">
      <View class="absolute w-full h-full" style={{ insetL: 0, insetT: 0, opacity: city.readouts() ? 1 : 0 }}>
        <View class="absolute" style={{ insetR: 12, insetT: 10 }}><HourChip host={host} /></View>
        <View class="absolute rounded-lg p-[4]" style={{ insetL: 10, insetB: 10, bgColor: GLASS, borderWidth: 1, borderColor: HAIRLINE }}>
          <IslandMap host={host} map={SMALL_MAP} />
          <Text class="absolute text-xs font-bold" style={{ insetL: 7, insetT: 4, textColor: DIM }}>N</Text>
        </View>
        <View class="absolute" style={{ insetL: 18 + SMALL_MAP.width + 8, insetB: 10 }}><Location host={host} /></View>
        <View class="absolute" style={{ insetR: 10, insetB: 10 }}><Gauge host={host} width={170} /></View>
      </View>
      <View class="absolute" style={{ insetL: 0, insetT: 84 }}><Note city={city} width={W} /></View>
      <View class="absolute" style={{ insetL: 10, insetT: 10 }}><Stats host={host} /></View>
      <View class="absolute items-center justify-center" style={{ insetL: 0, insetT: 40, width: W }}>
        <Fade shown={hint()}><Chip text={`${glyph("cross")} boost · ${glyph("rtrigger")}/${glyph("ltrigger")} up, down · D-pad hour · ${glyph("select")} camera · ${glyph("start")} menu`} /></Fade>
      </View>
    </View>
  );
}

/** A panel's list: its rows under its heading, which is drawn after them (a long list slides under it). */
function List(props: { width: number; children: [JSX.Element, JSX.Element] }) {
  return (
    <View class="relative" style={{ width: props.width, height: PANEL_H }}>
      <View class="absolute" style={{ insetL: 0, insetT: HEAD }}>{props.children[0]}</View>
      <View class="absolute" style={{ insetL: 0, insetT: 0 }}>{props.children[1]}</View>
    </View>
  );
}

const MAP_W = MAP.width + 24;

/** The menu over play: the island from above beside the list. */
function Over(props: { city: City; menu: Menu }) {
  const list = 440 - MAP_W;
  return (
    <Panel width={W} height={H - FOOTER} panelWidth={440} panelHeight={PANEL_H}>
      <View class="items-center justify-center" style={{ width: MAP_W, height: PANEL_H }}><IslandMap host={props.city.host} map={MAP} places /></View>
      <List width={list}>
        <Rows city={props.city} menu={props.menu} width={list} rowHeight={ROW} visible={VISIBLE} active={() => props.city.mode() === "menu"} warm={[props.city.lists.menu]} />
        <Heading text={props.city.heading()} width={list} height={HEAD} />
      </List>
    </Panel>
  );
}

