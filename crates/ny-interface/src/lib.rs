//! The renderer's side of `ui/app/protocol.ts`.
//!
//! The interface is one PocketJS guest (`ui/`) drawn over the city. It owns every 2D pixel, and what a button or a
//! finger means while a list is up. The renderer keeps a [`State`] and the guest is sent the members that changed,
//! as one JSON line a turn; what the guest asks for comes back as [`Command`]s. The lines travel over the guest's
//! service channel, PocketJS's `pocket.overlay`, answered in the process (`guest` for the QuickJS API, `wire` for
//! PocketJS's C hosts) instead of on a wire.
//!
//! [`session::Session`] is the flow around play (the title, play, the menu over it), written once for every
//! device.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

#[cfg(feature = "guest")]
pub mod guest;
pub mod session;
#[cfg(feature = "wire")]
pub mod wire;

pub use session::{pad, Pace, Pad, Session};

/// What the screen is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// The pack is being read; `State::message` names the step.
    #[default]
    Loading,
    /// The title over the city, seen through the title's camera.
    Title,
    /// The city has the screen and the pad moves the player.
    Play,
    /// A list is up over play; the pad is the interface's.
    Menu,
    /// `State::message` says what failed.
    Error,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Loading => "loading",
            Mode::Title => "title",
            Mode::Play => "play",
            Mode::Menu => "menu",
            Mode::Error => "error",
        }
    }

    pub fn parse(name: &str) -> Option<Mode> {
        [Mode::Loading, Mode::Title, Mode::Play, Mode::Menu, Mode::Error].into_iter().find(|m| m.name() == name)
    }
}

/// The player's mode by the reference's name for it (`ny_sim::Mode::name`): "person", "car", "cyclist",
/// "helicopter".
pub fn player(name: &str) -> Option<ny_sim::Mode> {
    ny_sim::Mode::ALL.into_iter().find(|m| m.name() == name)
}

/// Something the person can set: a switch (0 or 1), or with `choices` one of several named values.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Setting {
    pub key: &'static str,
    pub value: u32,
    pub choices: Vec<&'static str>,
}

impl Setting {
    pub fn switch(key: &'static str, on: bool) -> Self {
        Self { key, value: on as u32, choices: Vec::new() }
    }

    pub fn choice(key: &'static str, value: usize, choices: &[&'static str]) -> Self {
        Self { key, value: value as u32, choices: choices.to_vec() }
    }
}

/// The numbers that change while the player moves, sent at most once a turn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Telemetry {
    /// Miles an hour, as the reference shows them.
    pub mph: i32,
    /// Feet above the street.
    pub altitude: i32,
    /// Degrees clockwise from the grid's north (-z).
    pub heading: i32,
    /// The player on the map, metres (the city's x and z).
    pub x: i32,
    pub z: i32,
    /// The player boosts (1) or not (0): the speed meter turns taxi yellow.
    pub boost: i32,
}

/// What the interface is shown.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub mode: Mode,
    /// The loading step, or why `mode` is `Error`.
    pub message: String,
    /// How the player goes through the city.
    pub player: ny_sim::Mode,
    /// Index of the hour (`ny_sim::Hour`) and of the camera (the reference's `CAMERA_PRESETS`).
    pub hour: u8,
    pub camera: u8,
    /// Where the player is, named by the device (the reference's `getLocationLabel`): a landmark or a district, and
    /// the avenue and the street; `near` when the streets are near the place rather than its corner.
    pub place: String,
    pub streets: [String; 2],
    pub near: bool,
    /// What the device lets the person set, in menu order.
    pub options: Vec<Setting>,
    /// One line of renderer statistics while the `stats` setting is on.
    pub stats: String,
    /// What the interface last stored with `prefs`.
    pub prefs: String,
    /// The `wake` the interface asked for that last came due; 0 before any has.
    pub woke: u32,
    pub t: Telemetry,
}

