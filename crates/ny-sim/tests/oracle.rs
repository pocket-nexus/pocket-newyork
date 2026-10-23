//! The port against the reference: `web/pocket/sim.js` drove the reference's own controllers, cameras, location
//! label and signals with fixed scripts and recorded what they did (`.pocket-build/scene/ir/oracle/`); these tests
//! replay the scripts through `ny-sim` over the world the compiler wrote (`.pocket-build/scene/sim.pack`).
//!
//!   bun tools/newyork.ts export --only sim && cargo run --release -p ny-cook && cargo test -p ny-sim
//!
//! Without those files the tests say so and pass.

use ny_sim::camera::{Cameras, PRESETS};
use ny_sim::player::{Keys, Players};
use ny_sim::signal::{Phase, Timing};
use ny_sim::{Mode, World};
use serde_json::Value;
use std::path::PathBuf;

fn build() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.pocket-build/scene")
}

fn world() -> Option<World> {
    let bytes = std::fs::read(build().join("sim.pack")).ok()?;
    let index = ny_pack::Index::parse(&bytes).ok()?;
    World::from_sections(|t| index.range(t).ok().map(|(at, n)| bytes[at..at + n].to_vec())).ok()
}

fn oracle(name: &str) -> Option<Value> {
    let text = std::fs::read_to_string(build().join("ir/oracle").join(name)).ok()?;
    serde_json::from_str(&text).ok()
}

macro_rules! need {
    ($e:expr, $what:expr) => {
        match $e {
            Some(v) => v,
            None => {
                eprintln!("skipped: no {} (run the export and the compiler, see the file's head)", $what);
                return;
            }
        }
    };
}

fn mode_of(name: &str) -> Mode {
    match name {
        "person" => Mode::Walk,
        "car" => Mode::Drive,
        "cyclist" => Mode::Cycle,
        _ => Mode::Fly,
    }
}

fn f(v: &Value) -> f32 {
    v.as_f64().unwrap_or(0.0) as f32
}

/// Sets the entity of `mode` to the state a run started from.
fn start(p: &mut Players, mode: Mode, s: &Value) {
    let num = |k: &str| s.get(k).and_then(Value::as_f64).map(|x| x as f32);
    {
        let b = p.body_mut(mode);
        let pos = &s["position"];
        b.pos.x = f(&pos[0]);
        b.pos.y = f(&pos[1]);
        b.pos.z = f(&pos[2]);
        b.heading = num("heading").unwrap_or(0.0);
        b.speed = num("speed").unwrap_or(0.0);
    }
    match mode {
        Mode::Walk => p.person.vertical_vel = num("verticalVel").unwrap_or(0.0),
        Mode::Drive => {
            let c = &mut p.car;
            c.vx = num("vx").unwrap_or(0.0);
            c.vz = num("vz").unwrap_or(0.0);
            c.angular_vel = num("angularVel").unwrap_or(0.0);
            c.steer = num("steerAngle").unwrap_or(0.0);
            c.pitch_vel = num("pitchVel").unwrap_or(0.0);
            c.roll_vel = num("rollVel").unwrap_or(0.0);
            c.heave = num("heaveY").unwrap_or(0.0);
            c.heave_vel = num("heaveVel").unwrap_or(0.0);
            c.wheel_spin = num("wheelSpin").unwrap_or(0.0);
            if let Some(ch) = s["chassis"].as_array() {
                c.pitch = f(&ch[0]);
                c.roll = f(&ch[1]);
            }
        }
        Mode::Cycle => {
            p.cyclist.lean = num("leanAngle").unwrap_or(0.0);
            p.cyclist.pedal_phase = num("pedalPhase").unwrap_or(0.0);
        }
        Mode::Fly => {
            p.heli.vertical_speed = num("verticalSpeed").unwrap_or(0.0);
            p.heli.pitch = num("pitchAngle").unwrap_or(0.0);
            p.heli.roll = num("rollAngle").unwrap_or(0.0);
        }
    }
}

