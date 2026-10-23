//! What moves reads from the pack: `sim.json` (`web/pocket/sim.js`) as the tables of `ny_pack::sim`. Nothing is
//! derived here: the records are the reference's, in its order.

use ny_pack::sim::*;
use ny_pack::{self as pack, slice_bytes};
use serde_json::Value;
use std::path::Path;

fn f(v: &Value) -> f32 {
    v.as_f64().unwrap_or(f64::NAN) as f32
}

fn doubles(v: &Value) -> Vec<f64> {
    v.as_array().map(|a| a.iter().map(|x| x.as_f64().unwrap_or(f64::NAN)).collect()).unwrap_or_default()
}

fn floats(v: &Value) -> Vec<f32> {
    v.as_array().map(|a| a.iter().map(f).collect()).unwrap_or_default()
}

/// One name per line of `NAME`.
#[derive(Default)]
struct Names {
    lines: Vec<String>,
    index: std::collections::HashMap<String, u32>,
}

impl Names {
    fn id(&mut self, s: &str) -> u32 {
        let s = s.replace('\n', " ");
        if let Some(&i) = self.index.get(&s) {
            return i;
        }
        let i = self.lines.len() as u32;
        self.index.insert(s.clone(), i);
        self.lines.push(s);
        i
    }
}