fn escape(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c < ' ' => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn assign(to: &mut String, from: &str) {
    if to != from {
        to.clear();
        to.push_str(from);
    }
}

impl State {
    /// Writes where the player is, leaving each member that holds the same text as it was.
    pub fn locate(&mut self, place: &str, avenue: &str, street: &str, near: bool) {
        assign(&mut self.place, place);
        assign(&mut self.streets[0], avenue);
        assign(&mut self.streets[1], street);
        self.near = near;
    }

    /// The line the guest receives: every member, or with `since` the members that differ from it. `None` when
    /// nothing does.
    pub fn line(&self, since: Option<&State>) -> Option<String> {
        let mut s = String::with_capacity(256);
        s.push_str("{\"type\":\"state\",\"value\":{");
        let start = s.len();
        macro_rules! member {
            ($field:ident, $key:literal, $write:expr) => {
                if since.map_or(true, |old| old.$field != self.$field) {
                    if s.len() > start {
                        s.push(',');
                    }
                    s.push_str(concat!("\"", $key, "\":"));
                    #[allow(clippy::redundant_closure_call)]
                    ($write)(&mut s);
                }
            };
        }
        let flag = |s: &mut String, on: bool| s.push_str(if on { "true" } else { "false" });
        member!(mode, "mode", |s: &mut String| escape(s, self.mode.name()));
        member!(message, "message", |s: &mut String| escape(s, &self.message));
        member!(player, "player", |s: &mut String| escape(s, self.player.name()));
        member!(hour, "hour", |s: &mut String| { let _ = write!(s, "{}", self.hour); });
        member!(camera, "camera", |s: &mut String| { let _ = write!(s, "{}", self.camera); });
        member!(place, "place", |s: &mut String| escape(s, &self.place));
        member!(streets, "streets", |s: &mut String| {
            s.push('[');
            escape(s, &self.streets[0]);
            s.push(',');
            escape(s, &self.streets[1]);
            s.push(']');
        });
        member!(near, "near", |s: &mut String| flag(s, self.near));
        member!(options, "options", |s: &mut String| {
            s.push('[');
            for (i, o) in self.options.iter().enumerate() {
                let _ = write!(s, "{}{{\"key\":\"{}\",\"value\":{}", if i > 0 { "," } else { "" }, o.key, o.value);
                if !o.choices.is_empty() {
                    s.push_str(",\"choices\":[");
                    for (j, c) in o.choices.iter().enumerate() {
                        if j > 0 {
                            s.push(',');
                        }
                        escape(s, c);
                    }
                    s.push(']');
                }
                s.push('}');
            }
            s.push(']');
        });
        member!(stats, "stats", |s: &mut String| escape(s, &self.stats));
        member!(prefs, "prefs", |s: &mut String| escape(s, &self.prefs));
        member!(woke, "woke", |s: &mut String| { let _ = write!(s, "{}", self.woke); });
        member!(t, "t", |s: &mut String| {
            let t = &self.t;
            let _ = write!(s, "[{},{},{},{},{},{}]", t.mph, t.altitude, t.heading, t.x, t.z, t.boost);
        });
        if s.len() == start {
            return None;
        }
        s.push_str("}}\n");
        Some(s)
    }
}

/// What the interface asks of the renderer.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Leave the title as this player.
    Start(ny_sim::Mode),
    /// A list comes up over play, or leaves it.
    Menu(bool),
    /// Go through the city another way (the reference's mode buttons).
    Player(ny_sim::Mode),
    /// One of the reference's hours, by index (`ny_sim::Hour`).
    Hour(u8),
    /// One of the reference's cameras, by index (`CAMERA_PRESETS`).
    Camera(u8),
    /// Teleport to one of the reference's places, by index (`TELEPORT_LOCATIONS` in index.html's order).
    Place(u8),
    /// Back to the title.
    Title,
    Option { key: String, value: u32 },
    /// Controls drawn on a touch panel: the stick, -1…1 (y is ahead), and the held buttons ([`pad`] bits).
    Drive { mx: f32, my: f32, buttons: u32 },
    /// A finger turning the camera about the player, logical pixels since the last one.
    Look { dx: f32, dy: f32 },
    /// To store, and to hand back in [`State::prefs`].
    Prefs(String),
    /// The interface has nothing scheduled (nothing fading): a turn is worth its cost only when the renderer has
    /// news for it or a button it listens to changes.
    Idle(bool),
    /// Say `id` back in [`State::woke`] in this many seconds. The interface's own timers count its turns, and an
    /// idle guest takes none: what must happen later (a hint leaving) is timed here.
    Wake { id: u32, seconds: f32 },
}

/// The value after `"key":` in a flat JSON object. Quoted text is skipped when looking for the key, so a value
/// cannot pose as one.
fn field<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let bytes = json.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'"' {
            i += 1;
            continue;
        }
        let begin = i + 1;
        let mut end = begin;
        while end < bytes.len() && bytes[end] != b'"' {
            end += if bytes[end] == b'\\' { 2 } else { 1 };
        }
        if end >= bytes.len() {
            return None;
        }
        let rest = json[end + 1..].trim_start();
        if rest.starts_with(':') && &json[begin..end] == key {
            return Some(rest[1..].trim_start());
        }
        i = end + 1;
    }
    None
}

fn number(json: &str, key: &str) -> f32 {
    let Some(value) = field(json, key) else { return 0.0 };
    let end = value.find(|c: char| !(c.is_ascii_digit() || matches!(c, '-' | '.' | 'e' | 'E' | '+'))).unwrap_or(value.len());
    value[..end].parse::<f32>().unwrap_or(0.0)
}

