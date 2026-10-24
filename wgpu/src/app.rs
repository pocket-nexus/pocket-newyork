//! The shell: what `vita/src/main.rs` does a frame, for a tab and for the machine that builds the game.
//!
//! A frame is [`App::step`] (the commands the interface sent, the game and its world), the guest's turn (which the
//! page runs: [`App::guest_due`], [`App::heard`], [`App::say`]), then [`App::draw`]: the camera and the hour's
//! light, the shadow window, what the street hides, the sun's map, the sky, the city and what moves, the glow, and
//! the interface over it. The pack is read beside the frames ([`App::load`]): until it is in, a frame shows the
//! interface alone, saying what is being read.
//!
//! What differs from the PS Vita's frame: the world steps in the frame (a tab has one thread; the PS Vita steps it
//! on a thread of its own while it draws) and the highway's lanes a fourth as often in turn (an eighth there); the
//! reaches stay at the PS Vita's full reaches, with no governor; sixty frames a second.

use std::cell::RefCell;
use std::rc::Rc;

use ny_frame::horizon::Horizon;
use ny_frame::light::{bloom, frame_light, sun_disc};
use ny_frame::mat;
use ny_frame::movers::Movers;
use ny_frame::trees::{Reach as TreeReach, Trees};
use ny_frame::sun::SunView;
use ny_interface::{channel, pad, Mode, Pace, Pad, Session};
use ny_pack::{Instance, Preset};
use ny_sim::math::{v3, V3};
use ny_sim::{Game, Location};
use ny_view::Reach;
use pocket_web_wgpu::gpu::{Gpu, Screen};
use pocket_web_wgpu::overlay::Overlay;
use pocket_web_wgpu::task;
use serde_json::json;

use crate::city::{CityGpu, FrameU, Look, Show, Stats, SUN_SIDE};
use crate::pack::Pack;
use crate::post::Post;
use crate::shadow::Shadows;
use crate::sky::Sky;

/// The screen: the PS Vita's.
pub const WIDTH: u32 = 960;
pub const HEIGHT: u32 = 544;
/// Turns a second the interface's guest is offered, as the PS Vita's host offers them (`vita/src/interface.rs`).
pub const TURNS: u32 = 30;
/// The ground behind the interface while there is no city.
const GROUND: [f32; 3] = [0.035, 0.047, 0.067];
/// Frames over which the frame times are averaged.
const WINDOW: usize = 120;
/// The reaches of the levels of detail: the PS Vita's when its governor allows them in full.
const REACH: Reach = Reach { detail: 160.0, near: 420.0, mid: 2400.0, far: 7000.0 };
/// The trees drawn finer near the eye (`ny_frame::trees`): as far out as the reference draws its levels 0 (105 m)
/// and 1 (420 m), at most 128 trees at level 0 and 900 in all.
const TREES: TreeReach = TreeReach { fine: 105.0, fine_count: 128, reach: 420.0, count: 900 };

/// The pad as the page reads it: PocketJS's bits and two sticks in −1…1, right and up positive.
#[derive(Clone, Copy, Default)]
pub struct Held {
    pub buttons: u32,
    pub left: [f32; 2],
    pub right: [f32; 2],
}

// PocketJS's button bits (the PS Vita's own).
const P_SELECT: u32 = 0x1;
const P_START: u32 = 0x8;
const P_RIGHT: u32 = 0x20;
const P_LEFT: u32 = 0x80;
const P_L: u32 = 0x100 | 0x400;
const P_R: u32 = 0x200 | 0x800;
const P_CROSS: u32 = 0x4000;

