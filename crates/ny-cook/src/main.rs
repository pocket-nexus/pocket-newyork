//! The scene compiler: SceneIR in, a device pack out.
//!
//!   ny-cook --ir .pocket-build/scene/ir --profile profiles/vita60.json --out .pocket-build/scene/vita60/city.pack

mod bins;
mod geom;
mod ir;

use bins::{Bin, Bins, Vx, FAR, MID, NEAR};
use geom::{Aabb, M4};
use ir::{Ir, Object};
use ny_pack::{self as pack, City};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// What a device profile asks of the compiler.
#[derive(Deserialize, Clone, Debug)]
struct Profile {
    name: String,
    /// Side of a cell, metres; cells along a block, blocks along a region.
    cell: f32,
    block_cells: u32,
    region_blocks: u32,
    /// Share of a part's triangles each level keeps, and the error (relative to the part's size) it may make.
    keep: [f32; 3],
    error: [f32; 3],
    /// An instance shows at the mid and the far level when its bounding radius is at least this.
    mid_radius: f32,
    far_radius: f32,
}

/// How the compiler treats an object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    /// Not part of the static city: what moves (players, traffic, people), effects, the sky.
    Skip,
    /// Coloured by its probed vertices.
    Solid,
}

fn classify(ir: &Ir, o: &Object) -> Class {
    const MOVING: [&str; 8] = ["PlayerTaxiCar", "PlayerPerson", "PlayerCyclist", "PlayerHelicopter", "PeopleNear", "PeopleFar", "ManhattanTrafficAndPedestrians", "HighwayTraffic"];
    if MOVING.iter().any(|m| o.path.split('/').any(|p| p == *m)) {
        return Class::Skip;
    }
    if o.kind != "Mesh" && o.kind != "InstancedMesh" {
        return Class::Skip;
    }
    let m = &ir.materials[o.material(0)];
    if m.kind == "NodeMaterial" || m.kind.is_empty() {
        return Class::Skip; // the sky dome
    }
    match m.factory.as_deref() {
        Some("createTreeMaterial") => return Class::Skip,
        Some("createParkLampPoolMaterial" | "createContactShadowMaterial" | "createGodRayMaterial" | "createTunnelShadeMaterial" | "createSteamMaterial" | "createCarFlareMaterial") => return Class::Skip,
        _ => {}
    }
    if !o.shown {
        return Class::Skip;
    }
    if m.transparent && m.factory.as_deref() != Some("createRoadPaintMaterial") {
        return Class::Skip;
    }
    Class::Solid
}

/// A geometry's vertices in its own frame, as floats.
struct Source {
    pos: Vec<[f32; 3]>,
    nrm: Vec<[f32; 3]>,
    /// Triangles by material slot, then simplified for the mid and far level.
    lods: [Vec<(usize, Vec<u32>)>; 3],
    bounds: Aabb,
}

fn source(ir: &Ir, gid: usize, profile: &Profile) -> Source {
    let g = &ir.geometries[gid];
    let (p, ps) = ir.attribute(g, "position").expect("geometry without positions");
    let pos: Vec<[f32; 3]> = (0..g.vertex_count).map(|i| [p[i * ps], p[i * ps + 1], if ps > 2 { p[i * ps + 2] } else { 0.0 }]).collect();
    let tris = ir.triangles(g);
    let nrm: Vec<[f32; 3]> = match ir.attribute(g, "normal") {
        Some((n, 3)) => (0..g.vertex_count).map(|i| [n[i * 3], n[i * 3 + 1], n[i * 3 + 2]]).collect(),
        _ => {
            // Face normals, summed at each vertex.
            let mut acc = vec![[0.0f32; 3]; g.vertex_count];
            for (_, t) in &tris {
                for f in t.chunks(3) {
                    let (a, b, c) = (pos[f[0] as usize], pos[f[1] as usize], pos[f[2] as usize]);
                    let n = geom::cross(geom::sub(b, a), geom::sub(c, a));
                    for &i in f {
                        acc[i as usize] = geom::add(acc[i as usize], n);
                    }
                }
            }
            acc.into_iter().map(geom::normalize).collect()
        }
    };
    let mut bounds = Aabb::EMPTY;
    for &q in &pos {
        bounds.add(q);
    }
    let simplify = |t: &[u32], keep: f32, error: f32| -> Vec<u32> {
        if keep >= 1.0 || t.len() < 36 {
            return t.to_vec();
        }
        let bytes: &[u8] = unsafe { std::slice::from_raw_parts(pos.as_ptr() as *const u8, pos.len() * 12) };
        let adapter = meshopt::VertexDataAdapter::new(bytes, 12, 0).unwrap();
        let target = ((t.len() / 3) as f32 * keep) as usize * 3;
        meshopt::simplify(t, &adapter, target.max(3), error, meshopt::SimplifyOptions::None, None)
    };
    let lods = [0, 1, 2].map(|l| tris.iter().map(|(slot, t)| (*slot, simplify(t, profile.keep[l], profile.error[l]))).collect());
    Source { pos, nrm, lods, bounds }
}