#[test]
fn controllers_and_follow_cameras_follow_the_reference() {
    let w = need!(world(), "sim.pack");
    let o = need!(oracle("controllers.json"), "oracle/controllers.json");
    let dt = f(&o["dt"]);
    let mut report = Vec::new();
    for run in o["runs"].as_array().unwrap() {
        let mode = mode_of(run["mode"].as_str().unwrap());
        let preset = PRESETS.iter().position(|p| *p == run["preset"].as_str().unwrap()).unwrap() as u8;
        let mut p = Players::new(&w);
        p.active = mode;
        start(&mut p, mode, &run["start"]);
        let mut cam = Cameras::default();
        cam.reset(&w);
        cam.set_preset(&w, preset, true);
        let (mut pos_err, mut head_err, mut speed_err, mut cam_err) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        let mut first_off = None;
        let mut elapsed = 0.0;
        let frames = run["frames"].as_array().unwrap();
        for (i, fr) in frames.iter().enumerate() {
            let keys: Vec<f32> = fr[0].as_str().unwrap().chars().map(|c| if c == '1' { 1.0 } else { 0.0 }).collect();
            p.keys = Keys { forward: keys[0], backward: keys[1], left: keys[2], right: keys[3], up: keys[4], down: keys[5], boost: keys[6] > 0.0 };
            elapsed += dt;
            p.update(&w, dt, elapsed);
            cam.update(&w, &p, dt, elapsed);
            let b = p.body(mode);
            let e = ((b.pos.x - f(&fr[1])).powi(2) + (b.pos.y - f(&fr[2])).powi(2) + (b.pos.z - f(&fr[3])).powi(2)).sqrt();
            let h = (b.heading - f(&fr[4])).abs();
            let s = (b.speed - f(&fr[5])).abs();
            let c = [cam.pos.x, cam.pos.y, cam.pos.z, cam.look.x, cam.look.y, cam.look.z].iter().zip(&fr.as_array().unwrap()[6..12]).map(|(a, r)| (a - f(r)).abs()).fold(0.0f32, f32::max);
            if first_off.is_none() && e > 0.005 {
                first_off = Some(i);
            }
            pos_err = pos_err.max(e);
            head_err = head_err.max(h);
            speed_err = speed_err.max(s);
            cam_err = cam_err.max(c);
        }
        report.push(format!("{:<10} {:<6} {} frames: position {:.4} m, heading {:.5} rad, speed {:.4} m/s, camera {:.4} m{}", run["mode"].as_str().unwrap(), run["preset"].as_str().unwrap(), frames.len(), pos_err, head_err, speed_err, cam_err, first_off.map(|i| format!(" (first off by 5 mm at frame {i})")).unwrap_or_default()));
        assert!(pos_err < 0.01 && head_err < 1e-4 && speed_err < 1e-3 && cam_err < 0.01, "{}", report.join("\n"));
    }
    eprintln!("{}", report.join("\n"));
}

#[test]
fn scenic_cameras_follow_the_reference() {
    let w = need!(world(), "sim.pack");
    let o = need!(oracle("scenic.json"), "oracle/scenic.json");
    let dt = f(&o["dt"]);
    let p = Players::new(&w);
    let mut worst = 0.0f32;
    for run in o["runs"].as_array().unwrap() {
        let preset = PRESETS.iter().position(|q| *q == run["id"].as_str().unwrap()).unwrap() as u8;
        let mut cam = Cameras::default();
        cam.reset(&w);
        cam.set_preset(&w, preset, true);
        let mut elapsed = f(&run["start"]);
        for fr in run["frames"].as_array().unwrap() {
            elapsed += dt;
            cam.update(&w, &p, dt, elapsed);
            let c = [cam.pos.x, cam.pos.y, cam.pos.z, cam.look.x, cam.look.y, cam.look.z].iter().zip(&fr.as_array().unwrap()[1..7]).map(|(a, r)| (a - f(r)).abs()).fold(0.0f32, f32::max);
            worst = worst.max(c);
        }
    }
    eprintln!("scenic cameras: worst {worst:.4} m");
    assert!(worst < 0.005);
}

#[test]
fn location_labels_match_the_reference() {
    let w = need!(world(), "sim.pack");
    let o = need!(oracle("labels.json"), "oracle/labels.json");
    let all = o.as_array().unwrap();
    let mut wrong = Vec::new();
    for s in all {
        let (x, z, label) = (f(&s[0]), f(&s[1]), s[2].as_str().unwrap());
        let ours = w.label(&w.location(x, z));
        if ours != label {
            wrong.push(format!("({x}, {z}): reference {label:?}, port {ours:?}"));
        }
    }
    eprintln!("labels: {} of {} differ", wrong.len(), all.len());
    for line in wrong.iter().take(12) {
        eprintln!("  {line}");
    }
    assert!(wrong.is_empty());
}