/// The pad as the flow reads it (`session_pad` of the PS Vita): the left stick drives, the right stick turns the
/// camera, ✕ boosts, R and L climb and descend (R jumps on foot), the d-pad's left and right step the hour, SELECT the
/// camera, START the menu.
fn session_pad(h: &Held) -> Pad {
    let mut b = 0;
    for (bit, to) in [(P_CROSS, pad::BOOST), (P_R, pad::UP), (P_L, pad::DOWN), (P_LEFT, pad::EARLIER), (P_RIGHT, pad::LATER), (P_SELECT, pad::CAMERA), (P_START, pad::MENU)] {
        if h.buttons & bit != 0 {
            b |= to;
        }
    }
    Pad { buttons: b, lx: h.left[0], ly: h.left[1], rx: h.right[0], ry: h.right[1] }
}

/// What the pack gave, once it is in.
pub struct Loaded {
    presets: Vec<Preset>,
    world: ny_sim::world::World,
    city: CityGpu,
    sky: Sky,
    shadows: Shadows,
    pack_bytes: u64,
}

/// The read of the pack, beside the frames: what it is reading, and what it gave.
#[derive(Default)]
struct Loading {
    message: String,
    done: Option<Result<Loaded, String>>,
}

/// Reads the pack at `place` and puts the city on the GPU.
async fn load(gpu: Gpu, place: String, loading: Rc<RefCell<Loading>>) -> Result<Loaded, String> {
    let say = |text: String| loading.borrow_mut().message = text;
    say("Looking for the city".into());
    let pack = Pack::open(&place).await?;
    let presets: Vec<Preset> = ny_pack::table(&pack.read(ny_pack::PRST).await?);
    if presets.len() < 4 {
        return Err("the pack has fewer than four hours".into());
    }
    say("Reading the streets".into());
    // (what moves reads the sections after the pictures': the sim's, in one read)
    let tags = pack.tags();
    let sim: Vec<u32> = tags.iter().skip_while(|&&t| t != ny_pack::TDAT).skip(1).copied().collect();
    let mut sections = pack.read_run(&sim).await?;
    let world = ny_sim::world::World::from_sections(|tag| sections.remove(&tag))?;
    let city = CityGpu::load(&gpu, &pack, &say).await?;
    let sky = Sky::new(&gpu, &pack.read(ny_pack::SKYP).await?)?;
    say("Casting the first shadows".into());
    let start = &world.entities[1];
    let sun = presets[0].sun_dir;
    let shadows = Shadows::new(&gpu, &city.heights, &pack, v3(sun[0], sun[1], sun[2]), v3(start.pos[0], start.pos[1], start.pos[2])).await?;
    let pack_bytes = pack.source.length().await.unwrap_or(0);
    Ok(Loaded { presets, world, city, sky, shadows, pack_bytes })
}

/// The city once the pack is in, and what moves in it.
struct Live {
    presets: Vec<Preset>,
    game: Game,
    session: Session,
    city: CityGpu,
    sky: Sky,
    shadows: Shadows,
    movers: Movers,
    trees: Trees,
    horizon: Horizon,
    placed: Vec<(usize, Instance)>,
    located: Option<Location>,
    stats: Stats,
    pack_bytes: u64,
    /// Bytes of the pack read before the first frame of the city.
    read_at_start: u64,
}

/// What this shell sets beside the game, by control words.
struct Settings {
    show: Show,
    models: bool,
    post: bool,
    sun: bool,
    occlude: bool,
    /// A fixed eye (position, target), for captures, and its field of view (the game's when `None`); `None` follows
    /// the game.
    view: Option<(V3, V3)>,
    fov: Option<f32>,
}

pub struct App {
    pub gpu: Gpu,
    pub screen: Screen,
    overlay: Overlay,
    post: Post,
    loading: Rc<RefCell<Loading>>,
    live: Option<Live>,
    set: Settings,
    /// The interface's guest: turns owed (sixtieths of a second) and the buttons held since the last.
    guest: (u32, u32),
    pace: Pace,
    skipped: u32,
    touched: bool,
    prefs: Option<String>,
    pub trouble: String,
    /// The world's step, the frame's processor work, and the shadow window's sweep, in milliseconds (smoothed).
    sim_ms: f32,
    cpu_ms: f32,
    last: Option<f64>,
    began: f64,
    frames: u64,
    intervals: [f32; WINDOW],
    late: u32,
    dt: f32,
    loaded_ms: f64,
    opened_at: f64,
}

