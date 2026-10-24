//! Pocket New York on PS Vita.
//!
//! A frame at 960 × 544, sixty times a second: the sky, the city's cells, blocks and regions at the level of detail
//! their distance asks for, then the interface. Every 2D pixel is the interface's: the PocketJS guest of `ui/`
//! (`interface.rs`). What moves is `ny_sim::Game` (the reference's controllers, cameras and traffic), and the flow
//! around it (the title, play, the menu) is `ny_interface::Session`, as on every other device.
//!
//! Development loop over PocketJS's wired debug transport: the pack is copied once from the USB share
//! (`host0:newyork/city.pack`) to the memory card and read from there, the interface's bundle is read from the share
//! (`host0:newyork/newyork.js`, `newyork.pak`), `host0:newyork/control.json` steers the run, and status receipts carry
//! frame timings under `engine`.

mod city;
mod glow;
mod gpu;
mod hostfs;
mod interface;
mod near;
mod paths;
mod shadow;
mod sunmap;
mod sky;

use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use city::{CityGpu, PackFile, Show};
use gpu::Gpu;
use ny_frame::light::{bloom, frame_light, sun_disc};
use ny_frame::movers::Movers;
use ny_frame::trees::Reach as TreeReach;
use ny_frame::{horizon, mat};
use ny_interface::{channel, pad, Mode, Pad, Session};
use ny_pack::Preset;
use ny_sim::math::{v3, V3};
use ny_sim::Game;
use ny_view::Reach;
use pocket_vita_gxm::mem::{Arena, Kind};
use pocket_vita_gxm::target::Fence;
use pocketjs_vita::{dev, dev_protocol::Op, devmenu::Action, graphics, input};
use serde_json::{json, Value};
use vita2d_sys as g;

#[no_mangle]
#[used]
pub static sceUserMainThreadStackSize: u32 = 1024 * 1024;

#[no_mangle]
#[used]
pub static _newlib_heap_size_user: u32 = 96 * 1024 * 1024;

extern "C" {
    fn scePowerSetArmClockFrequency(freq: i32) -> i32;
    fn scePowerSetBusClockFrequency(freq: i32) -> i32;
    fn scePowerSetGpuClockFrequency(freq: i32) -> i32;
    fn scePowerSetGpuXbarClockFrequency(freq: i32) -> i32;
    fn scePowerGetArmClockFrequency() -> i32;
    fn scePowerGetGpuClockFrequency() -> i32;
    fn sceDisplayGetVcount() -> i32;
    fn sceDisplayWaitVblankStart() -> i32;
    fn sceKernelPowerTick(kind: i32) -> i32;
}

/// Bytes of the parameter buffer: where GXM keeps a scene's transformed vertices until its tiles are drawn. When a
/// scene fills it, GXM draws what it has and starts again, at several times the cost.
const PARAMETER_BUFFER: u32 = 32 * 1024 * 1024;

extern "C" {
    fn __real_sceGxmInitialize(params: *const g::SceGxmInitializeParams) -> i32;
}

/// vita2d's start of GXM, with the city's parameter buffer (see `build.rs`).
///
/// # Safety
/// Called by vita2d in place of `sceGxmInitialize`, with its parameters.
#[no_mangle]
pub unsafe extern "C" fn __wrap_sceGxmInitialize(params: *const g::SceGxmInitializeParams) -> i32 {
    let mut p = *params;
    p.parameterBufferSize = PARAMETER_BUFFER;
    __real_sceGxmInitialize(&p)
}

extern "C" {
    fn __real_sceGxmCreateContext(params: *const g::SceGxmContextParams, context: *mut *mut g::SceGxmContext) -> i32;
}

/// The context's ring buffers: a frame of the city (the sun's map and the scene, some 1 500 draws and their
/// uniforms) outgrows vita2d's (128 KiB of VDM commands, 2 MiB of vertex and 512 KiB of fragment uniforms) with
/// two frames in flight, and the CPU would wait on the GPU for room.
const VDM_RING: usize = 1024 * 1024;
const VERTEX_RING: usize = 6 * 1024 * 1024;
const FRAGMENT_RING: usize = 3 * 1024 * 1024;

/// # Safety
/// Called by vita2d in place of `sceGxmCreateContext`, with its parameters; the rings it is given live as long as
/// the process.
#[no_mangle]
pub unsafe extern "C" fn __wrap_sceGxmCreateContext(params: *const g::SceGxmContextParams, context: *mut *mut g::SceGxmContext) -> i32 {
    let mut p = *params;
    let ring = |size: usize| -> Option<*mut core::ffi::c_void> {
        let mut block = pocket_vita_gxm::mem::Block::with_access(Kind::Main, size, false).ok()?;
        let at = block.alloc(size, 4096)?;
        core::mem::forget(block);
        Some(at.cast())
    };
    if let (Some(vdm), Some(vertex), Some(fragment)) = (ring(VDM_RING), ring(VERTEX_RING), ring(FRAGMENT_RING)) {
        p.vdmRingBufferMem = vdm;
        p.vdmRingBufferMemSize = VDM_RING as u32;
        p.vertexRingBufferMem = vertex;
        p.vertexRingBufferMemSize = VERTEX_RING as u32;
        p.fragmentRingBufferMem = fragment;
        p.fragmentRingBufferMemSize = FRAGMENT_RING as u32;
    }
    __real_sceGxmCreateContext(&p, context)
}

/// The world's step (the traffic, the people, the highway) on a thread of its own, alongside a frame's drawing: the
/// frame hands it the game after placing what moves and takes it back before it reads the game again.
struct WorldThread {
    kick: mpsc::Sender<(usize, ny_sim::game::WorldStep)>,
    done: mpsc::Receiver<()>,
    out: Option<ny_sim::game::WorldStep>,
}