fn instance_matrix(ir: &Ir, o: &Object, i: usize, cache: &mut HashMap<usize, Vec<f32>>) -> M4 {
    match &o.instances {
        None => o.matrix_world,
        Some(inst) => {
            let m = cache.entry(o.id).or_insert_with(|| ir.floats(&inst.matrix, false));
            let mut local = [0.0f32; 16];
            local.copy_from_slice(&m[i * 16..i * 16 + 16]);
            geom::mul(&o.matrix_world, &local)
        }
    }
}

fn glow_bytes(e: [f32; 3]) -> [u8; 4] {
    let top = e[0].max(e[1]).max(e[2]);
    if top < 1.0 / 512.0 {
        return [0; 4];
    }
    let m = (top / pack::GLOW_RANGE * 255.0).ceil().clamp(1.0, 255.0);
    let s = m / 255.0 * pack::GLOW_RANGE;
    [(e[0] / s * 255.0).round() as u8, (e[1] / s * 255.0).round() as u8, (e[2] / s * 255.0).round() as u8, m as u8]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let ir_dir = PathBuf::from(arg("--ir").unwrap_or(".pocket-build/scene/ir".into()));
    let profile_path = arg("--profile").unwrap_or("profiles/vita60.json".into());
    let out = PathBuf::from(arg("--out").unwrap_or(".pocket-build/scene/vita60/city.pack".into()));
    let profile: Profile = serde_json::from_str(&std::fs::read_to_string(&profile_path).expect("profile")).expect("profile");
    let t0 = std::time::Instant::now();
    let ir = Ir::load(&ir_dir).unwrap_or_else(|e| panic!("{e}"));
    eprintln!("ir: {} objects, {} geometries, {} materials ({:.1} s)", ir.objects.len(), ir.geometries.len(), ir.materials.len(), t0.elapsed().as_secs_f32());

    let classes: Vec<Class> = ir.objects.iter().map(|o| classify(&ir, o)).collect();
    let mut sources: HashMap<usize, Source> = HashMap::new();
    let mut mats: HashMap<usize, Vec<f32>> = HashMap::new();

    // The extent of what is drawn, from every instance's bounds.
    let mut world = Aabb::EMPTY;
    let mut placed: Vec<(usize, usize, M4, Aabb)> = Vec::new();
    for (o, class) in ir.objects.iter().zip(&classes) {
        if *class == Class::Skip {
            continue;
        }
        let src = sources.entry(o.geometry).or_insert_with(|| source(&ir, o.geometry, &profile));
        if src.pos.is_empty() {
            continue;
        }
        for i in 0..o.instance_count() {
            let m = instance_matrix(&ir, o, i, &mut mats);
            let mut b = Aabb::EMPTY;
            let (lo, hi) = (src.bounds.min, src.bounds.max);
            for k in 0..8 {
                b.add(geom::point(&m, [if k & 1 == 0 { lo[0] } else { hi[0] }, if k & 2 == 0 { lo[1] } else { hi[1] }, if k & 4 == 0 { lo[2] } else { hi[2] }]));
            }
            if !b.min.iter().chain(b.max.iter()).all(|v| v.is_finite()) || b.size().iter().any(|&s| s > 60_000.0) {
                continue;
            }
            world.union(&b);
            placed.push((o.id, i, m, b));
        }
    }
    eprintln!("placed {} instances; world {:?} .. {:?}", placed.len(), world.min, world.max);

    let cell = profile.cell;
    let x0 = (world.min[0] / cell).floor() * cell;
    let z0 = (world.min[2] / cell).floor() * cell;
    let group = (profile.block_cells * profile.region_blocks) as f32 * cell;
    let cells_x = (((world.max[0] - x0) / group).ceil() * group / cell) as u32;
    let cells_z = (((world.max[2] - z0) / group).ceil() * group / cell) as u32;
    let city = City {
        x0,
        z0,
        cell,
        cells_x,
        cells_z,
        block_cells: profile.block_cells,
        region_blocks: profile.region_blocks,
        y_min: world.min[1],
        y_max: world.max[1],
        // The reference's start: 5th Ave by 37th St, looking north.
        view: [3.6, 8.0, 380.0, 3.6, 8.0, 300.0],
        flags: 0,
    };
    let mut bins = Bins::new(city);

    // Probed colours are read an object at a time.
    let mut probe_of: Option<(usize, Option<ir::Probe>)> = None;
    for (oid, i, m, b) in &placed {
        let o = &ir.objects[*oid];
        if probe_of.as_ref().map(|p| p.0) != Some(*oid) {
            probe_of = Some((*oid, ir.probe(*oid)));
        }
        let probe = probe_of.as_ref().unwrap().1.as_ref();
        let src = &sources[&o.geometry];
        let (nm, mirror) = geom::normal_matrix(m);
        let center = b.center();
        let radius = b.radius();
        for (lod, lod_id) in [(0usize, NEAR), (1, MID), (2, FAR)] {
            if (lod == 1 && radius < profile.mid_radius) || (lod == 2 && radius < profile.far_radius) {
                continue;
            }
            let place = bins.place(lod_id, center);
            for (slot, tris) in &src.lods[lod] {
                if tris.is_empty() {
                    continue;
                }
                let mat = &ir.materials[o.material(*slot)];
                let unlit = mat.kind == "MeshBasicNodeMaterial";
                let kind = if mat.side == 2 { pack::kind::OPEN } else { pack::kind::SOLID };
                // The vertices this slot uses, once each.
                let mut remap: HashMap<u32, u32> = HashMap::new();
                let mut verts: Vec<Vx> = Vec::new();
                let mut idx: Vec<u32> = Vec::with_capacity(tris.len());
                for f in tris.chunks(3) {
                    // A face the probe saw as see-through stays out (worn paint, cut-outs).
                    if let Some(pr) = probe {
                        if f.iter().all(|&v| pr.albedo(*i, v as usize)[3] < 40) {
                            continue;
                        }
                    }
                    let order = if mirror { [f[0], f[2], f[1]] } else { [f[0], f[1], f[2]] };
                    for v in order {
                        let n = verts.len() as u32;
                        let j = *remap.entry(v).or_insert_with(|| {
                            let vi = v as usize;
                            let (color, glow) = match probe {
                                Some(pr) => {
                                    let a = pr.albedo(*i, vi);
                                    let e = pr.emission(*i, vi);
                                    if unlit {
                                        let lin = [geom::linear(a[0]), geom::linear(a[1]), geom::linear(a[2])];
                                        ([0, 0, 0, pack::GLOW_ALWAYS], glow_bytes(lin))
                                    } else {
                                        ([a[0], a[1], a[2], 0], glow_bytes(e))
                                    }
                                }
                                None => ([200, 0, 200, 0], [0; 4]),
                            };
                            let nn = geom::normal(&nm, src.nrm[vi]);
                            verts.push(Vx { p: geom::point(m, src.pos[vi]), n: nn.map(geom::snorm8), ao: 255, a: color, b: glow, uv: [0.0; 2] });
                            n
                        });
                        idx.push(j);
                    }
                }
                if !idx.is_empty() {
                    let bin: &mut Bin = bins.bin(lod_id, place, kind, pack::NO_TEX);
                    bin.add(&verts, &idx);
                }
            }
        }
    }
    let tris = bins.triangles();
    eprintln!("triangles: near {} mid {} far {}", tris[0], tris[1], tris[2]);

    let w = bins::Written::from_bins(&bins);
    let mut pw = pack::Writer::default();
    let meta = serde_json::json!({
        "name": "Pocket New York",
        "profile": profile.name,
        "source": "Slop New York by mrdoob (MIT), its SceneIR",
        "triangles": tris,
        "batches": w.batches.len(),
    });
    pw.add(pack::META, meta.to_string().into_bytes());
    pw.add(pack::CITY, pack::bytes_of(&city).to_vec());
    pw.add(pack::CELL, pack::slice_bytes(&w.cells).to_vec());
    pw.add(pack::BLCK, pack::slice_bytes(&w.blocks).to_vec());
    pw.add(pack::REGN, pack::slice_bytes(&w.regions).to_vec());
    pw.add(pack::BTCH, pack::slice_bytes(&w.batches).to_vec());
    pw.add(pack::VWAL, pack::slice_bytes(&w.walls).to_vec());
    pw.add(pack::VSOL, pack::slice_bytes(&w.solids).to_vec());
    pw.add(pack::VGND, pack::slice_bytes(&w.grounds).to_vec());
    pw.add(pack::IDX0, pack::slice_bytes(&w.indices).to_vec());
    let bytes = pw.finish();
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    std::fs::write(&out, &bytes).unwrap();
    eprintln!(
        "pack: {:.1} MB ({} cells {}×{}, {} batches, {} solid / {} wall / {} ground vertices, {} indices) in {:.1} s → {}",
        bytes.len() as f32 / 1e6,
        w.cells.len(),
        city.cells_x,
        city.cells_z,
        w.batches.len(),
        w.solids.len(),
        w.walls.len(),
        w.grounds.len(),
        w.indices.len(),
        t0.elapsed().as_secs_f32(),
        out.display()
    );
}