/// Adds the simulation's sections from `<ir>/sim.json`, when the export wrote one; returns whether it did.
pub fn add(w: &mut pack::Writer, ir: &Path) -> Result<bool, String> {
    let Ok(text) = std::fs::read_to_string(ir.join("sim.json")) else { return Ok(false) };
    let s: Value = serde_json::from_str(&text).map_err(|e| format!("sim.json: {e}"))?;
    let mut names = Names::default();
    let mut points: Vec<[f64; 2]> = Vec::new();

    let rect = |v: &Value| [f(&v["minX"]), f(&v["maxX"]), f(&v["minZ"]), f(&v["maxZ"])];
    let shore = &s["shore"];
    let (west, east) = (floats(&shore["west"]), floats(&shore["east"]));
    let timing = &s["signal"];
    let specials: Vec<String> = s["specials"].as_array().ok_or("sim.json: specials")?.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect();
    let mut world = SimWorld {
        island: rect(&s["island"]),
        sidewalk_height: f(&s["island"]["sidewalkHeight"]),
        water_level: f(&s["island"]["waterLevel"]),
        central_park: rect(&s["centralPark"]),
        lawn: if s["bryantLawn"].is_object() { rect(&s["bryantLawn"]) } else { [0.0; 4] },
        lawn_top: if s["bryantLawn"].is_object() { f(&s["bryantLawn"]["top"]) } else { f32::NAN },
        cut_z: f(&s["cutZ"]),
        shore_z0: f(&shore["z0"]),
        shore_step: f(&shore["step"]),
        shore_n: west.len() as u32,
        signal: [f(&timing["cycle"]), f(&timing["avenueAmber"]), f(&timing["avenueRed"]), f(&timing["streetAmber"]), f(&timing["walk"])],
        specials: [0; 4],
    };
    for (k, name) in specials.iter().enumerate().take(4) {
        world.specials[k] = names.id(name);
    }
    let shore_pairs: Vec<[f32; 2]> = west.iter().zip(&east).map(|(&a, &b)| [a, b]).collect();

    let c = &s["colliders"];
    let (box_, obb) = (floats(&c["box"]), c["obb"].as_array().cloned().unwrap_or_default());
    let colliders: Vec<Collider> = (0..obb.len())
        .map(|i| {
            let b = &box_[i * 11..i * 11 + 11];
            Collider { min_x: b[0], max_x: b[1], min_z: b[2], max_z: b[3], max_y: b[4], obb: obb[i].as_u64().unwrap_or(0) as u32, cx: b[5], cz: b[6], hx: b[7], hz: b[8], cs: b[9], sn: b[10] }
        })
        .collect();

    let mut blocks = Vec::new();
    for b in s["blocks"].as_array().ok_or("sim.json: blocks")? {
        let poly = doubles(&b["poly"]);
        let first = points.len() as u32;
        points.extend(poly.chunks(2).map(|p| [p[0], p[1]]));
        let mut flags = 0;
        if b["rect"].as_bool() == Some(true) {
            flags |= block::RECT;
        }
        if b["median"].as_bool() == Some(true) {
            flags |= block::MEDIAN;
        }
        blocks.push(SimBlock { min_x: f(&b["minX"]), max_x: f(&b["maxX"]), min_z: f(&b["minZ"]), max_z: f(&b["maxZ"]), first, count: (poly.len() / 2) as u32, flags, park: f(&b["park"]) });
    }
    let mut roads = Vec::new();
    for r in s["roads"].as_array().ok_or("sim.json: roads")? {
        let pts = doubles(&r["pts"]);
        let first = points.len() as u32;
        points.extend(pts.chunks(2).map(|p| [p[0], p[1]]));
        roads.push(Road { first, count: (pts.len() / 2) as u32, name: names.id(r["name"].as_str().unwrap_or("")), pad: 0 });
    }
    let streets: Vec<Street> = s["streets"].as_array().ok_or("sim.json: streets")?.iter().map(|t| Street { z: f(&t["z"]), number: t["number"].as_u64().unwrap_or(0) as u32, name: names.id(t["short"].as_str().unwrap_or("")), pad: 0 }).collect();
    let landmarks: Vec<Mark> = s["landmarks"].as_array().ok_or("sim.json: landmarks")?.iter().map(|l| Mark { x: f(&l["x"]), z: f(&l["z"]), heading: 0.0, name: names.id(l["name"].as_str().unwrap_or("")) }).collect();
    let teleports: Vec<Mark> = s["teleports"].as_array().ok_or("sim.json: teleports")?.iter().map(|t| Mark { x: f(&t["x"]), z: f(&t["z"]), heading: f(&t["heading"]), name: names.id(t["id"].as_str().unwrap_or("")) }).collect();
    let cameras: Vec<CameraPreset> = s["cameras"].as_array().ok_or("sim.json: cameras")?.iter().map(|c| CameraPreset { fov: f(&c["fov"]), scenic: c["scenic"].as_bool().unwrap_or(false) as u32, id: names.id(c["id"].as_str().unwrap_or("")), pad: 0 }).collect();
    let mut ents = Vec::new();
    for mode in ["person", "car", "cyclist", "helicopter"] {
        let e = &s["entities"][mode];
        let p = floats(&e["position"]);
        ents.push(EntityStart { pos: [p[0], p[1], p[2]], heading: f(&e["heading"]), chase_distance: f(&e["chaseDistance"]), chase_height: f(&e["chaseHeight"]), target_y: f(&e["targetY"]), pad: 0.0 });
    }

    let net = &s["network"];
    let nodes: Vec<TrafficNode> = net["nodes"].as_array().ok_or("sim.json: nodes")?.iter().map(|n| TrafficNode { x: f(&n["x"]), z: f(&n["z"]), hx: f(&n["hx"]), hz: f(&n["hz"]), signal: n["signal"].as_bool().unwrap_or(false) as u32, groups: n["groups"].as_u64().unwrap_or(2) as u32 }).collect();
    let (mut edges, mut lanes, mut moves, mut outs) = (Vec::new(), Vec::<f32>::new(), Vec::new(), Vec::<i8>::new());
    for e in net["edges"].as_array().ok_or("sim.json: edges")? {
        let l = floats(&e["lanes"]);
        let lane_first = lanes.len() as u32;
        lanes.extend(&l);
        let move_first = moves.len() as u32;
        let ms = e["moves"].as_array().cloned().unwrap_or_default();
        for m in &ms {
            let out_first = outs.len() as u32;
            for o in m["out"].as_array().cloned().unwrap_or_default() {
                outs.push(o.as_i64().unwrap_or(-1).clamp(-1, 127) as i8);
            }
            let turn = match m["turn"].as_str() {
                Some("L") => turn::LEFT,
                Some("R") => turn::RIGHT,
                _ => turn::STRAIGHT,
            };
            moves.push(TrafficMove { to: m["to"].as_u64().unwrap_or(0) as u32, turn, out_first, pad: 0 });
        }
        edges.push(TrafficEdge {
            from: e["from"].as_u64().unwrap_or(0) as u32,
            to: e["to"].as_u64().unwrap_or(0) as u32,
            sx: f(&e["sx"]),
            sz: f(&e["sz"]),
            dx: f(&e["dx"]),
            dz: f(&e["dz"]),
            rx: f(&e["rx"]),
            rz: f(&e["rz"]),
            len: f(&e["len"]),
            lane_first,
            lane_count: l.len() as u32,
            move_first,
            move_count: ms.len() as u32,
            flags: if e["ave"].as_bool() == Some(true) { edge::AVE } else { 0 },
            group: e["group"].as_u64().unwrap_or(0) as u32,
            opposite: e["opposite"].as_i64().unwrap_or(-1) as i32,
        });
    }

    // Where pedestrians may walk, and the highway's lanes (optional: an older export has neither).
    let mut walk_edges: Vec<WalkEdge> = Vec::new();
    let mut walk_nodes: Vec<WalkNode> = Vec::new();
    if let Some(wk) = s.get("walks") {
        let bits = |v: &Value| v.as_f64().unwrap_or(0.0) as u64;
        for e in wk["edges"].as_array().cloned().unwrap_or_default() {
            walk_edges.push(WalkEdge {
                half: f(&e["half"]),
                clx: f(&e["clx"]),
                clz: f(&e["clz"]),
                in_side: e["inSide"].as_u64().unwrap_or(255).min(255) as u8,
                out_side: e["outSide"].as_u64().unwrap_or(255).min(255) as u8,
                general: e["general"].as_bool().unwrap_or(false) as u8,
                pad: 0,
                walk: [bits(&e["walk"][0]), bits(&e["walk"][1])],
            });
        }
        for n in wk["nodes"].as_array().cloned().unwrap_or_default() {
            let mut w = WalkNode { road: [-1; 4], ..Default::default() };
            for (k, side) in n.as_array().cloned().unwrap_or_default().iter().enumerate().take(4) {
                if side.is_null() {
                    continue;
                }
                w.road[k] = side["road"].as_i64().unwrap_or(-1) as i32;
                w.across[k] = f(&side["across"]);
                w.half[k] = f(&side["half"]);
                w.lines[k] = side["lines"].as_u64().unwrap_or(0) as u32;
            }
            walk_nodes.push(w);
        }
        if walk_edges.len() != edges.len() || walk_nodes.len() != nodes.len() {
            return Err(format!("sim.json: walks for {} edges and {} nodes, the network has {} and {}", walk_edges.len(), walk_nodes.len(), edges.len(), nodes.len()));
        }
    }
    let mut hwy_lanes: Vec<HighwayLane> = Vec::new();
    let mut hwy_pts: Vec<[f32; 3]> = Vec::new();
    for l in s["highway"].as_array().cloned().unwrap_or_default() {
        let pts = floats(&l["pts"]);
        let first = hwy_pts.len() as u32;
        hwy_pts.extend(pts.chunks_exact(3).map(|p| [p[0], p[1], p[2]]));
        hwy_lanes.push(HighwayLane { first, count: hwy_pts.len() as u32 - first, speed: f(&l["speed"]), pad: 0 });
    }

    let mut sections: Vec<(u32, Vec<u8>)> = vec![
        (SIMW, pack::bytes_of(&world).to_vec()),
        (COLL, slice_bytes(&colliders).to_vec()),
        (SHOR, slice_bytes(&shore_pairs).to_vec()),
        (SBLK, slice_bytes(&blocks).to_vec()),
        (SPTS, slice_bytes(&points).to_vec()),
        (ROAD, slice_bytes(&roads).to_vec()),
        (STRT, slice_bytes(&streets).to_vec()),
        (LMRK, slice_bytes(&landmarks).to_vec()),
        (TELE, slice_bytes(&teleports).to_vec()),
        (NAME, names.lines.join("\n").into_bytes()),
        (NODE, slice_bytes(&nodes).to_vec()),
        (EDGE, slice_bytes(&edges).to_vec()),
        (LANE, slice_bytes(&lanes).to_vec()),
        (MOVE, slice_bytes(&moves).to_vec()),
        (MOUT, slice_bytes(&outs).to_vec()),
        (CAMS, slice_bytes(&cameras).to_vec()),
        (ENTS, slice_bytes(&ents).to_vec()),
    ];
    if !walk_edges.is_empty() {
        sections.push((WALK, slice_bytes(&walk_edges).to_vec()));
        sections.push((WNOD, slice_bytes(&walk_nodes).to_vec()));
    }
    if !hwy_lanes.is_empty() {
        sections.push((HWYL, slice_bytes(&hwy_lanes).to_vec()));
        sections.push((HWYP, slice_bytes(&hwy_pts).to_vec()));
    }
    // The same sections alone, beside the IR: what `cargo test -p ny-sim` reads, without a city pack.
    let mut alone = pack::Writer::default();
    for (t, bytes) in &sections {
        alone.add(*t, bytes.clone());
        w.add(*t, bytes.clone());
    }
    let _ = std::fs::write(ir.join("../sim.pack"), alone.finish());
    eprintln!(
        "sim: {} colliders, {} blocks, {} roads, {} names, {} nodes, {} edges, {} moves, {} highway lanes of {} points",
        colliders.len(),
        blocks.len(),
        roads.len(),
        names.lines.len(),
        nodes.len(),
        edges.len(),
        moves.len(),
        hwy_lanes.len(),
        hwy_pts.len()
    );
    Ok(true)
}