impl WorldThread {
    fn new() -> WorldThread {
        let (kick, jobs) = mpsc::channel::<(usize, ny_sim::game::WorldStep)>();
        let (finished, done) = mpsc::channel();
        let _ = std::thread::Builder::new().name("newyork-world".into()).stack_size(256 * 1024).spawn(move || {
            while let Ok((game, w)) = jobs.recv() {
                // Safety: the frame does not touch the game between `kick` and `join`.
                unsafe { (*(game as *mut Game)).step_world(&w) };
                if finished.send(()).is_err() {
                    break;
                }
            }
        });
        WorldThread { kick, done, out: None }
    }

    /// Hands `game` to the thread for `w`. Nothing else may touch it until `join`.
    fn kick(&mut self, game: &mut Game, w: ny_sim::game::WorldStep) {
        if self.kick.send((game as *mut Game as usize, w)).is_ok() {
            self.out = Some(w);
        }
    }

    /// Waits for the step handed over, if any, and gives back what it was for.
    fn join(&mut self) -> Option<ny_sim::game::WorldStep> {
        let w = self.out.take()?;
        let _ = self.done.recv();
        Some(w)
    }
}

/// Samples per pixel of the display surface the scene is drawn into (4 costs the street's frame some 2.5 ms more).
const DEFAULT_MSAA: u64 = 2;

/// vita2d's pool of temporary vertices, which the interface and the Devkit menu draw from. A frame takes half of it,
/// in turn (see the display scene).
const POOL_BYTES: u32 = 2 * 1024 * 1024;

/// ARM, bus, GPU and GPU crossbar clocks (MHz) the frame budget assumes.
const CLOCKS: [i32; 4] = [444, 222, 222, 166];

unsafe fn set_clocks() {
    scePowerSetArmClockFrequency(CLOCKS[0]);
    scePowerSetBusClockFrequency(CLOCKS[1]);
    scePowerSetGpuClockFrequency(CLOCKS[2]);
    scePowerSetGpuXbarClockFrequency(CLOCKS[3]);
}

// Vita controller bits.
const P_SELECT: u32 = 0x1;
const P_START: u32 = 0x8;
const P_UP: u32 = 0x10;
const P_RIGHT: u32 = 0x20;
const P_DOWN: u32 = 0x40;
const P_LEFT: u32 = 0x80;
const P_L: u32 = 0x100 | 0x400;
const P_R: u32 = 0x200 | 0x800;
const P_TRIANGLE: u32 = 0x1000;
const P_CIRCLE: u32 = 0x2000;
const P_CROSS: u32 = 0x4000;
const P_SQUARE: u32 = 0x8000;

unsafe fn text(font: *mut g::vita2d_pgf, x: i32, y: i32, color: u32, scale: f32, s: &str) {
    let c = std::ffi::CString::new(s.replace('\0', " ")).unwrap();
    g::vita2d_pgf_draw_text(font, x, y, color, scale, c.as_ptr());
}

/// What a frame needs before there is a city to draw: the interface, the wired-debug host and a count of frames.
struct Shell {
    ui: interface::Ui,
    dev: dev::Host,
    /// The system font, loaded when a frame has no interface to draw.
    font: *mut g::vita2d_pgf,
    frame: u32,
}

impl Shell {
    /// A frame of the interface alone, while the pack is copied and read (`Mode::Loading`, `message` the step) or
    /// after the start failed (`Mode::Error`, `message` the reason).
    unsafe fn frame(&mut self, mode: Mode, message: &str) {
        let state = &mut channel().state;
        state.mode = mode;
        if state.message != message {
            state.message.clear();
            state.message.push_str(message);
        }
        self.ui.turn(interface::TURN, &interface::NEUTRAL, true, None);
        if !self.ui.live() && self.font.is_null() {
            self.font = g::vita2d_load_default_pgf();
        }
        graphics::begin_frame(0xff11_0c09);
        if self.ui.live() {
            self.ui.draw();
        } else {
            // No interface: the system font says what is going on.
            text(self.font, 48, 80, 0xffff_ffff, 1.4, "Pocket New York");
            let chars: Vec<char> = message.chars().collect();
            for (i, line) in chars.chunks(90).take(4).enumerate() {
                text(self.font, 48, 130 + i as i32 * 28, 0xffd0_d0d0, 1.0, &line.iter().collect::<String>());
            }
        }
        self.dev.overlay();
        graphics::present();
        self.dev.engine = json!({"stage": if mode == Mode::Error { "error" } else { "loading" }, "message": message, "interface": {"up": self.ui.live(), "open": channel().is_open(), "error": self.ui.error}});
        self.dev.publish(self.frame, "newyork");
        serve(&mut self.dev, self.frame, Action::None);
        self.frame += 1;
    }

