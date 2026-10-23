//! The flow around play, the same on every device: the title over the city, play with the pad moving the player,
//! and the menu over it. A device owns the pad's wiring and the scene; it hands this its [`Pad`] each frame, and
//! this steps the [`Game`] and says what the interface is shown.

use alloc::string::String;
use alloc::vec::Vec;

use ny_sim::{Game, Hour, Input, Mode as Player};

use crate::{channel, Command, Mode, Setting, State, Telemetry};

/// The pad as a device hands it over, whatever its buttons are called there. The PS Vita: ✕ boosts, R climbs (a
/// jump on foot) and L descends, left and right on the d-pad step the hour, SELECT the camera, START the menu.
pub mod pad {
    /// Sprint, boost.
    pub const BOOST: u32 = 1 << 0;
    /// The helicopter climbs; on foot, a jump.
    pub const UP: u32 = 1 << 1;
    /// The helicopter descends.
    pub const DOWN: u32 = 1 << 2;
    /// The hour before, the hour after.
    pub const EARLIER: u32 = 1 << 3;
    pub const LATER: u32 = 1 << 4;
    /// The next camera.
    pub const CAMERA: u32 = 1 << 5;
    /// The button that opens the menu. The interface hears it by itself; this bit is for a device with no interface
    /// on the screen, where it takes the player through the four modes.
    pub const MENU: u32 = 1 << 16;
    /// The bits the player reads while held, and the ones play reads when pressed.
    pub const MOVE: u32 = BOOST | UP | DOWN;
    pub const KEYS: u32 = EARLIER | LATER | CAMERA;
}

/// Sticks in -1…1 (ahead and right positive) and [`pad`] bits.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Pad {
    pub buttons: u32,
    pub lx: f32,
    pub ly: f32,
    pub rx: f32,
    pub ry: f32,
}

/// Radians a second the right stick turns the camera about the player at full tilt.
const ORBIT: f32 = 2.4;
/// Radians a finger turns the camera per logical pixel it drags: about the reference's drag on a screen 480
/// pixels wide.
const LOOK: f32 = 0.006;

pub struct Session {
    pub mode: Mode,
    /// The right stick's pitch is turned over.
    pub invert: bool,
    /// The device draws traffic and lets the person turn it off; whether it is on.
    pub traffic: bool,
    pub traffic_on: bool,
    /// Renderer statistics are shown (as a line the interface draws).
    pub stats: bool,
    /// The interface has nothing scheduled: see [`Pace`].
    pub idle: bool,
    /// Frames between two refreshes of the numbers in play.
    pub numbers_every: u32,
    /// The device's cameras (the reference's `CAMERA_PRESETS`): how many, how many of the first follow the player,
    /// and which one shows the city behind the title.
    pub cameras: u8,
    pub follow: u8,
    pub title_camera: Option<u8>,
    /// Where a `Place` command takes the player: x, z and the heading to face (index.html's teleports).
    pub places: Vec<[f32; 3]>,
    /// The camera play had when the title took the screen, and gets back.
    play_camera: u8,
    /// A touch panel's stick and keys, and what a finger dragged since it was last spent.
    drive: (f32, f32, u32),
    look: (f32, f32),
    prev_buttons: u32,
    frames: u32,
    /// The player boosts this frame.
    boosting: bool,
    /// The settings as last listed for the interface.
    listed: Option<(bool, bool, bool, bool)>,
    /// Seconds this flow has run, and what the interface asked to be woken for: an id and the second it is due.
    seconds: f32,
    wakes: Vec<(u32, f32)>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Session {
        Session {
            mode: Mode::Title,
            invert: false,
            traffic: false,
            traffic_on: true,
            stats: false,
            idle: false,
            numbers_every: 2,
            cameras: 1,
            follow: 1,
            title_camera: None,
            places: Vec::new(),
            play_camera: 0,
            drive: (0.0, 0.0, 0),
            look: (0.0, 0.0),
            prev_buttons: u32::MAX,
            frames: 0,
            boosting: false,
            listed: None,
            seconds: 0.0,
            wakes: Vec::new(),
        }
    }

    /// Nothing a touch panel held carries over a change of mode.
    fn rest(&mut self) {
        self.drive = (0.0, 0.0, 0);
        self.look = (0.0, 0.0);
    }

    /// The camera play shows: the one asked for, unless it is a scenic one and the player just changed (the
    /// reference then goes back to its chase camera).
    fn follow_camera(&self, camera: u8) -> u8 {
        if camera < self.follow {
            camera
        } else {
            0
        }
    }