impl App {
    /// The shell with no city: its frames show the interface alone.
    pub fn open(gpu: Gpu, screen: Screen) -> App {
        let overlay = Overlay::new(&gpu, screen.format);
        let post = Post::new(&gpu, screen.format, screen.width, screen.height);
        App {
            gpu,
            screen,
            overlay,
            post,
            loading: Rc::new(RefCell::new(Loading::default())),
            live: None,
            set: Settings { show: Show { reach: REACH, walls: true, solids: true, instances: true, trees: 400.0 }, models: true, post: true, sun: true, occlude: true, view: None, fov: None },
            guest: (0, 0),
            pace: Pace::new(),
            skipped: 0,
            touched: false,
            prefs: None,
            trouble: String::new(),
            sim_ms: 0.0,
            cpu_ms: 0.0,
            last: None,
            began: 0.0,
            frames: 0,
            intervals: [1000.0 / 60.0; WINDOW],
            late: 0,
            dt: 1.0 / 60.0,
            loaded_ms: 0.0,
            opened_at: task::now(),
        }
    }

    /// Starts reading the pack at `place` (its file, or a manifest of its pieces and its name after `#`).
    pub fn load(&mut self, place: &str) {
        let (gpu, loading, place) = (self.gpu.clone(), self.loading.clone(), place.to_string());
        task::spawn(async move {
            let done = load(gpu, place, loading.clone()).await;
            loading.borrow_mut().done = Some(done);
        });
    }

    /// The city is in: the title and play draw it.
    pub fn entered(&self) -> bool {
        self.live.is_some()
    }

    /// The start failed: the interface says why.
    pub fn fail(&mut self, why: &str) {
        self.trouble = why.into();
    }

    /// The pack has been read: the game starts, as the PS Vita's does after its load.
    fn enter(&mut self, l: Loaded) {
        let mut game = Game::with_world(l.world);
        // The highway's 2 642 cars stepped a lane at a time, every fourth frame each.
        game.highway.every = 4;
        let mut session = Session::new();
        session.traffic = true;
        session.cameras = game.world.cameras.len() as u8;
        session.follow = 3;
        session.title_camera = game.world.camera_named("drone").map(|c| c as u8);
        session.places = game.world.teleports.iter().map(|t| [t.x, t.z, t.heading]).collect();
        let movers = Movers::new(|name| l.city.model(name));
        let trees = l.city.trees();
        let read_at_start = l.city.read_so_far().bytes;
        unsafe { channel() }.state.message.clear();
        self.live = Some(Live { presets: l.presets, game, session, city: l.city, sky: l.sky, shadows: l.shadows, movers, trees, horizon: Horizon::new(), placed: Vec::with_capacity(crate::city::MODEL_INSTANCES), located: None, stats: Stats::default(), pack_bytes: l.pack_bytes, read_at_start });
        self.loaded_ms = task::now() - self.opened_at;
    }