    /// The start failed: the reason stays on the screen and in the status receipt.
    unsafe fn fail(&mut self, error: String) -> ! {
        pocketjs_vita::vita_log(format_args!("newyork: {error}"));
        loop {
            self.frame(Mode::Error, &error);
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Copies `from` to `to` through a temporary file, a megabyte at a time, with a frame of the loading screen after
/// each: `said(megabytes done, megabytes a second)` is its line.
unsafe fn copy(from: &str, to: &str, shell: &mut Shell, said: impl Fn(f32, f32) -> String) -> Result<usize, String> {
    let mut src = std::fs::File::open(from).map_err(|e| format!("{from}: {e}"))?;
    let temp = format!("{to}.part");
    let mut dst = std::fs::File::create(&temp).map_err(|e| format!("{temp}: {e}"))?;
    let mut chunk = vec![0u8; 1024 * 1024];
    let (mut done, t) = (0usize, Instant::now());
    loop {
        let n = src.read(&mut chunk).map_err(|e| format!("{from}: {e}"))?;
        if n == 0 {
            break;
        }
        dst.write_all(&chunk[..n]).map_err(|e| format!("{temp}: {e}"))?;
        done += n;
        let mb = done as f32 / 1e6;
        shell.frame(Mode::Loading, &said(mb, mb / t.elapsed().as_secs_f32().max(0.001)));
    }
    drop(dst);
    let _ = std::fs::remove_file(to);
    std::fs::rename(&temp, to).map_err(|e| format!("{to}: {e}"))?;
    Ok(done)
}

/// Brings the pack on the computer to the memory card when it is not the one already there, and says which pack
/// to read.
unsafe fn sync_pack(live: bool, shell: &mut Shell) -> Result<&'static str, String> {
    if live {
        let _ = std::fs::create_dir_all(paths::DATA);
        let host = hostfs::read(paths::PACK_HOST_ID, 4096);
        let card = std::fs::read(paths::PACK_CARD_ID).ok();
        if let Some(id) = host {
            if Some(&id) != card.as_ref() {
                let total = serde_json::from_slice::<Value>(&id).ok().and_then(|v| v["bytes"].as_u64()).unwrap_or(0) as f32 / 1e6;
                copy(paths::PACK_HOST, paths::PACK_CARD, shell, |mb, rate| format!("Copying the city to the memory card: {mb:.0} of {total:.0} MB at {rate:.1} MB/s"))?;
                std::fs::write(paths::PACK_CARD_ID, &id).map_err(|e| format!("{}: {e}", paths::PACK_CARD_ID))?;
            }
        }
    }
    let first = if live { [paths::PACK_CARD, paths::PACK_APP] } else { [paths::PACK_APP, paths::PACK_CARD] };
    Ok(if std::fs::File::open(first[0]).is_ok() { first[0] } else { first[1] })
}

/// Copies a file the computer has put in the share's `outbox` to this app's folder on the memory card (a package,
/// for VitaShell to install), and leaves `<name>.done` beside the original.
unsafe fn fetch(name: &str, shell: &mut Shell) {
    let (from, to) = (format!("{}/outbox/{name}", paths::HOST), format!("{}/{name}", paths::DATA));
    let _ = std::fs::create_dir_all(paths::DATA);
    let said = match copy(&from, &to, shell, |mb, rate| format!("Copying {name} to the memory card: {mb:.0} MB at {rate:.1} MB/s")) {
        Ok(bytes) => format!("{bytes} bytes at {to}"),
        Err(e) => format!("failed: {e}"),
    };
    let _ = hostfs::write(&format!("{from}.done"), said.as_bytes());
    g::vita2d_wait_rendering_done();
}

/// Remote control: `host0:newyork/control.json`, polled off the render thread.
fn control_watcher() -> mpsc::Receiver<Value> {
    let (tx, rx) = mpsc::channel();
    let _ = std::thread::Builder::new().name("newyork-control".into()).stack_size(256 * 1024).spawn(move || {
        // What is there at launch is left over from an earlier run: only changes after it count.
        let path = format!("{}/control.json", paths::HOST);
        let mut last = hostfs::read(&path, 64 * 1024).unwrap_or_default();
        loop {
            std::thread::sleep(Duration::from_millis(300));
            if let Some(bytes) = hostfs::read(&path, 64 * 1024) {
                if bytes != last {
                    last = bytes.clone();
                    if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
                        if tx.send(v).is_err() {
                            return;
                        }
                    }
                }
            }
        }
    });
    rx
}

/// What this device sets beside the game.
struct Settings {
    /// Wait for the GPU after each frame and time it.
    profile: bool,
    show: Show,
    /// Display refreshes per frame: 1 is sixty frames a second.
    pace: i32,
    /// Draw the interface (off for a measurement of what it costs).
    interface: bool,
    /// Draw what moves, and the glow over the frame (off for measurements).
    models: bool,
    post: bool,
    /// Draw the sun's depth map (off for a measurement of what it costs); leave out what the street hides.
    sun: bool,
    occlude: bool,
    /// A fixed eye (position, target, fov), for captures and measurements; `None` follows the game.
    view: Option<(V3, V3, f32)>,
    /// Triangles a frame may draw: the reaches of the levels of detail follow it (and the GPU).
    budget: u32,
    govern: bool,
    /// The reaches when the budget allows them in full.
    full: Reach,
    /// The trees drawn finer near the eye (`ny_frame::trees`).
    trees: TreeReach,
}

/// Seconds a frame may wait for the GPU to finish the frame before last, per refresh it is paced to, before the
/// governor draws the levels of detail in.
const BEHIND: f32 = 0.006;

/// A control message's `press`: a mask of PocketJS button bits, or names.
fn press(ui: &mut interface::Ui, v: &Value) {
    if let Some(mask) = v.as_u64() {
        ui.press(mask as u32);
    }
    for name in v.as_array().into_iter().flatten().filter_map(Value::as_str) {
        ui.press(match name {
            "up" => P_UP,
            "right" => P_RIGHT,
            "down" => P_DOWN,
            "left" => P_LEFT,
            "l" => 0x100,
            "r" => 0x200,
            "triangle" => P_TRIANGLE,
            "circle" => P_CIRCLE,
            "cross" => P_CROSS,
            "square" => P_SQUARE,
            "start" => P_START,
            "select" => P_SELECT,
            _ => continue,
        });
    }
}

/// A message from the computer: `{"mode": "title" | "play" | "menu"}` sets the flow outright, `{"press": …}` and
/// `{"tap": [x, y]}` work the interface, `{"hour": 0..3}`, `{"player": "car"}`, `{"camera": n}`, `{"place": n}` and
/// `{"teleport": [x, z, heading]}` go to the game, `{"view": {"pos", "target", "fov"}}` fixes the eye (`null` lets go),
/// and the rest writes this device's settings (`near`, `mid`, `detail`, `walls`, `solids`, `instances`, `profile`,
/// `pace`, `interface`, `models`, `post`).
fn apply_control(v: &Value, s: &mut Settings, game: &mut Game, session: &mut Session, ui: &mut interface::Ui) {
    let flag = |k: &str, cur: bool| v[k].as_bool().unwrap_or(cur);
    let num = |k: &str, cur: f32| v[k].as_f64().map(|x| x as f32).unwrap_or(cur);
    s.profile = flag("profile", s.profile);
    s.interface = flag("interface", s.interface);
    s.models = flag("models", s.models);
    s.post = flag("post", s.post);
    s.sun = flag("sun", s.sun);
    s.occlude = flag("occlude", s.occlude);
    s.show.walls = flag("walls", s.show.walls);
    s.show.solids = flag("solids", s.show.solids);
    s.show.instances = flag("instances", s.show.instances);
    s.show.trees = num("trees", s.show.trees);
    s.trees.fine = num("treeFine", s.trees.fine);
    s.trees.fine_count = num("treeFineCount", s.trees.fine_count as f32) as usize;
    s.trees.reach = num("treeReach", s.trees.reach);
    s.trees.count = num("treeCount", s.trees.count as f32) as usize;
    // `near`, `mid` and `detail` set the reaches outright and stop the governor; `budget` hands them back to it.
    if v.get("near").is_some() || v.get("mid").is_some() || v.get("detail").is_some() || v.get("far").is_some() {
        s.show.reach.near = num("near", s.show.reach.near);
        s.show.reach.mid = num("mid", s.show.reach.mid);
        s.show.reach.detail = num("detail", s.show.reach.detail);
        s.show.reach.far = num("far", s.show.reach.far);
        s.govern = false;
    }
    if let Some(x) = v["budget"].as_u64() {
        s.budget = x as u32;
        s.govern = true;
    }
    if let Some(x) = v["pace"].as_i64() {
        s.pace = (x as i32).clamp(1, 4);
    }
    if let Some(mode) = v["mode"].as_str().and_then(Mode::parse).filter(|m| !matches!(m, Mode::Loading | Mode::Error)) {
        session.mode = mode;
    }
    if let Some(h) = v["hour"].as_u64() {
        game.set_hour(ny_sim::Hour::ALL[(h as usize).min(3)]);
    }
    if let Some(p) = v["player"].as_str().and_then(ny_interface::player) {
        game.set_mode(p);
    }
    if let Some(c) = v["camera"].as_u64() {
        game.set_camera(c as u8);
    }
    if let Some(p) = v["place"].as_u64() {
        game.teleport_to(p as usize);
    }
    if let Some(on) = v["crowd"].as_bool() {
        game.crowd_on = on;
    }
    if let Some(on) = v["highway"].as_bool() {
        game.highway_on = on;
    }
    if let Some(t) = v["teleport"].as_array() {
        let f = |i: usize| t.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32;
        game.teleport(f(0), f(1), f(2));
    }
    if v["gc"] == Value::Bool(true) {
        // Safety: the render thread, between two turns.
        unsafe { ui.collect() };
    }
    press(ui, &v["press"]);
    if let (Some(x), Some(y)) = (v["tap"][0].as_u64(), v["tap"][1].as_u64()) {
        ui.tap(x as u16, y as u16);
    }
    let f = |a: &Value, i: usize| a.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32;
    if v.get("view").is_some() {
        s.view = match (&v["view"]["pos"], &v["view"]["target"]) {
            (p, t) if p.is_array() && t.is_array() => Some((v3(f(p, 0), f(p, 1), f(p, 2)), v3(f(t, 0), f(t, 1), f(t, 2)), v["view"]["fov"].as_f64().unwrap_or(56.0) as f32)),
            _ => None,
        };
    }
}

/// Display refreshes from the count `then` to the count `now`. The display's counter has 16 bits: it starts again
/// at 0 after 65 536 refreshes (18 minutes).
fn refreshes(now: i32, then: i32) -> i32 {
    now.wrapping_sub(then) & 0xffff
}

/// The pad as the flow reads it: the left stick drives, the right stick turns the camera, ✕ boosts, R and L climb
/// and descend (R jumps on foot), the d-pad's left and right step the hour, SELECT the camera, START the menu.
fn session_pad(p: &input::Pad) -> Pad {
    let mut b = 0;
    for (bit, to) in [(P_CROSS, pad::BOOST), (P_R, pad::UP), (P_L, pad::DOWN), (P_LEFT, pad::EARLIER), (P_RIGHT, pad::LATER), (P_SELECT, pad::CAMERA), (P_START, pad::MENU)] {
        if p.buttons & bit != 0 {
            b |= to;
        }
    }
    let axis = |v: u8| {
        let x = (v as f32 - 127.5) / 127.5;
        // The sticks rest off centre by up to a fifth of their travel: nothing inside that counts.
        const DEAD: f32 = 0.24;
        if x.abs() < DEAD {
            0.0
        } else {
            (x - DEAD * x.signum()) / (1.0 - DEAD)
        }
    };
    Pad { buttons: b, lx: axis(p.lx), ly: -axis(p.ly), rx: axis(p.rx), ry: -axis(p.ry) }
}

/// Rolling frame statistics over the last `N` frames.
struct Timing {
    ms: [f32; Timing::N],
    at: usize,
    late: u32,
    frames: u32,
}

impl Timing {
    const N: usize = 240;
    fn push(&mut self, ms: f32, late: bool) {
        self.ms[self.at] = ms;
        self.at = (self.at + 1) % Self::N;
        self.frames += 1;
        if late {
            self.late += 1;
        }
    }
    fn avg(&self) -> f32 {
        self.ms.iter().sum::<f32>() / Self::N as f32
    }
    fn worst(&self) -> f32 {
        self.ms.iter().fold(0.0, |a, &b| a.max(b))
    }
}

fn main() {
    unsafe {
        // Development builds take boot switches from the USB share: {"msaa": 0 | 2 | 4, "title": false, "mode": "play",
        // "defines": [..]}.
        let live = cfg!(feature = "usb-debug");
        let boot: Value = if live { hostfs::read(&format!("{}/boot.json", paths::HOST), 4096).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null) } else { Value::Null };
        let samples = boot["msaa"].as_u64().unwrap_or(DEFAULT_MSAA);
        // The Pocket3D title card plays before the renderer starts. A development build skips it with {"title": false}.
        if !(live && boot["title"] == Value::Bool(false)) {
            pocket3d_title::vita::play();
        }
        // The display's own surface is multisampled: the scene is drawn straight into it.
        let display_msaa = match samples {
            4 => g::SceGxmMultisampleMode_SCE_GXM_MULTISAMPLE_4X,
            2 => g::SceGxmMultisampleMode_SCE_GXM_MULTISAMPLE_2X,
            _ => g::SceGxmMultisampleMode_SCE_GXM_MULTISAMPLE_NONE,
        };
        if g::vita2d_init_advanced_with_msaa(POOL_BYTES, display_msaa) < 0 {
            pocketjs_vita::vita_log(format_args!("newyork: vita2d did not start"));
            return;
        }
        if let Err(error) = graphics::init_with_pool(POOL_BYTES) {
            pocketjs_vita::vita_log(format_args!("newyork: graphics {error}"));
            return;
        }
        set_clocks();
        input::init();
        // The interface comes up first: it shows the load, and a failure.
        let mut shell = Shell { ui: interface::Ui::boot(), dev: dev::Host::new(), font: core::ptr::null_mut(), frame: 0 };
        channel().state.prefs = std::fs::read_to_string(format!("{}/{}", paths::DATA, paths::INTERFACE_FILE)).unwrap_or_default();

        // ------------------------------------------------------------------ load
        let t_load = Instant::now();
        shell.frame(Mode::Loading, "Looking for the city");
        let pack_path = match sync_pack(live, &mut shell) {
            Ok(p) => p,
            Err(e) => shell.fail(e),
        };
        let copy_ms = t_load.elapsed().as_millis() as u64;
        let loaded = (|| -> Result<_, String> {
            let mut p = PackFile::open(pack_path)?;
            let meta: Value = serde_json::from_slice(&p.read(ny_pack::META)?).map_err(|e| e.to_string())?;
            let presets: Vec<Preset> = ny_pack::table(&p.read(ny_pack::PRST)?);
            if presets.len() < 4 {
                return Err("the pack has fewer than four hours".into());
            }
            shell.frame(Mode::Loading, "Reading the streets");
            let world = ny_sim::world::World::from_sections(|tag| p.read_opt(tag))?;
            let mut gpu = Gpu::new(live)?;
            let mut vram = Arena::new(Kind::Cdram, 4 * 1024 * 1024);
            let msaa = display_msaa;
            let heights: ny_pack::Heights = ny_pack::read(&p.read_range(ny_pack::HMAP, 0, 32)?, 0).ok_or("heights record")?;
            let shadow_unit = 65535.0 * heights.unit;
            // {"defines": ["NAME", …]} compiles the scene programs with those defined (frame.cg's probes).
            let extra: String = boot["defines"].as_array().into_iter().flatten().filter_map(Value::as_str).map(|d| format!("#define {d}\n")).collect();
            let city = CityGpu::load(&mut p, &mut gpu, &mut vram, msaa, shadow_unit, &extra, |line| shell.frame(Mode::Loading, line))?;
            let sky = sky::Sky::new(&mut gpu, &mut vram, msaa, &p.read(ny_pack::SKYP)?)?;
            let mut targets = Arena::new(Kind::Main, 4 * 1024 * 1024);
            let glow = glow::Glow::new(&mut gpu, &mut vram, &mut targets, msaa)?;
            let sunmap = sunmap::SunMap::new(&mut vram, &mut targets)?;
            gpu.finish();
            shell.frame(Mode::Loading, "Casting the first shadows");
            let start = &world.entities[1];
            let sun = presets[0].sun_dir;
            let shadows = shadow::Shadows::new(&mut vram, pack_path, heights, (p.range(ny_pack::HMAP)?.0 + 32) as u64, v3(sun[0], sun[1], sun[2]), v3(start.pos[0], start.pos[1], start.pos[2]))?;
            Ok((meta, presets, world, gpu, city, vram, sky, shadows, glow, targets, sunmap))
        })();
        let (meta, presets, world, gpu, mut city, vram, sky, mut shadows, mut glow, _targets, mut sunmap) = match loaded {
            Ok(x) => x,
            Err(e) => shell.fail(e),
        };
        let load_ms = t_load.elapsed().as_millis() as u64;
        let pack_id: Value = std::fs::read(paths::PACK_CARD_ID).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);

