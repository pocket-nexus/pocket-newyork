// What crosses between a device's renderer and the interface. The renderer owns the game (the player, its camera,
// the hour, the traffic) and the scene; the interface owns every 2D pixel and what a button or a finger means while
// a list is up. Both sides speak JSON lines over PocketJS's in-process overlay service: the renderer sends the
// members of its state that changed, the interface sends commands. `crates/ny-interface` is the renderer's side.
//
// The renderers parse commands with a small scanner, so commands are flat objects of numbers, booleans and short
// strings.

export type Mode = "loading" | "title" | "play" | "menu" | "error";

/** How the player goes through the city, by the reference's name for it (`ny_sim::Mode::name`). */
export type Player = "person" | "car" | "cyclist" | "helicopter";

/** A switch (0 or 1), or with `choices` one of several named values. */
export interface Setting {
  key: string;
  value: number;
  choices?: string[];
}

export interface HostState {
  /** What the screen is for. Behind the title the title's camera shows the city. */
  mode: Mode;
  /** The loading step, or why `mode` is "error". */
  message: string;
  player: Player;
  /** Index of the hour (the reference's four, `HOURS`) and of the camera (`CAMERAS`). */
  hour: number;
  camera: number;
  /** Where the player is, as the reference names it: a landmark or a district ("" for none), the avenue and the
   *  street ("" each for none), and whether the streets are near the place rather than its corner. */
  place: string;
  streets: [string, string];
  near: boolean;
  /** What the device lets the person set, in menu order. */
  options: Setting[];
  /** One line of renderer statistics while the `stats` setting is on. */
  stats: string;
  /** What the interface last stored with `prefs`. */
  prefs: string;
  /** The `wake` the interface asked for that last came due; 0 before any has. */
  woke: number;
  /** The numbers that change in play, at most once a turn (see `T`). */
  t: number[];
}

/** Indices into `HostState.t`. */
export const T = {
  /** Miles an hour. */
  mph: 0,
  /** Feet above the street. */
  altitude: 1,
  /** Degrees clockwise from the grid's north. */
  heading: 2,
  /** The player on the map: the city's metres east and south. */
  x: 3,
  z: 4,
  /** 1 while the player boosts. */
  boost: 5,
} as const;

/** The pad bits a touch panel's keys hold (`ny_interface::pad`). */
export const KEY = { boost: 1, up: 2, down: 4 } as const;

export type Command =
  /** Leave the title as this player. */
  | { type: "start"; player: Player }
  /** A list comes up over play, or leaves it. */
  | { type: "menu"; on: boolean }
  /** Go through the city another way. */
  | { type: "player"; id: Player }
  /** One of the reference's hours, by index. */
  | { type: "hour"; index: number }
  /** One of the reference's cameras, by index. */
  | { type: "camera"; index: number }
  /** Teleport to one of the reference's places, by index. */
  | { type: "place"; index: number }
  /** Back to the title. */
  | { type: "title" }
  | { type: "option"; key: string; value: number }
  /** Controls drawn on a touch panel: the stick, -100…100 on each axis (y is ahead), and the held keys (`KEY`). */
  | { type: "drive"; mx: number; my: number; b: number }
  /** A finger turning the camera about the player: logical px since the last command. */
  | { type: "look"; dx: number; dy: number }
  | { type: "prefs"; value: string }
  /** The interface has nothing scheduled (nothing fading). A turn of the guest costs milliseconds on the slower
   *  machines, so while this is on the renderer turns it only when it has news or a button the interface listens
   *  to changes. */
  | { type: "idle"; on: boolean }
  /** Say `id` back in `HostState.woke` in this many seconds. The interface's own timers count its turns, and an
   *  idle guest takes none: what must happen later (a hint leaving) is timed by the renderer. */
  | { type: "wake"; id: number; seconds: number };