    /// Words from a development tool or a page's address: `start=car mode=play player=person|car|cyclist|helicopter
    /// hour=0..3 camera=n place=n teleport=x,z,heading view=x,y,z>x,y,z (a fixed eye and what it looks at; `view=`
    /// alone lets go) fov=degrees sun=0|1 occlude=0|1 post=0|1 models=0|1 traffic=0|1 near=m mid=m far=m detail=m`.
    pub fn control(&mut self, words: &str) {
        for word in words.split_ascii_whitespace() {
            let Some((k, v)) = word.split_once('=') else { continue };
            let list: Vec<f32> = v.split([',', '>']).filter_map(|x| x.parse().ok()).collect();
            let x = list.first().copied().unwrap_or(0.0);
            match k {
                "fov" => self.set.fov = (x > 0.0).then_some(x),
                "sun" => self.set.sun = x != 0.0,
                "occlude" => self.set.occlude = x != 0.0,
                "post" => self.set.post = x != 0.0,
                "models" => self.set.models = x != 0.0,
                "near" => self.set.show.reach.near = x,
                "mid" => self.set.show.reach.mid = x,
                "far" => self.set.show.reach.far = x,
                "detail" => self.set.show.reach.detail = x,
                "view" if list.len() >= 6 => self.set.view = Some((v3(list[0], list[1], list[2]), v3(list[3], list[4], list[5]))),
                "view" => {
                    self.set.view = None;
                    self.set.fov = None;
                }
                _ => {}
            }
            let Some(live) = &mut self.live else { continue };
            let game = &mut live.game;
            match k {
                "mode" => {
                    if let Some(m) = Mode::parse(v).filter(|m| !matches!(m, Mode::Loading | Mode::Error)) {
                        live.session.mode = m;
                    }
                }
                // (as the title's Play does: into play in that mode, on its follow camera)
                "start" => {
                    if let Some(p) = ny_interface::player(v) {
                        live.session.command(game, ny_interface::Command::Start(p));
                    }
                }
                "player" => {
                    if let Some(p) = ny_interface::player(v) {
                        game.set_mode(p);
                    }
                }
                "hour" => {
                    game.set_hour(ny_sim::Hour::ALL[(x as usize).min(3)]);
                }
                "camera" => {
                    game.set_camera(x as u8);
                }
                "place" => {
                    game.teleport_to(x as usize);
                }
                "teleport" if list.len() >= 3 => {
                    game.teleport(list[0], list[1], list[2]);
                }
                "traffic" => game.traffic_on = x != 0.0,
                _ => {}
            }
        }
    }

    // ---- the interface: a PocketJS guest the page runs, heard and answered through the channel

    /// A guest has opened the interface's channel: it is told the whole state on its next turn.
    pub fn interface_opened(&mut self) {
        unsafe { channel() }.open("pocket.overlay");
        self.guest = (60 / TURNS, 0);
        self.skipped = 0;
        self.pace = Pace::new();
    }

    /// The guest went with its realm; until another opens the channel the pad keeps the flow.
    pub fn interface_closed(&mut self) {
        unsafe { channel() }.close();
    }

    /// The guest's turn in this frame: the sixtieths of a second it is for and the buttons it is handed, or `None`.
    /// A turn is offered `TURNS` times a second and taken when it is worth its cost (`Pace`); the turns passed over
    /// are time the interface's clock is owed, no more than three of them.
    pub fn guest_due(&mut self, touching: bool) -> Option<(u32, u32)> {
        let turn = 60 / TURNS;
        let (owed, buttons) = &mut self.guest;
        *owed = (*owed).min(2 * turn);
        if *owed < turn {
            return None;
        }
        *owed -= turn;
        let buttons = core::mem::take(buttons);
        let contact = touching || self.touched;
        self.touched = touching;
        if let Some(live) = &self.live {
            if !self.pace.due(&live.session, buttons, contact) {
                self.skipped += 1;
                return None;
            }
        }
        Some((turn * (1 + core::mem::take(&mut self.skipped).min(3)), buttons))
    }

    /// The line of state the guest has not seen.
    pub fn heard(&mut self) -> Option<String> {
        unsafe { channel() }.poll()
    }

    /// A line the guest sent: a command for the next step.
    pub fn say(&mut self, line: &str) {
        unsafe { channel() }.receive(line);
    }

    /// The settings the page kept from the last visit.
    pub fn prefs_stored(&mut self, text: &str) {
        unsafe { channel() }.state.prefs = text.into();
    }

    /// What the interface asked to have kept since the last call.
    pub fn prefs_take(&mut self) -> Option<String> {
        self.prefs.take()
    }

    pub fn overlay(&mut self, pixels: &[u8], width: u32, height: u32) -> Result<(), String> {
        self.overlay.write(&self.gpu, pixels, width, height)
    }