        // What moves, as instances of the pack's models.
        let mut movers = Movers::new(|name| city.model(name));
        let mut trees = city.trees();
        let mut fine_trees = 0usize;
        let mut placed: Vec<(usize, ny_pack::Instance)> = Vec::with_capacity(city::MODEL_INSTANCES);
        let mut sun_tris = 0u32;
        let mut shown_late = false;
        // Milliseconds (smoothed) the frame waited for the world's thread.
        let mut joined_ms = 0.0f32;
        // Milliseconds (smoothed) of the frame's CPU parts: placing what moves, the sun's pass, the city's, the models'.
        let mut cpu_part = [0.0f32; 4];
        let mut game = Game::with_world(world);
        let mut world_thread = WorldThread::new();
        let mut horizon = horizon::Horizon::new();
        let mut horizon_ms = 0.0f32;
        // The highway's 2 642 cars cost this CPU some 12 ms a frame stepped together: a lane at a time, every eighth frame
        // each.
        game.highway.every = 8;
        let mut session = Session::new();
        session.traffic = true;
        session.cameras = game.world.cameras.len() as u8;
        session.follow = 3;
        session.title_camera = game.world.camera_named("drone").map(|c| c as u8);
        session.places = game.world.teleports.iter().map(|t| [t.x, t.z, t.heading]).collect();
        if let Some(mode) = boot["mode"].as_str().and_then(Mode::parse).filter(|m| !matches!(m, Mode::Loading | Mode::Error)) {
            session.mode = mode;
        }
        channel().state.message.clear();

