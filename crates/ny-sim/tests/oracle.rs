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