    pub fn overlay_hide(&mut self) {
        self.overlay.hide();
    }

    // ---- a frame

    /// The first half of a frame at `now` (milliseconds on a clock that goes forward): what the interface asked
    /// for, the game. Returns the sixtieths of a second that passed.
    pub fn step(&mut self, now: f64, held: &Held) -> u32 {
        self.began = task::now();
        let interval = self.last.map(|last| (now - last) as f32).unwrap_or(1000.0 / 60.0);
        self.last = Some(now);
        let interval = if interval > 0.0 { interval } else { 1000.0 / 60.0 };
        self.intervals[self.frames as usize % WINDOW] = interval;
        if self.entered() && interval > 1.5 * 1000.0 / 60.0 {
            self.late += 1;
        }
        let ticks = ((interval * 0.06).round() as u32).clamp(1, 6);
        self.guest.0 += ticks;
        self.guest.1 |= held.buttons;
        self.dt = (interval / 1000.0).clamp(0.001, 0.1);

        if self.live.is_none() {
            let done = self.loading.borrow_mut().done.take();
            match done {
                Some(Ok(loaded)) => self.enter(loaded),
                Some(Err(e)) => self.trouble = e,
                None => {}
            }
        }
        let Some(live) = &mut self.live else {
            // A frame of the interface alone, while the pack is read (or after the start failed).
            let state = unsafe { &mut channel().state };
            if self.trouble.is_empty() {
                state.mode = Mode::Loading;
                state.message = self.loading.borrow().message.clone();
            } else {
                state.mode = Mode::Error;
                state.message = self.trouble.clone();
            }
            return ticks;
        };

        // What the interface asked for; what it wants kept comes back.
        if let Some(text) = live.session.obey(&mut live.game, |_, _| {}) {
            unsafe { channel() }.state.prefs = text.clone();
            self.prefs = Some(text);
        }
        let t = task::now();
        live.session.run(&mut live.game, &session_pad(held), self.dt);
        self.sim_ms += ((task::now() - t) as f32 - self.sim_ms) * 0.1;

        // What the interface shows.
        let state = unsafe { &mut channel().state };
        live.session.publish(&live.game, state);
        // Where the player is, by the names of the reference's places and streets.
        let loc = live.game.location();
        if live.located != Some(loc) {
            live.located = Some(loc);
            let w = &live.game.world;
            let name = |n: Option<u16>| n.map(|n| w.name(n)).unwrap_or("");
            state.locate(name(loc.place), name(loc.avenue), name(loc.street), loc.near);
        }
        if !live.session.stats {
            state.stats.clear();
        } else if self.frames % 30 == 0 {
            let average = self.intervals.iter().sum::<f32>() / WINDOW as f32;
            let s = &live.stats;
            state.stats = format!("{:.1} fps · {:.1} ms · late {} · {} draws · {}k tris", 1000.0 / average.max(0.1), self.cpu_ms, self.late, s.draws, (s.tris[0] + s.tris[1] + s.tris[2]) / 1000);
        }
        ticks
    }