        let mut fence = Fence::new(0, 2);
        let control = if live { control_watcher() } else { mpsc::channel().1 };
        let mut set = Settings {
            profile: false,
            show: Show { reach: Reach { detail: 110.0, near: 260.0, mid: 1400.0, far: 7000.0 }, walls: true, solids: true, instances: true, trees: 400.0 },
            // Thirty frames a second: the city takes what a second refresh gives it.
            pace: boot["pace"].as_i64().unwrap_or(2) as i32,
            interface: true,
            models: true,
            post: true,
            sun: true,
            occlude: true,
            view: None,
            budget: 300_000,
            govern: true,
            full: Reach { detail: 160.0, near: 420.0, mid: 2400.0, far: 7000.0 },
            // The reference draws its trees at level 0 within 105 m and level 1 within 420 m; here level 0 to the
            // nearest ten within 35 m and level 1 to 200 in all within 200 m (a tree is some 1 500 triangles at
            // level 0, 360 at level 1, 50 as the cells' crown). In Central Park at the golden hour the GPU took 51 ms
            // a frame so, 61 with the crowns alone: their sparse cards cost less than the crowns' dense ones.
            trees: TreeReach { fine: 35.0, fine_count: 10, reach: 200.0, count: 200 },
        };
        let mut scale = 1.0f32;
        let mut hold = 0u32;