#[test]
fn signals_match_the_reference() {
    let w = need!(world(), "sim.pack");
    let o = need!(oracle("signals.json"), "oracle/signals.json");
    let t = Timing(w.info.signal);
    let name = |p: Phase| match p {
        Phase::Green => "green",
        Phase::Amber => "amber",
        Phase::Red => "red",
    };
    for s in o.as_array().unwrap() {
        let time = f(&s[0]);
        assert_eq!(name(t.phase(time, true)), s[1].as_str().unwrap(), "avenue at {time}");
        assert_eq!(name(t.phase(time, false)), s[2].as_str().unwrap(), "street at {time}");
        assert_eq!(t.walk(time, true), s[3].as_bool().unwrap(), "avenue walk at {time}");
        assert_eq!(t.walk(time, false), s[4].as_bool().unwrap(), "street walk at {time}");
    }
}

#[test]
fn teleports_arrive_where_the_reference_does() {
    let w = need!(world(), "sim.pack");
    let o = need!(oracle("teleports.json"), "oracle/teleports.json");
    let mut worst = 0.0f32;
    for t in o.as_array().unwrap() {
        let mode = mode_of(t["mode"].as_str().unwrap());
        let mut p = Players::new(&w);
        p.active = mode;
        for (name, m) in [("person", Mode::Walk), ("car", Mode::Drive), ("cyclist", Mode::Cycle), ("helicopter", Mode::Fly)] {
            let b = &t["before"][name];
            let body = p.body_mut(m);
            body.pos.x = f(&b[0]);
            body.pos.y = f(&b[1]);
            body.pos.z = f(&b[2]);
        }
        let (x, z) = p.teleport(&w, f(&t["x"]), f(&t["z"]), f(&t["heading"]));
        let to = &t["to"];
        let e = (x - f(&to[0])).abs().max((z - f(&to[2])).abs()).max((p.body(mode).pos.y - f(&to[1])).abs());
        worst = worst.max(e);
        assert!(e < 0.01, "{} to {}: port ({x}, {z}), reference {to}", t["mode"], t["id"]);
    }
    eprintln!("teleports: worst {worst:.4} m");
}

/// The highway's traffic against the reference's run (`web/pocket/sim.js` `highwayRun`): every car's place and
/// speed every tenth frame of 15 s from the seeded start, then the transforms of the cars a camera draws.
#[test]
fn the_highway_drives_as_the_reference() {
    use ny_sim::highway::Highway;
    let w = need!(world(), "sim.pack");
    let o = need!(oracle("highway.json"), "oracle/highway.json");
    if w.highway.is_empty() {
        eprintln!("skipped: the pack has no highway lanes");
        return;
    }
    let mut h = Highway::new(&w);
    let start = o["start"].as_array().unwrap();
    assert_eq!(h.cars.len(), start.len(), "the seeded fleet");
    let mut worst_start: f64 = 0.0;
    for (c, s) in h.cars.iter().zip(start) {
        assert_eq!(c.lane as i64, s[0].as_i64().unwrap());
        let kind = match c.kind {
            ny_sim::traffic::Kind::Taxi => 0,
            ny_sim::traffic::Kind::Sedan => 1,
            _ => 2,
        };
        assert_eq!(kind, s[1].as_i64().unwrap(), "car kind");
        for k in 0..3 {
            assert!((c.paint[k] as f64 - s[2 + k].as_f64().unwrap()).abs() < 1e-5, "paint");
        }
        worst_start = worst_start.max((c.u - s[5].as_f64().unwrap()).abs());
    }
    let dt = o["dt"].as_f64().unwrap() as f32;
    let (mut worst_u, mut worst_v): (f64, f64) = (0.0, 0.0);
    let mut frame = 0;
    for rec in o["frames"].as_array().unwrap() {
        let r = rec.as_array().unwrap();
        let f = r[0].as_i64().unwrap();
        while frame < f {
            h.step(&w, dt, None);
            frame += 1;
        }
        for (i, c) in h.cars.iter().enumerate() {
            let (u, v) = (r[1 + i * 2].as_f64().unwrap(), r[2 + i * 2].as_f64().unwrap());
            let len = h.lanes[c.lane as usize].len;
            let mut du = (c.u - u).abs();
            du = du.min((du - len).abs());
            worst_u = worst_u.max(du);
            worst_v = worst_v.max((c.v - v).abs());
        }
    }
    // The cars the camera draws: which, and their transforms
    let cam = &o["camera"];
    let eye = ny_sim::math::V3 { x: f(&cam["pos"][0]), y: f(&cam["pos"][1]), z: f(&cam["pos"][2]) };
    let ahead = ny_sim::math::V3 { x: f(&cam["fwd"][0]), y: f(&cam["fwd"][1]), z: f(&cam["fwd"][2]) };
    let mut drawn = Vec::new();
    h.drawn(&w, eye, ahead, &mut drawn);
    let want = o["drawn"].as_array().unwrap();
    assert_eq!(drawn.len(), want.len(), "cars drawn");
    let mut worst_m: f32 = 0.0;
    for (p, rec) in drawn.iter().zip(want) {
        let r = rec.as_array().unwrap();
        assert_eq!(p.car as i64, r[0].as_i64().unwrap(), "the same car in the same order");
        // three.js's matrix (column-major) of Euler(pitch, yaw, 0, 'YXZ') at the place
        let (sy, cy, sp, cp) = (p.yaw.sin(), p.yaw.cos(), p.pitch.sin(), p.pitch.cos());
        let m = [cy, 0.0, -sy, 0.0, sy * sp, cp, cy * sp, 0.0, sy * cp, -sp, cy * cp, 0.0, p.x, p.y, p.z, 1.0];
        for k in 0..16 {
            let d = (m[k] - f(&r[1 + k])).abs();
            worst_m = worst_m.max(if k >= 12 { d } else { d * 10.0 });
        }
    }
    eprintln!("highway: {} cars; start {worst_start:.2e} m, after 15 s place {worst_u:.2e} m, speed {worst_v:.2e} m/s; {} drawn, transforms within {worst_m:.2e}", h.cars.len(), drawn.len());
    assert!(worst_start < 1e-6, "start {worst_start}");
    assert!(worst_u < 0.01, "place {worst_u}");
    assert!(worst_v < 0.001, "speed {worst_v}");
    assert!(worst_m < 0.01, "transforms {worst_m}");
}

