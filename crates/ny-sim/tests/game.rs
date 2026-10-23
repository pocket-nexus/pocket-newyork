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

/// The sidewalks' crowd around a signalized intersection for two minutes: everyone placed, on their tracks; people
/// step off a curb only on their WALK, and no vehicle drives over someone on a crosswalk.
#[test]
fn pedestrians_cross_on_the_walk_and_vehicles_give_way() {
    use ny_sim::signal::Timing;
    let w = need_world!();
    if w.walk_edges.is_empty() {
        eprintln!("skipped: the pack has no walk tables");
        return;
    }
    let timing = Timing(w.info.signal);
    // The signalized intersection nearest the reference's start
    let n = *w.nodes.iter().filter(|n| n.signal != 0).min_by(|a, b| ((a.x - 3.6).hypot(a.z - 410.6)).total_cmp(&(b.x - 3.6).hypot(b.z - 410.6))).unwrap();
    let mut g = Game::with_world(w);
    g.set_mode(Mode::Walk);
    g.teleport(n.x + 25.0, n.z + 25.0, 0.0);
    let dt = 1.0 / 60.0;
    let kerb = 1.6;
    let mut was: Vec<Option<(i32, f32, f32)>> = vec![None; g.pedestrians().len()];
    let (mut stepped_off, mut off_walk, mut over, mut on_crosswalks) = (0, 0, 0, 0usize);
    let t0 = std::time::Instant::now();
    let frames = 120 * 60;
    for f in 0..frames {
        g.step(&Input::default(), dt);
        let clock = g.clock;
        for (i, p) in g.pedestrians().iter().enumerate() {
            if !p.placed {
                was[i] = None;
                continue;
            }
            let x = p.track.ax + p.track.ux * p.t;
            let z = p.track.az + p.track.uz * p.t;
            assert!((x - p.x).abs() < 1e-3 && (z - p.z).abs() < 1e-3 && p.t >= -1e-3 && p.t <= p.track.len + 1e-3, "person {i} off its track");
            if p.track.node < 0 {
                was[i] = None;
                continue;
            }
            let progress = if p.dir > 0.0 { p.t } else { p.track.len - p.t };
            let mid = p.t > kerb && p.t < p.track.len - kerb;
            if let Some((node, before, dir)) = was[i] {
                if node == p.track.node && dir == p.dir && before <= kerb && progress > kerb && mid {
                    stepped_off += 1;
                    let avenue = p.track.side < 2;
                    if !timing.walk(clock - dt, avenue) && !timing.walk(clock, avenue) {
                        off_walk += 1;
                    }
                }
            }
            was[i] = Some((p.track.node, progress, p.dir));
            if mid {
                on_crosswalks += 1;
                for v in g.vehicles() {
                    if !v.placed() {
                        continue;
                    }
                    let (fx, fz) = (v.yaw.sin(), v.yaw.cos());
                    let (dx, dz) = (x - v.x, z - v.z);
                    let along = (dx * fx + dz * fz).abs();
                    let across = (dz * fx - dx * fz).abs();
                    if along < v.half + 0.3 && across < v.half_width + 0.3 {
                        over += 1;
                        if over < 5 {
                            eprintln!("frame {f}: person {i} under a vehicle at {x:.1}, {z:.1} (speed {:.1})", v.speed);
                        }
                    }
                }
            }
        }
    }
    let ms = t0.elapsed().as_secs_f32() * 1000.0 / frames as f32;
    let placed = g.pedestrians().iter().filter(|p| p.placed).count();
    let crossers = g.pedestrians().iter().filter(|p| p.placed && p.track.node >= 0).count();
    let eye = g.view().eye;
    let within = |r: f32| g.pedestrians().iter().filter(|p| p.placed && (p.x - eye.x).hypot(p.z - eye.z) < r).count();
    eprintln!("crowd: within 25 m of the eye {}, 100 m {}, 320 m {}", within(25.0), within(100.0), within(320.0));
    eprintln!("crowd: {placed} placed, {crossers} on crosswalks; {stepped_off} stepped off a curb ({off_walk} off the WALK), {on_crosswalks} person-frames on crosswalks, {over} under a vehicle; {ms:.3} ms a frame on this machine");
    assert_eq!(placed, g.pedestrians().len());
    assert!(crossers > 0 && stepped_off > 0, "nobody crossed");
    assert_eq!(off_walk, 0, "stepped off a curb without the WALK");
    assert_eq!(over, 0, "a vehicle drove over someone on a crosswalk");
}

/// The highway's cars run on, and a camera over the West Side Highway draws some.
#[test]
fn the_highway_runs_and_is_drawn() {
    let w = need_world!();
    if w.highway.is_empty() {
        eprintln!("skipped: the pack has no highway lanes");
        return;
    }
    let mut g = Game::with_world(w);
    let before: f64 = g.highway.cars.iter().map(|c| c.u).sum();
    for _ in 0..600 {
        g.step(&Input::default(), 1.0 / 60.0);
    }
    let moving = g.highway.cars.iter().filter(|c| c.v > 1.0).count();
    let after: f64 = g.highway.cars.iter().map(|c| c.u).sum();
    let mut drawn = Vec::new();
    let lane = &g.highway.lanes[0];
    let p = g.world.highway_pts[lane.first + lane.count / 2];
    g.highway.drawn(&g.world, ny_sim::math::V3 { x: p[0] + 60.0, y: 40.0, z: p[2] }, ny_sim::math::V3 { x: -1.0, y: -0.3, z: 0.0 }, &mut drawn);
    let near = drawn.iter().filter(|c| (c.x - p[0] - 60.0).hypot(c.z - p[2]) < 700.0).count();
    eprintln!("highway: {} cars, {moving} moving after 10 s, {} drawn from over its middle, {near} of them within 700 m", g.highway.cars.len(), drawn.len());
    assert!(moving > g.highway.cars.len() / 2);
    assert!(after != before);
    assert!(!drawn.is_empty());
}