        let ctx = g::vita2d_get_context();
        let mut timing = Timing { ms: [16.7; Timing::N], at: 0, late: 0, frames: 0 };
        let mut last = Instant::now();
        let mut last_vcount = sceDisplayGetVcount();
        let (mut draw_ms, mut gpu_ms, mut ui_ms, mut ui_draw_ms, mut sim_ms) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let mut stats = city::Stats::default();
        let mut clock_tick = 0u32;
        let mut frame_no = shell.frame;
        let (mut late_cpu, mut late_gpu) = (0u32, 0u32);
        let mut located: Option<ny_sim::Location> = None;
        g::vita2d_wait_rendering_done();
        shell.ui.collect();
        let mut mode_was = session.mode;

        loop {
            // -------------------------------------------------------------- input and the game
            let t_frame = Instant::now();
            let raw = input::read();
            let (buttons, action) = shell.dev.menu.input(raw.buttons);
            let menu = shell.dev.menu.visible;
            let pad = if menu { interface::NEUTRAL } else { input::Pad { buttons, ..raw } };
            while let Ok(v) = control.try_recv() {
                if let Some(name) = v["fetch"].as_str().filter(|n| !n.contains('/') && !n.contains("..")) {
                    shell.frame = frame_no;
                    fetch(name, &mut shell);
                    frame_no = shell.frame;
                }
                apply_control(&v, &mut set, &mut game, &mut session, &mut shell.ui);
            }
            if let Some(text) = session.obey(&mut game, |_, _| {}) {
                paths::write_text(paths::INTERFACE_FILE, &text);
                channel().state.prefs = text;
            }
            if session.mode != mode_was {
                if session.mode == Mode::Title {
                    shell.ui.collect();
                }
                mode_was = session.mode;
            }
            let vcount = sceDisplayGetVcount();
            let vblanks = refreshes(vcount, last_vcount).clamp(1, 4);
            last_vcount = vcount;
            let dt = vblanks as f32 / 60.0;
            let ts = Instant::now();
            let world_step = session.run_now(&mut game, &session_pad(&pad), dt);
            sim_ms = sim_ms * 0.9 + ts.elapsed().as_secs_f32() * 100.0;

            // -------------------------------------------------------------- the interface
            let tu = Instant::now();
            {
                let state = &mut channel().state;
                session.publish(&game, state);
                // Where the player is, by the names of the reference's places and streets.
                let loc = game.location();
                if located != Some(loc) {
                    located = Some(loc);
                    let w = &game.world;
                    let name = |n: Option<u16>| n.map(|n| w.name(n)).unwrap_or("");
                    state.locate(name(loc.place), name(loc.avenue), name(loc.street), loc.near);
                }
                if !session.stats {
                    state.stats.clear();
                } else if frame_no % 30 == 0 {
                    state.stats = format!("{:.1} fps · {:.1} ms · late {} · {} draws · {}k tris", 1000.0 / timing.avg().max(0.1), timing.avg(), timing.late, stats.draws, (stats.tris[0] + stats.tris[1] + stats.tris[2]) / 1000);
                }
            }
            shell.ui.turn(dt, &pad, menu, Some(&session));
            ui_ms = ui_ms * 0.9 + tu.elapsed().as_secs_f32() * 100.0;

            // -------------------------------------------------------------- camera and light
            let view = game.view();
            let (eye, target, fov) = set.view.unwrap_or((view.eye, view.look, view.fov));
            let look_dir = (target - eye).normalize();
            let aspect = 960.0 / 544.0;
            let vp = mat::mul(&mat::perspective(fov, aspect, 0.4, 30000.0), &mat::view(eye, look_dir));
            let preset = &presets[game.hour.index()];
            let (mut frame, look, haze, night) = frame_light(preset, city.lamp_unit, eye, look_dir);
            let sun = preset.sun_dir;
            shadows.update(v3(sun[0], sun[1], sun[2]), eye);
            let map = shadows.mapping();
            frame[13] = map[0];
            frame[14] = map[1];
            frame[15] = map[2];
            let sky_mvp = mat::mul(&mat::perspective(fov, aspect, 0.5, 4.0), &mat::view(V3::ZERO, look_dir));

            // -------------------------------------------------------------- the scene
            let t2 = Instant::now();
            let slot = (frame_no % 2) as usize;
            // This slot's vertices were last used two frames ago: the GPU must be done with them.
            fence.wait(slot);
            let waited = t2.elapsed();
            let tp = Instant::now();
            // What moves: the traffic (cars in view within the middle distance, at most 450 m: past it a car is a few
            // pixels), the sidewalks' people and the highway's cars, and the player.
            placed.clear();
            let planes = mat::planes(&vp);
            movers.place(&game, eye, &planes, set.show.reach.mid.clamp(250.0, 450.0), dt, game.traffic_on && set.models, &mut placed);
            // The trees nearest the eye, at the reference's finer levels.
            let moving = placed.len();
            if set.models && set.show.instances && set.show.trees > 0.0 {
                city.place_trees(&mut trees, eye, &planes, &set.trees, &mut placed);
            } else {
                city.no_trees();
            }
            fine_trees = placed.len() - moving;
            let hour_shown = game.hour.index();
            let sun_disc = sun_disc(preset);
            // The world's step for this frame on its own thread while the frame draws; the game is the thread's until
            // the join below (what moves is drawn as it stood after the frame before's step).
            if let Some(w) = world_step {
                world_thread.kick(&mut game, w);
            }
            let groups = city.prepare_models(&mut placed);
            let smooth = |a: &mut f32, t: Instant| *a = *a * 0.9 + t.elapsed().as_secs_f32() * 100.0;
            smooth(&mut cpu_part[0], tp);
            let ts = Instant::now();
            // The sun's depth map around the eye, before the frame (not under the moon: its light casts little).
            if set.sun && preset.night < 0.5 {
                sunmap.place(eye, v3(sun[0], sun[1], sun[2]));
                if sunmap.begin(ctx).is_ok() {
                    sun_tris = city.draw_sun(ctx, &sunmap, eye, &groups);
                    sunmap.end(ctx);
                }
            } else {
                sunmap.off();
                sun_tris = 0;
            }
            smooth(&mut cpu_part[1], ts);
            // What is bright in the frame on display glows over this one, as the reference's bloom does for the hour.
            let (glow_gain, glow_threshold) = bloom(preset);
            if let Err(e) = if set.post { glow.run(ctx, glow_gain, glow_threshold, g::vita2d_get_current_fb(), 960) } else { Ok(()) } {
                pocketjs_vita::vita_log(format_args!("newyork: {e}"));
            }
            g::vita2d_pool_reset();
            if slot == 1 {
                g::vita2d_pool_malloc(POOL_BYTES / 2);
            }
            g::vita2d_start_drawing_advanced(core::ptr::null_mut(), 0);
            // The sky covers the whole frame: nothing is cleared.
            g::sceGxmSetViewport(ctx, 480.0, 480.0, 272.0, -272.0, 0.0, 1.0);
            sky.draw(ctx, &sky_mvp, hour_shown, &sun_disc);
            let th = Instant::now();
            // What the street hides (from above the city, nothing).
            horizon.build(eye, if set.occlude { shadows.coarse() } else { None }.as_deref());
            horizon_ms = horizon_ms * 0.9 + th.elapsed().as_secs_f32() * 100.0;
            let tc = Instant::now();
            stats = city.draw(ctx, &vp, eye, &frame, &look, &haze, night, shadows.texture(), &sunmap, &horizon, &set.show, frame_no);
            smooth(&mut cpu_part[2], tc);
            if set.models {
                let tm = Instant::now();
                stats.tris[2] += city.draw_models(ctx, &vp, &frame, &look, &haze, night, shadows.texture(), &sunmap, &groups);
                smooth(&mut cpu_part[3], tm);
            }
            if set.post {
                glow.add(ctx, glow_gain);
            }
            // vita2d draws (the interface, the menu) expect its own viewport and no depth.
            g::sceGxmSetViewport(ctx, 480.0, 480.0, 272.0, -272.0, 0.5, 0.5);
            gpu::state_overlay(ctx, false);
            let td = Instant::now();
            if set.interface {
                shell.ui.draw();
            }
            ui_draw_ms = ui_draw_ms * 0.9 + td.elapsed().as_secs_f32() * 100.0;
            shell.dev.overlay();
            g::sceGxmEndScene(ctx, core::ptr::null(), fence.signal(slot));
            draw_ms = draw_ms * 0.9 + t2.elapsed().as_secs_f32() * 100.0;
            if set.profile {
                let tg = Instant::now();
                fence.wait(slot);
                gpu_ms = gpu_ms * 0.9 + tg.elapsed().as_secs_f32() * 100.0;
            }
            // The reaches follow the triangle budget: in quickly when a frame draws too many, out slowly when there is
            // room; they also come in when this frame had to wait for the GPU or the last one was shown late (the
            // CPU's work grows with the reaches too: the draws, the sun's pass), and stay in for two seconds after.
            if set.govern {
                let drawn = stats.tris[0] + stats.tris[1] + stats.tris[2];
                let behind = !set.profile && (waited.as_secs_f32() > BEHIND * set.pace as f32 || shown_late);
                if behind {
                    hold = (120 / set.pace) as u32;
                }
                if drawn > set.budget {
                    scale *= 0.97;
                } else if behind {
                    scale *= 0.985;
                } else if (drawn as f32) < set.budget as f32 * 0.88 && hold == 0 {
                    scale *= 1.008;
                }
                hold = hold.saturating_sub(1);
                // (not below 0.45: what the street costs most, its trees and what moves, does not come in with the reaches)
                scale = scale.clamp(0.45, 1.0);
                set.show.reach = Reach { detail: set.full.detail * scale.max(0.6), near: set.full.near * scale, mid: set.full.mid * scale, far: set.full.far * (0.5 + 0.5 * scale) };
            }
            let worked = t_frame.elapsed().saturating_sub(waited);
            g::vita2d_swap_buffers();
            while refreshes(sceDisplayGetVcount(), last_vcount) < set.pace {
                sceDisplayWaitVblankStart();
            }

            let now = Instant::now();
            let shown = refreshes(sceDisplayGetVcount(), last_vcount);
            timing.push((now - last).as_secs_f32() * 1000.0, shown > set.pace);
            shown_late = shown > set.pace;
            if shown > set.pace {
                if worked.as_secs_f32() > 0.012 * set.pace as f32 {
                    late_cpu += 1;
                } else {
                    late_gpu += 1;
                }
            }
            last = now;

            // -------------------------------------------------------------- status
            clock_tick += 1;
            if clock_tick % 120 == 0 {
                sceKernelPowerTick(0);
            }
            if clock_tick % 60 == 0 && scePowerGetArmClockFrequency() < CLOCKS[0] - 20 {
                set_clocks();
            }
            let tj = Instant::now();
            if let Some(w) = world_thread.join() {
                game.push_player(&w);
            }
            joined_ms = joined_ms * 0.9 + tj.elapsed().as_secs_f32() * 100.0;
            if frame_no % 10 == 0 {
                let r = game.readout();
                let (filled, slots) = city.near.filled();
                shell.dev.engine = json!({
                    "stage": "running",
                    "mode": session.mode.name(),
                    "interface": {"up": shell.ui.live(), "open": channel().is_open(), "error": shell.ui.error, "turns": shell.ui.turns, "turnMs": shell.ui.turn_ms, "worstTurnMs": shell.ui.worst_ms, "collections": shell.ui.collections, "collectMs": shell.ui.collect_ms},
                    "pack": {"path": pack_path, "sha256": pack_id["sha256"], "bytes": pack_id["bytes"], "profile": meta["profile"]},
                    "loadMs": load_ms, "copyMs": copy_ms,
                    "frameMs": timing.avg(), "worstMs": timing.worst(), "late": timing.late, "lateBy": {"cpu": late_cpu, "gpu": late_gpu}, "frames": timing.frames, "pace": set.pace,
                    "cpuMs": {"draw": draw_ms, "sim": sim_ms, "interface": ui_ms, "interfaceDraw": ui_draw_ms, "joinWait": joined_ms, "horizon": horizon_ms, "place": cpu_part[0], "sunPass": cpu_part[1], "city": cpu_part[2], "models": cpu_part[3]},
                    "gpuMs": if set.profile { json!(gpu_ms) } else { Value::Null },
                    "city": {"draws": stats.draws, "tris": {"wall": stats.tris[0], "solid": stats.tris[1], "instanced": stats.tris[2], "sun": sun_tris}, "places": stats.places},
                    "trees": {"finer": fine_trees, "cut": city.cut_reach()},
                    "near": {"slots": [filled, slots], "reads": city.near.slots.reads, "mb": city.near.slots.bytes as f32 / 1e6, "starved": city.near.slots.starved},
                    "camera": {"pos": [eye.x, eye.y, eye.z], "target": [target.x, target.y, target.z], "fov": fov},
                    "game": {"player": r.mode.name(), "hour": game.hour.index(), "camera": game.camera, "mph": r.mph, "at": r.at, "label": game.label()},
                    "settings": {"near": set.show.reach.near, "mid": set.show.reach.mid, "detail": set.show.reach.detail, "profile": set.profile, "view": set.view.is_some()},
                    "governor": {"on": set.govern, "budget": set.budget, "scale": scale, "held": hold},
                    "programs": {"compiled": gpu.compiled, "cached": gpu.cached},
                    "shadows": {"sweeps": shadows.sweeps, "sweepMs": shadows.sweep_ms()},
                    "msaa": samples,
                    "memory": {"geometry": city.geometry_bytes, "textures": city.texture_bytes, "vram": vram.reserved()},
                    "clockMhz": [scePowerGetArmClockFrequency(), scePowerGetGpuClockFrequency()],
                });
            }
            shell.dev.publish(frame_no, "newyork");
            serve(&mut shell.dev, frame_no, action);
            frame_no = frame_no.wrapping_add(1);
        }
    }
}