/// Pedestrians on the sidewalks against the reference's (`walkersRun`): from where each was first seen on a track,
/// its place along it, speed and way every frame it stayed there (the walk alone: turning round at the ends).
#[test]
fn pedestrians_walk_as_the_reference() {
    use ny_sim::crowd::{Crowd, Pedestrian, Track};
    let o = need!(oracle("walkers.json"), "oracle/walkers.json");
    let dt = o["dt"].as_f64().unwrap() as f32;
    let (mut worst_t, mut worst_v, mut turns, mut frames) = (0.0f32, 0.0f32, 0, 0);
    for run in o["runs"].as_array().unwrap() {
        let s = &run["start"];
        let mut p = Pedestrian {
            placed: true,
            track: Track { ax: f(&s["ax"]), az: f(&s["az"]), ux: f(&s["ux"]), uz: f(&s["uz"]), len: f(&s["len"]), node: -1, side: 0, line: 0.0 },
            t: f(&s["t"]),
            dir: f(&s["dir"]),
            v: f(&s["v"]),
            pause: f(&s["pause"]),
            speed: f(&s["speed"]),
            ..Default::default()
        };
        let ts = run["t"].as_array().unwrap();
        for rec in &ts[1..] {
            let r = rec.as_array().unwrap();
            let dir_was = p.dir;
            Crowd::advance(&mut p, dt, false, true, f32::INFINITY);
            if p.dir != dir_was {
                turns += 1;
            }
            frames += 1;
            assert_eq!(p.dir, f(&r[2]), "way at frame {frames} of person {}", run["ped"]);
            worst_t = worst_t.max((p.t - f(&r[0])).abs());
            worst_v = worst_v.max((p.v - f(&r[1])).abs());
        }
    }
    eprintln!("walkers: {} runs, {frames} frames, {turns} turns; place within {worst_t:.2e} m, speed {worst_v:.2e} m/s", o["runs"].as_array().unwrap().len());
    // (in floats against the reference's doubles: a step rounds by some 4 µm 100 m along a track, 1 200 steps by ~5 mm;
    // the reference draws its walks from Math.random, and an export's longest runs drift to ~14 mm)
    assert!(worst_t < 0.02, "place {worst_t}");
    assert!(worst_v < 0.002, "speed {worst_v}");
}