    /// Does what the interface asked. A command that is the device's own comes back.
    pub fn command(&mut self, game: &mut Game, command: Command) -> Option<Command> {
        match command {
            Command::Start(player) => {
                if self.mode == Mode::Title {
                    self.mode = Mode::Play;
                    game.set_mode(player);
                    game.set_camera(self.follow_camera(self.play_camera));
                    self.rest();
                }
            }
            Command::Menu(on) => match (self.mode, on) {
                (Mode::Play, true) => {
                    self.mode = Mode::Menu;
                    self.rest();
                }
                (Mode::Menu, false) => self.mode = Mode::Play,
                _ => {}
            },
            Command::Player(player) => {
                game.set_mode(player);
                if self.mode == Mode::Title {
                    self.play_camera = self.follow_camera(self.play_camera);
                } else {
                    game.set_camera(self.follow_camera(game.camera));
                }
            }
            Command::Hour(index) => game.set_hour(Hour::ALL[(index as usize).min(Hour::ALL.len() - 1)]),
            Command::Camera(index) => {
                if index < self.cameras {
                    if self.mode == Mode::Title {
                        self.play_camera = index;
                    } else {
                        game.set_camera(index);
                    }
                }
            }
            Command::Place(index) => {
                if let Some(&[x, z, heading]) = self.places.get(index as usize) {
                    game.teleport(x, z, heading);
                    // The reference's teleport ends on its chase camera.
                    if self.mode == Mode::Title {
                        self.play_camera = 0;
                    } else {
                        game.set_camera(0);
                    }
                }
            }
            Command::Title => {
                if self.mode != Mode::Title {
                    self.play_camera = game.camera;
                }
                self.mode = Mode::Title;
                if let Some(camera) = self.title_camera {
                    game.set_camera(camera);
                }
                self.rest();
            }
            Command::Option { key, value } => match key.as_str() {
                "invert" => self.invert = value != 0,
                "stats" => self.stats = value != 0,
                "traffic" if self.traffic => self.traffic_on = value != 0,
                _ => return Some(Command::Option { key, value }),
            },
            Command::Drive { mx, my, buttons } => self.drive = (mx.clamp(-1.0, 1.0), my.clamp(-1.0, 1.0), buttons & pad::MOVE),
            Command::Look { dx, dy } => {
                self.look.0 += dx * LOOK;
                self.look.1 += dy * LOOK;
            }
            Command::Idle(on) => self.idle = on,
            Command::Wake { id, seconds } => {
                if self.wakes.len() < 16 {
                    self.wakes.push((id, self.seconds + seconds.max(0.0)));
                }
            }
            other => return Some(other),
        }
        None
    }

    /// Does everything the interface asked since the last frame. `other(key, value)` is told an option this flow
    /// does not know; the return is the text the interface wants kept, when it said so.
    pub fn obey(&mut self, game: &mut Game, mut other: impl FnMut(&str, u32)) -> Option<String> {
        let mut kept = None;
        while let Some(command) = unsafe { channel() }.next() {
            match self.command(game, command) {
                Some(Command::Prefs(text)) => kept = Some(text),
                Some(Command::Option { key, value }) => other(&key, value),
                _ => {}
            }
        }
        kept
    }

    /// The settings this flow keeps, in menu order.
    pub fn settings(&self, out: &mut Vec<Setting>) {
        if self.traffic {
            out.push(Setting::switch("traffic", self.traffic_on));
        }
        out.push(Setting::switch("invert", self.invert));
        out.push(Setting::switch("stats", self.stats));
    }

    /// What the player reads this frame: nothing while the title or a list has the pad.
    fn input(&mut self, pad: &Pad, dt: f32) -> Input {
        if self.mode != Mode::Play {
            self.boosting = false;
            return Input::default();
        }
        let (mx, my, held) = self.drive;
        let touched = mx != 0.0 || my != 0.0;
        let buttons = pad.buttons | held;
        let ry = if self.invert { -pad.ry } else { pad.ry };
        // What a finger dragged is spent over a few frames: the camera follows it without a step.
        let k = (20.0 * dt).min(1.0);
        let turn = (self.look.0 * k, self.look.1 * k);
        self.look.0 -= turn.0;
        self.look.1 -= turn.1;
        if self.look.0.abs() < 1e-4 {
            self.look.0 = 0.0;
        }
        if self.look.1.abs() < 1e-4 {
            self.look.1 = 0.0;
        }
        let per_second = 1.0 / dt.max(1e-3);
        let rise = (buttons & pad::UP != 0) as i32 - (buttons & pad::DOWN != 0) as i32;
        self.boosting = buttons & pad::BOOST != 0;
        Input {
            ahead: if touched { my } else { pad.ly },
            turn: if touched { mx } else { pad.lx },
            rise: rise as f32,
            boost: self.boosting,
            orbit: [pad.rx * ORBIT + turn.0 * per_second, ry * ORBIT + turn.1 * per_second],
        }
    }