fn text(json: &str, key: &str) -> Option<String> {
    let value = field(json, key)?.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = value.chars();
    loop {
        match chars.next()? {
            '"' => return Some(out),
            '\\' => out.push(chars.next()?),
            c => out.push(c),
        }
    }
}

fn flag(json: &str, key: &str) -> bool {
    field(json, key).is_some_and(|v| v.starts_with("true"))
}

fn index(json: &str, key: &str) -> u8 {
    number(json, key).clamp(0.0, 255.0) as u8
}

impl Command {
    pub fn parse(line: &str) -> Option<Command> {
        let n = |key| number(line, key);
        Some(match text(line, "type")?.as_str() {
            "start" => Command::Start(player(&text(line, "player")?)?),
            "menu" => Command::Menu(flag(line, "on")),
            "player" => Command::Player(player(&text(line, "id")?)?),
            "hour" => Command::Hour(index(line, "index")),
            "camera" => Command::Camera(index(line, "index")),
            "place" => Command::Place(index(line, "index")),
            "title" => Command::Title,
            "option" => Command::Option { key: text(line, "key")?, value: n("value").max(0.0) as u32 },
            "drive" => Command::Drive { mx: n("mx") / 100.0, my: n("my") / 100.0, buttons: n("b").max(0.0) as u32 },
            "look" => Command::Look { dx: n("dx"), dy: n("dy") },
            "prefs" => Command::Prefs(text(line, "value")?),
            "idle" => Command::Idle(flag(line, "on")),
            "wake" => Command::Wake { id: n("id").max(0.0) as u32, seconds: n("seconds") },
            _ => return None,
        })
    }
}

/// The channel's two ends: the state last sent, and what the guest said.
#[derive(Default)]
pub struct Interface {
    pub state: State,
    sent: Option<State>,
    open: bool,
    inbox: VecDeque<String>,
}

impl Interface {
    /// The guest opened its service: only the overlay is here.
    pub fn open(&mut self, service: &str) -> bool {
        self.open = service == "pocket.overlay";
        self.sent = None; // a fresh guest gets the whole state
        self.open
    }

    /// A guest holds the channel: the interface is on the screen.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The guest is gone (it failed, or is being replaced).
    pub fn close(&mut self) {
        self.open = false;
        self.inbox.clear();
    }

    /// The state line, when the guest has not seen this state.
    pub fn poll(&mut self) -> Option<String> {
        self.poll_within(usize::MAX)
    }

    /// The same for a guest whose buffer takes `capacity` bytes: a longer line is not sent, and stays owed.
    pub fn poll_within(&mut self, capacity: usize) -> Option<String> {
        if !self.open {
            return None;
        }
        let line = self.state.line(self.sent.as_ref()).filter(|line| line.len() <= capacity)?;
        self.sent = Some(self.state.clone());
        Some(line)
    }

    /// The guest has not seen the state as it now is.
    pub fn news(&self) -> bool {
        self.open && self.sent.as_ref() != Some(&self.state)
    }

    /// What the guest has not seen is at most readouts (the numbers in play, the statistics line): text swapped in
    /// place, with nothing set moving.
    pub fn only_readouts(&self) -> bool {
        let Some(sent) = &self.sent else { return false };
        let State { mode, message, player, hour, camera, place, streets, near, options, prefs, woke, stats: _, t: _ } = &self.state;
        *mode == sent.mode
            && *message == sent.message
            && *player == sent.player
            && *hour == sent.hour
            && *camera == sent.camera
            && *place == sent.place
            && *streets == sent.streets
            && *near == sent.near
            && *options == sent.options
            && *prefs == sent.prefs
            && *woke == sent.woke
    }

    /// A line from the guest.
    pub fn receive(&mut self, line: &str) {
        if self.inbox.len() < 32 {
            self.inbox.push_back(String::from(line));
        }
    }

    /// The next command the guest sent, oldest first.
    pub fn next(&mut self) -> Option<Command> {
        while let Some(line) = self.inbox.pop_front() {
            if let Some(command) = Command::parse(&line) {
                return Some(command);
            }
        }
        None
    }
}

static mut INTERFACE: Option<Interface> = None;