/// Answers wired-debug requests at a frame boundary.
unsafe fn serve(dev: &mut dev::Host, frame: u32, action: Action) {
    let mut request = dev.poll();
    let op = request.as_ref().map(|r| r.command.op).or(match action {
        Action::Capture => Some(Op::Capture),
        _ => None,
    });
    match op {
        Some(Op::Status) => request.take().unwrap().finish(Ok(dev.status(frame, "newyork"))),
        Some(Op::Menu) => {
            dev.menu.visible = !dev.menu.visible;
            request.take().unwrap().finish(Ok(json!({"menu": dev.menu.visible})));
        }
        Some(Op::Capture) => {
            if let Some(request) = request.take() {
                let _ = request.reply.try_send(dev.capture(frame));
            } else {
                dev.capture_from_menu(frame);
            }
        }
        Some(Op::Native) => {
            let request = request.take().unwrap();
            g::vita2d_wait_rendering_done();
            let result = dev::exec_native(request.native_path.as_ref().unwrap());
            request.finish(result.map(|_| json!({})));
        }
        Some(Op::Push | Op::Reload | Op::Reset | Op::Stage) => {
            if let Some(request) = request.take() {
                request.finish(Err("Pocket New York's interface is read from the share when the app starts; use native to start it again".into()));
            }
        }
        None => {}
    }
}