    /// One frame of `dt` seconds: the pad (or the interface's controls) moves the player, and the game steps.
    pub fn run(&mut self, game: &mut Game, pad: &Pad, dt: f32) {
        let pressed = pad.buttons & !self.prev_buttons;
        self.prev_buttons = pad.buttons;
        self.frames = self.frames.wrapping_add(1);
        self.seconds += dt;
        // No interface on the screen: the city has it, and the menu button takes the player through the modes.
        if !unsafe { channel() }.is_open() && self.frames > 30 {
            self.mode = Mode::Play;
            if pressed & pad::MENU != 0 {
                let at = Player::ALL.iter().position(|&m| m == game.mode).unwrap_or(0);
                game.set_mode(Player::ALL[(at + 1) % Player::ALL.len()]);
                game.set_camera(self.follow_camera(game.camera));
            }
        }
        if self.mode == Mode::Title {
            if let Some(camera) = self.title_camera {
                game.set_camera(camera);
            }
        }
        if self.mode == Mode::Play {
            let hours = Hour::ALL.len() as i32;
            let step = (pressed & pad::LATER != 0) as i32 - (pressed & pad::EARLIER != 0) as i32;
            if step != 0 {
                game.set_hour(Hour::ALL[(game.hour.index() as i32 + step).rem_euclid(hours) as usize]);
            }
            if pressed & pad::CAMERA != 0 && self.cameras > 0 {
                game.set_camera((game.camera + 1) % self.cameras);
            }
        }
        let input = self.input(pad, dt);
        game.step(&input, dt);
    }

    /// Writes what the interface is shown. The numbers in play are refreshed every `numbers_every` frames; where the
    /// player is (`State::locate`) is the device's to write.
    pub fn publish(&mut self, game: &Game, state: &mut State) {
        let r = game.readout();
        state.mode = self.mode;
        state.player = r.mode;
        state.hour = r.hour.index() as u8;
        state.camera = game.camera;
        let listed = (self.traffic, self.traffic_on, self.invert, self.stats);
        if self.listed != Some(listed) {
            self.listed = Some(listed);
            state.options.clear();
            self.settings(&mut state.options);
        }
        // One wake a frame: each is a line of its own, so the interface hears every id.
        if let Some(at) = self.wakes.iter().position(|w| w.1 <= self.seconds) {
            state.woke = self.wakes.swap_remove(at).0;
        }
        if self.frames % self.numbers_every.max(1) != 0 {
            return;
        }
        let whole = |x: f32| libm::floorf(x + 0.5) as i32;
        let mph = whole(r.mph);
        state.t = Telemetry {
            mph,
            altitude: whole(r.altitude_ft.max(0.0)),
            heading: whole(r.heading.to_degrees()).rem_euclid(360),
            x: whole(r.at[0]),
            z: whole(r.at[1]),
            boost: (self.boosting && mph > 0) as i32,
        };
    }
}

/// PocketJS's START: the one button the interface listens to during play (it opens the menu).
const GUEST_MENU: u32 = 0x0008;
/// Turns the guest still takes after the last reason for one: what a press or a line started (a list sliding in, a
/// mark moving) plays out.
const LINGER: u8 = 12;

/// Which of the turns a device offers the guest are worth their cost. A turn runs the whole framework's frame,
/// milliseconds on the slower machines however little changed.
#[derive(Default)]
pub struct Pace {
    buttons: u32,
    linger: u8,
}

impl Pace {
    pub const fn new() -> Pace {
        Pace { buttons: 0, linger: 0 }
    }