/// The one channel, on the thread that runs the guest and the game.
///
/// # Safety
/// Call only on that thread; do not keep the reference across a guest turn.
pub unsafe fn channel() -> &'static mut Interface {
    (*core::ptr::addr_of_mut!(INTERFACE)).get_or_insert_with(Interface::default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use ny_sim::Mode as Player;

    #[test]
    fn commands_from_the_interface() {
        // Lines as ui/test/harness.ts logs them.
        let parse = |line| Command::parse(line).unwrap();
        assert_eq!(parse(r#"{"type":"start","player":"car"}"#), Command::Start(Player::Drive));
        assert_eq!(parse(r#"{"type":"start","player":"helicopter"}"#), Command::Start(Player::Fly));
        assert_eq!(Command::parse(r#"{"type":"start","player":"boat"}"#), None);
        assert_eq!(parse(r#"{"type":"menu","on":true}"#), Command::Menu(true));
        assert_eq!(parse(r#"{"type":"player","id":"person"}"#), Command::Player(Player::Walk));
        assert_eq!(parse(r#"{"type":"hour","index":3}"#), Command::Hour(3));
        assert_eq!(parse(r#"{"type":"camera","index":10}"#), Command::Camera(10));
        assert_eq!(parse(r#"{"type":"place","index":7}"#), Command::Place(7));
        assert_eq!(parse(r#"{"type":"title"}"#), Command::Title);
        assert_eq!(parse(r#"{"type":"option","key":"traffic","value":0}"#), Command::Option { key: "traffic".into(), value: 0 });
        assert_eq!(parse(r#"{"type":"drive","mx":0,"my":72,"b":5}"#), Command::Drive { mx: 0.0, my: 0.72, buttons: 5 });
        assert_eq!(parse(r#"{"type":"look","dx":-3.5,"dy":12}"#), Command::Look { dx: -3.5, dy: 12.0 });
        assert_eq!(parse(r#"{"type":"prefs","value":"{\"hud\":1}"}"#), Command::Prefs(r#"{"hud":1}"#.into()));
        assert_eq!(parse(r#"{"type":"idle","on":true}"#), Command::Idle(true));
        assert_eq!(parse(r#"{"type":"wake","id":4,"seconds":7}"#), Command::Wake { id: 4, seconds: 7.0 });
        assert_eq!(Command::parse(r#"{"type":"pocket.overlay.control","name":"x","node":3}"#), None);
        // A value that looks like a key is not one.
        assert_eq!(parse(r#"{"type":"prefs","value":"\"type\":\"title\""}"#), Command::Prefs(r#""type":"title""#.into()));
    }

    #[test]
    fn the_guest_is_sent_what_changed() {
        let mut interface = Interface::default();
        assert_eq!(interface.poll(), None);
        assert!(!interface.open("youtube"));
        assert!(interface.open("pocket.overlay"));
        interface.state.mode = Mode::Title;
        interface.state.options = vec![Setting::switch("traffic", true), Setting::switch("stats", false)];
        assert_eq!(
            interface.poll().unwrap(),
            "{\"type\":\"state\",\"value\":{\"mode\":\"title\",\"message\":\"\",\"player\":\"car\",\"hour\":0,\"camera\":0,\"place\":\"\",\"streets\":[\"\",\"\"],\"near\":false,\"options\":[{\"key\":\"traffic\",\"value\":1},{\"key\":\"stats\",\"value\":0}],\"stats\":\"\",\"prefs\":\"\",\"woke\":0,\"t\":[0,0,0,0,0,0]}}\n"
        );
        assert_eq!(interface.poll(), None);
        // A turn in play: only the numbers go out.
        interface.state.t = Telemetry { mph: 27, altitude: 0, heading: 358, x: 4, z: 470, boost: 1 };
        assert!(interface.news() && interface.only_readouts());
        assert_eq!(interface.poll().unwrap(), "{\"type\":\"state\",\"value\":{\"t\":[27,0,358,4,470,1]}}\n");
        // A corner passed: the place and the streets, once.
        interface.state.locate("Empire State Building", "5th Ave", "W 34th St", false);
        assert!(!interface.only_readouts());
        assert_eq!(interface.poll().unwrap(), "{\"type\":\"state\",\"value\":{\"place\":\"Empire State Building\",\"streets\":[\"5th Ave\",\"W 34th St\"]}}\n");
        interface.state.locate("Empire State Building", "5th Ave", "W 34th St", false);
        assert_eq!(interface.poll(), None);
        interface.state.mode = Mode::Menu;
        interface.state.player = Player::Fly;
        assert_eq!(interface.poll().unwrap(), "{\"type\":\"state\",\"value\":{\"mode\":\"menu\",\"player\":\"helicopter\"}}\n");
        interface.receive(r#"{"type":"menu","on":false}"#);
        interface.receive("not a command");
        interface.receive(r#"{"type":"title"}"#);
        assert_eq!(interface.next(), Some(Command::Menu(false)));
        assert_eq!(interface.next(), Some(Command::Title));
        assert_eq!(interface.next(), None);
        assert!(!interface.news());
        // A guest that starts again is sent everything.
        assert!(interface.open("pocket.overlay"));
        assert!(interface.poll().unwrap().contains("\"player\":\"helicopter\""));
    }
}