    /// The second half: the city, with the interface over it. Before the city is in, the interface alone.
    pub fn draw(&mut self) -> Result<(), String> {
        self.frames += 1;
        let Some(live) = self.live.as_mut() else {
            let target = self.screen.frame(&self.gpu)?;
            let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
            drop(target.pass(&mut encoder, GROUND));
            self.overlay.draw(&mut encoder, &target);
            self.gpu.queue.submit([encoder.finish()]);
            target.present();
            return Ok(());
        };
        let gpu = &self.gpu;
        let set = &self.set;

        // The camera and the hour's light.
        let view = live.game.view();
        let (eye, target, fov) = match set.view {
            Some((eye, target)) => (eye, target, set.fov.unwrap_or(view.fov)),
            None => (view.eye, view.look, view.fov),
        };
        let look_dir = (target - eye).normalize();
        let aspect = WIDTH as f32 / HEIGHT as f32;
        // (what is drawn is chosen through the PS Vita's projection; it is drawn through one whose depth runs from 1
        // at the near plane to 0 at infinity, which holds its precision far away)
        let vp = mat::mul(&mat::perspective(fov, aspect, 0.4, 30000.0), &mat::view(eye, look_dir));
        let drawn_vp = mat::mul(&reversed(fov, aspect, 0.4), &mat::view(eye, look_dir));
        let preset = &live.presets[live.game.hour.index()];
        let (mut frame, look, haze, night) = frame_light(preset, live.city.lamp_unit, eye, look_dir);
        let sun = v3(preset.sun_dir[0], preset.sun_dir[1], preset.sun_dir[2]);
        live.shadows.update(gpu, &live.city.heights, sun, eye);
        let map = live.shadows.mapping();
        frame[13..16].copy_from_slice(&map);
        let sky_vp = mat::mul(&mat::perspective(fov, aspect, 0.5, 4.0), &mat::view(V3::ZERO, look_dir));

        // What moves: the traffic (cars in view within the middle distance, at most 450 m), the sidewalks' people and
        // the highway's cars, and the player.
        live.placed.clear();
        let planes = mat::planes(&vp);
        live.movers.place(&live.game, eye, &planes, set.show.reach.mid.clamp(250.0, 450.0), self.dt, live.game.traffic_on && set.models, &mut live.placed);
        if !set.models {
            live.placed.clear();
        }
        // The trees nearest the eye, at the reference's finer levels.
        if set.models && set.show.instances && set.show.trees > 0.0 {
            live.city.place_trees(gpu, &mut live.trees, eye, &planes, &TREES, &mut live.placed);
        } else {
            live.city.no_trees();
        }
        // The sun's map around the eye (not under the moon: its light casts little); what the street hides.
        let sun_drawn = set.sun && preset.night < 0.5;
        let sun_view = if sun_drawn { SunView::place(eye, sun, SUN_SIDE) } else { SunView::off() };
        live.horizon.build(eye, if set.occlude { live.shadows.coarse() } else { None }.as_deref());
        let look_of = Look { vp: &vp, eye, frame: &frame, look: &look, haze: &haze, night, sun: &sun_view, horizon: &live.horizon, show: &set.show, frame_no: self.frames as u32 };
        let (stats, _) = live.city.prepare(gpu, &look_of, sun_drawn, &mut live.placed);
        live.stats = stats;
        // (the map's rows run with its y, as the PS Vita's viewport has them: its clip space's y turned over)
        let mut sun_draw = sun_view.draw;
        for k in 4..8 {
            sun_draw[k] = -sun_draw[k];
        }
        live.city.frame_uniforms(
            gpu,
            &FrameU {
                vp: drawn_vp,
                sun_read: sun_view.read,
                sun_draw,
                f: frame,
                tone: [look[0], look[1], look[2], look[3]],
                sun: [look[4], look[5], look[6], if night { 1.0 } else { 0.0 }],
                lamps: [look[8], live.shadows.unit(), 0.0, 0.0],
                lamp_map: live.city.lamp_map,
            },
        );
        live.sky.prepare(gpu, &sky_vp, live.game.hour.index(), &sun_disc(preset));

        let target = self.screen.frame(gpu)?;
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        if sun_drawn && live.city.sun_draws() {
            let mut pass = encoder.begin_render_pass(&pocket_web_wgpu::wgpu::RenderPassDescriptor {
                label: Some("the sun's map"),
                color_attachments: &[],
                depth_stencil_attachment: Some(pocket_web_wgpu::wgpu::RenderPassDepthStencilAttachment { view: &live.city.sun_map, depth_ops: Some(pocket_web_wgpu::wgpu::Operations { load: pocket_web_wgpu::wgpu::LoadOp::Clear(1.0), store: pocket_web_wgpu::wgpu::StoreOp::Store }), stencil_ops: None }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            live.city.draw_sun(&mut pass);
        }
        {
            let mut pass = self.post.scene_pass(&mut encoder);
            live.sky.draw(&mut pass);
            live.city.draw(&mut pass);
        }
        // What is bright in the frame glows over it, as the reference's bloom does for the hour.
        let (gain, threshold) = bloom(preset);
        self.post.finish(gpu, &mut encoder, &target, if set.post { gain } else { 0.0 }, threshold);
        self.overlay.draw(&mut encoder, &target);
        gpu.queue.submit([encoder.finish()]);
        target.present();
        self.cpu_ms += ((task::now() - self.began) as f32 - self.cpu_ms) * 0.1;
        Ok(())
    }

    /// The run as a JSON object.
    pub fn status(&self) -> String {
        let average = self.intervals.iter().sum::<f32>() / WINDOW as f32;
        let worst = self.intervals.iter().fold(0.0f32, |a, &b| a.max(b));
        let mut status = json!({
            "target": "web",
            "stage": if self.entered() { "running" } else if self.trouble.is_empty() { "loading" } else { "error" },
            "message": self.loading.borrow().message,
            "frames": self.frames, "frameMs": average, "worstMs": worst, "late": self.late, "cpuMs": self.cpu_ms, "simMs": self.sim_ms,
            "loadMs": self.loaded_ms,
            "interface": {"open": unsafe { channel() }.is_open()},
            "adapter": self.gpu.adapter,
            "blocks": self.gpu.features.contains(pocket_web_wgpu::wgpu::Features::TEXTURE_COMPRESSION_BC),
            "trouble": self.trouble,
        });
        if let Some(live) = &self.live {
            let view = live.game.view();
            let r = live.game.readout();
            let s = &live.stats;
            let (filled, slots) = live.city.slots.filled();
            status["mode"] = json!(live.session.mode.name());
            // (nothing is being read around the eye, and every cell within the near reach is in)
            status["settled"] = json!(live.city.wanting == 0 && live.city.slots.reading() == 0 && !live.shadows.reading());
            status["camera"] = json!({"pos": [view.eye.x, view.eye.y, view.eye.z], "target": [view.look.x, view.look.y, view.look.z], "fov": view.fov});
            status["game"] = json!({"player": r.mode.name(), "hour": live.game.hour.index(), "camera": live.game.camera, "mph": r.mph, "at": r.at, "label": live.game.label()});
            status["city"] = json!({"draws": s.draws, "tris": {"wall": s.tris[0], "solid": s.tris[1], "instanced": s.tris[2], "sun": s.sun}, "places": s.places});
            status["near"] = json!({"slots": [filled, slots], "reads": live.city.slots.reads, "mb": live.city.slots.bytes as f32 / 1e6, "starved": live.city.slots.starved});
            status["shadows"] = json!({"sweeps": live.shadows.sweeps, "sweepMs": live.shadows.sweep_ms});
            status["memory"] = json!({"geometry": live.city.geometry_bytes, "textures": live.city.texture_bytes});
            let read = live.city.read_so_far();
            status["pack"] = json!({"bytes": live.pack_bytes, "read": {"requests": read.requests, "bytes": read.bytes, "atStart": live.read_at_start}});
        } else {
            status["mode"] = json!(if self.trouble.is_empty() { "loading" } else { "error" });
            status["settled"] = json!(false);
        }
        status.to_string()
    }
}

/// A perspective whose depth runs from 1 at `near` to 0 at infinity, looking down −z (rows, as `mat` has them).
fn reversed(fov_y_deg: f32, aspect: f32, near: f32) -> mat::Mat4 {
    let f = 1.0 / (fov_y_deg.to_radians() * 0.5).tan();
    [f / aspect, 0.0, 0.0, 0.0, 0.0, f, 0.0, 0.0, 0.0, 0.0, 0.0, near, 0.0, 0.0, -1.0, 0.0]
}