    /// `buttons`: the guest's pad (PocketJS's bits) as it would be handed over this turn; `touching`: a finger is on
    /// a surface the guest draws.
    pub fn due(&mut self, session: &Session, buttons: u32, touching: bool) -> bool {
        let heard = if session.mode == Mode::Play { buttons & GUEST_MENU } else { buttons };
        let changed = heard != self.buttons;
        self.buttons = heard;
        let interface = unsafe { channel() };
        let news = interface.news();
        if !session.idle || changed || touching || (news && !interface.only_readouts()) {
            self.linger = LINGER;
            return true;
        }
        if self.linger > 0 {
            self.linger -= 1;
            return true;
        }
        news
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// The game as the device starts it.
    fn game() -> Game {
        Game::new()
    }

    fn session() -> Session {
        let mut s = Session::new();
        s.traffic = true;
        s.cameras = 11;
        s.follow = 3;
        s.title_camera = Some(10);
        s.places = vec![[4.0, 477.0, 0.0], [-581.2, -324.1, 0.0]];
        s
    }

    // One test: the channel is one for the process.
    #[test]
    fn the_flow_follows_the_interface() {
        let mut g = game();
        let mut s = session();
        unsafe { channel() }.open("pocket.overlay");
        let dt = 1.0 / 60.0;
        // Behind the title the title's camera shows the city, and the pad moves nobody.
        s.run(&mut g, &Pad { buttons: pad::UP | pad::BOOST, ly: 1.0, ..Pad::default() }, dt);
        assert_eq!((s.mode, g.camera), (Mode::Title, 10));
        assert_eq!(s.input(&Pad { ly: 1.0, ..Pad::default() }, dt).ahead, 0.0);
        // A place chosen from the title is where play starts, on the chase camera.
        assert_eq!(s.command(&mut g, Command::Place(1)), None);
        let at = g.readout().at;
        assert!((at[0] + 581.2).abs() < 60.0 && (at[1] + 324.1).abs() < 60.0, "teleported to {at:?}");
        assert_eq!(s.command(&mut g, Command::Start(Player::Fly)), None);
        assert_eq!((s.mode, g.mode, g.camera), (Mode::Play, Player::Fly, 0));
        // In play the pad is the player's: the left stick, R and L, ✕.
        let input = s.input(&Pad { ly: 0.8, lx: -0.5, rx: 1.0, buttons: pad::UP | pad::BOOST, ..Pad::default() }, dt);
        assert_eq!((input.ahead, input.turn, input.rise, input.boost), (0.8, -0.5, 1.0, true));
        assert!((input.orbit[0] - ORBIT).abs() < 1e-4);
        // The d-pad steps the hour, both ways round; SELECT steps the camera.
        s.run(&mut g, &Pad { buttons: pad::EARLIER, ..Pad::default() }, dt);
        assert_eq!(g.hour, Hour::Midnight);
        s.run(&mut g, &Pad::default(), dt);
        s.run(&mut g, &Pad { buttons: pad::LATER, ..Pad::default() }, dt);
        assert_eq!(g.hour, Hour::Golden);
        s.run(&mut g, &Pad { buttons: pad::CAMERA, ..Pad::default() }, dt);
        assert_eq!(g.camera, 1);
        s.run(&mut g, &Pad { buttons: pad::CAMERA, ..Pad::default() }, dt);
        assert_eq!(g.camera, 1, "a button held is one press");
        // A scenic camera holds until the player changes mode: the reference goes back to its chase camera then.
        s.command(&mut g, Command::Camera(7));
        assert_eq!(g.camera, 7);
        s.command(&mut g, Command::Player(Player::Walk));
        assert_eq!((g.mode, g.camera), (Player::Walk, 0));
        s.command(&mut g, Command::Camera(2));
        s.command(&mut g, Command::Player(Player::Cycle));
        assert_eq!(g.camera, 2, "a follow camera stays");
        // Under a list the pad is the interface's.
        s.command(&mut g, Command::Menu(true));
        assert_eq!(s.mode, Mode::Menu);
        assert_eq!(s.input(&Pad { ly: 1.0, buttons: pad::BOOST, ..Pad::default() }, dt).ahead, 0.0);
        s.run(&mut g, &Pad { buttons: pad::CAMERA, ..Pad::default() }, dt);
        assert_eq!(g.camera, 2, "SELECT is the list's under a list");
        s.command(&mut g, Command::Hour(2));
        assert_eq!(g.hour, Hour::Twilight);
        // Settings: the flow's own, and one it does not know is the device's.
        assert_eq!(s.command(&mut g, Command::Option { key: "traffic".into(), value: 0 }), None);
        assert!(!s.traffic_on);
        assert_eq!(s.command(&mut g, Command::Option { key: "stats".into(), value: 1 }), None);
        let glow = Command::Option { key: "glow".into(), value: 0 };
        assert_eq!(s.command(&mut g, glow.clone()), Some(glow));
        let mut state = State::default();
        s.publish(&g, &mut state);
        assert_eq!(state.options, vec![Setting::switch("traffic", false), Setting::switch("invert", false), Setting::switch("stats", true)]);
        assert_eq!((state.mode, state.player, state.hour, state.camera), (Mode::Menu, Player::Cycle, 2, 2));
        // The title takes its camera, and play gets its own back.
        s.command(&mut g, Command::Menu(false));
        s.command(&mut g, Command::Title);
        assert_eq!((s.mode, g.camera), (Mode::Title, 10));
        s.command(&mut g, Command::Start(Player::Drive));
        assert_eq!((s.mode, g.mode, g.camera), (Mode::Play, Player::Drive, 2));

        // A touch panel: the stick and the keys drive, a finger turns the camera.
        s.command(&mut g, Command::Drive { mx: 0.25, my: 1.0, buttons: pad::UP | 0x100 });
        let input = s.input(&Pad::default(), dt);
        assert_eq!((input.ahead, input.turn, input.rise), (1.0, 0.25, 1.0));
        s.command(&mut g, Command::Look { dx: 100.0, dy: 0.0 });
        let mut turned = 0.0;
        for _ in 0..60 {
            turned += s.input(&Pad::default(), dt).orbit[0] * dt;
        }
        assert!((turned - 100.0 * LOOK).abs() < 0.01, "turned {turned}");
        s.command(&mut g, Command::Drive { mx: 0.0, my: 0.0, buttons: 0 });

        // What the interface asked to be woken for comes due on this side's clock, one id a frame.
        s.command(&mut g, Command::Wake { id: 7, seconds: 0.5 });
        s.command(&mut g, Command::Wake { id: 8, seconds: 0.5 });
        for _ in 0..28 {
            s.run(&mut g, &Pad::default(), dt);
            s.publish(&g, &mut state);
        }
        assert_eq!(state.woke, 0);
        let mut heard = vec![];
        for _ in 0..6 {
            s.run(&mut g, &Pad::default(), dt);
            s.publish(&g, &mut state);
            heard.push(state.woke);
        }
        heard.retain(|&id| id != 0);
        heard.dedup();
        assert_eq!(heard, [7, 8]);

        // The numbers in play: the player's place and heading, in the city's metres and degrees.
        s.numbers_every = 1;
        s.command(&mut g, Command::Place(0));
        s.run(&mut g, &Pad::default(), dt);
        s.publish(&g, &mut state);
        let r = g.readout();
        assert_eq!((state.t.x, state.t.z), (libm::floorf(r.at[0] + 0.5) as i32, libm::floorf(r.at[1] + 0.5) as i32));
        assert!((0..360).contains(&state.t.heading));

        // An idle guest is turned when there is a reason.
        let mut pace = Pace::default();
        assert!(pace.due(&s, 0, false));
        s.command(&mut g, Command::Idle(true));
        *unsafe { &mut channel().state } = state.clone();
        unsafe { channel() }.poll();
        for _ in 0..LINGER {
            assert!(pace.due(&s, 0, false));
        }
        assert!(!pace.due(&s, 0, false));
        // In play the interface hears the menu button alone.
        assert!(!pace.due(&s, 0x4000, false));
        assert!(pace.due(&s, GUEST_MENU, false));
        for _ in 0..LINGER {
            pace.due(&s, GUEST_MENU, false);
        }
        assert!(!pace.due(&s, GUEST_MENU, false));
        // New numbers are worth one turn; a new street is worth the lingering ones; a finger every turn.
        unsafe { channel() }.state.t.mph += 1;
        assert!(pace.due(&s, GUEST_MENU, false));
        unsafe { channel() }.poll();
        assert!(!pace.due(&s, GUEST_MENU, false));
        unsafe { channel() }.state.locate("", "Broadway", "W 42nd St", false);
        assert!(pace.due(&s, GUEST_MENU, false));
        unsafe { channel() }.poll();
        for _ in 0..LINGER {
            pace.due(&s, GUEST_MENU, false);
        }
        assert!(!pace.due(&s, GUEST_MENU, false));
        assert!(pace.due(&s, GUEST_MENU, true));

        // With no interface on the screen the pad keeps the flow: START steps the player's mode.
        unsafe { channel() }.close();
        let mut s = session();
        let mut g = game();
        for _ in 0..40 {
            s.run(&mut g, &Pad::default(), dt);
        }
        assert_eq!(s.mode, Mode::Play);
        let before = g.mode;
        s.run(&mut g, &Pad { buttons: pad::MENU, ..Pad::default() }, dt);
        assert_ne!(g.mode, before);
    }
}
