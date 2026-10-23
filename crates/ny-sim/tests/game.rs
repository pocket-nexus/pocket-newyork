//! The game over the compiler's world: modes, teleports, the readout and the traffic, run for a minute.
//!
//!   bun tools/newyork.ts export --only sim && cargo run --release -p ny-cook && cargo test -p ny-sim

use ny_sim::traffic::Kind;
use ny_sim::{Game, Hour, Input, Mode, World};
use std::path::PathBuf;

fn world() -> Option<World> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.pocket-build/scene/sim.pack");
    let bytes = std::fs::read(path).ok()?;
    let index = ny_pack::Index::parse(&bytes).ok()?;
    World::from_sections(|t| index.range(t).ok().map(|(at, n)| bytes[at..at + n].to_vec())).ok()
}

macro_rules! need_world {
    () => {
        match world() {
            Some(w) => w,
            None => {
                eprintln!("skipped: no .pocket-build/scene/sim.pack");
                return;
            }
        }
    };
}

#[test]
fn the_stand_in_game_moves_without_a_pack() {
    let mut g = Game::new();
    let start = g.readout().at;
    for _ in 0..120 {
        g.step(&Input { ahead: 1.0, ..Default::default() }, 1.0 / 60.0);
    }
    let r = g.readout();
    assert!(r.mph > 10.0, "the taxi drives: {} mph", r.mph);
    assert!((r.at[1] - start[1]).abs() > 10.0);
    assert_eq!(g.label(), "");
}

#[test]
fn it_starts_where_the_reference_starts() {
    let w = need_world!();
    let g = Game::with_world(w);
    let r = g.readout();
    assert_eq!(r.mode, Mode::Drive);
    assert_eq!(r.hour, Hour::Golden);
    assert_eq!(g.label(), "5th Ave & E 37th St");
    // Facing south, down the avenue towards the Empire State Building
    assert!((r.heading - core::f32::consts::PI).abs() < 1e-3);
    let v = g.view();
    assert!(v.eye.z < 410.6 && v.look.z > v.eye.z, "the chase camera is behind the taxi, looking ahead");
}

#[test]
fn every_mode_teleports_to_every_place() {
    let w = need_world!();
    let n = w.teleports.len();
    let mut g = Game::with_world(w);
    for mode in Mode::ALL {
        g.set_mode(mode);
        for k in 0..n {
            assert!(g.teleport_to(k));
            let r = g.readout();
            let t = g.world.teleports[k];
            assert!((r.at[0] - t.x).hypot(r.at[1] - t.z) < 64.0, "{mode:?} to {}: {:?}", g.world.name(t.name as u16), r.at);
            for _ in 0..30 {
                g.step(&Input { ahead: 0.6, turn: 0.2, ..Default::default() }, 1.0 / 60.0);
            }
            assert!(!g.label().is_empty());
        }
    }
}

#[test]
fn scenic_cameras_hand_back_to_the_chase_camera() {
    let w = need_world!();
    let mut g = Game::with_world(w);
    let observatory = g.world.camera_named("observatory").unwrap() as u8;
    g.set_camera(observatory);
    for _ in 0..60 {
        g.step(&Input::default(), 1.0 / 60.0);
    }
    assert!(g.view().eye.y > 300.0, "on the 86th floor deck");
    assert!(g.label().starts_with("Empire State Building"), "{}", g.label());
    g.set_mode(Mode::Fly);
    assert_eq!(g.camera, 0);
}

#[test]
fn traffic_keeps_its_lanes_and_its_signals() {
    let w = need_world!();
    let mut g = Game::with_world(w);
    let t0 = std::time::Instant::now();
    let mut worst_overlap = 0.0f32;
    let frames = 60 * 60;
    for f in 0..frames {
        g.step(&Input { ahead: if f % 600 < 300 { 0.5 } else { 0.0 }, ..Default::default() }, 1.0 / 60.0);
        if f % 30 == 0 {
            let v = g.vehicles();
            for a in 0..v.len() {
                for b in a + 1..v.len() {
                    if !v[a].placed() || !v[b].placed() {
                        continue;
                    }
                    // Two boxes on the ground, along a's heading: overlapping when both their lengths and their
                    // widths overlap (the reference keeps 2 m and more between cars in a lane)
                    let (fx, fz) = (v[a].yaw.sin(), v[a].yaw.cos());
                    let (dx, dz) = (v[b].x - v[a].x, v[b].z - v[a].z);
                    let along = (dx * fx + dz * fz).abs();
                    let across = (dz * fx - dx * fz).abs();
                    let parallel = (v[a].yaw - v[b].yaw).cos().abs() > 0.95;
                    if parallel && across < v[a].half_width + v[b].half_width {
                        let o = v[a].half + v[b].half - along;
                        if o > worst_overlap {
                            eprintln!("frame {f}: {a} {:?} speed {:.1} and {b} {:?} speed {:.1}, {along:.2} m along, {across:.2} across", v[a].on(), v[a].speed, v[b].on(), v[b].speed);
                        }
                        worst_overlap = worst_overlap.max(o);
                    }
                }
            }
        }
    }
    let ms = t0.elapsed().as_secs_f32() * 1000.0 / frames as f32;
    let v = g.vehicles();
    let placed = v.iter().filter(|c| c.placed()).count();
    let moving = v.iter().filter(|c| c.placed() && c.speed > 1.0).count();
    let buses = v.iter().filter(|c| c.kind == Kind::Bus && c.placed()).count();
    eprintln!("traffic: {placed} placed ({buses} buses), {moving} moving after a minute; {ms:.3} ms a frame on this machine; worst overlap {worst_overlap:.2} m");
    assert!(placed > 150, "{placed}");
    assert!(moving > 20, "{moving}");
    assert!(worst_overlap < 0.5);
}

#[test]
fn cars_wait_at_red_and_go_on_green() {
    let w = need_world!();
    let mut g = Game::with_world(w);
    // waiting at a signal's stop line: (on red, on green), counted over the first two cycles
    let (mut red, mut green) = (0usize, 0usize);
    let mut crossed_red = 0usize;
    let mut seen = 0usize;
    for f in 0..60 * 120 {
        g.step(&Input::default(), 1.0 / 60.0);
        if f % 15 != 0 {
            continue;
        }
        let t = g.clock;
        for v in g.vehicles() {
            let (e, _, s, conn, _, _) = v.on();
            if e < 0 {
                continue;
            }
            let edge = g.world.edges[e as usize];
            if g.world.nodes[edge.to as usize].signal == 0 {
                continue;
            }
            let phase = ny_sim::signal::Timing(g.world.info.signal).phase(t, edge.group == 0);
            let at_line = conn.is_none() && edge.len - s - v.half < 3.0;
            if at_line && v.speed < 0.3 {
                seen += 1;
                match phase {
                    ny_sim::signal::Phase::Red => red += 1,
                    ny_sim::signal::Phase::Green => green += 1,
                    _ => {}
                }
            }
            // A car at the very start of a crossing while its road has had red for over a second
            if let Some(cs) = conn {
                let late = ny_sim::signal::Timing(g.world.info.signal).phase(t - 1.5, edge.group == 0) == ny_sim::signal::Phase::Red;
                if phase == ny_sim::signal::Phase::Red && late && cs < 0.5 && cs > -v.half {
                    crossed_red += 1;
                }
            }
        }
    }
    eprintln!("signals: {seen} waits at a line seen, {red} on red, {green} on green; {crossed_red} entries on a red");
    assert!(red > 50 && red > green * 4, "cars wait at red: {red} on red, {green} on green");
    assert_eq!(crossed_red, 0);
}
