//! The scene compiler: SceneIR in, a device pack out.
//!
//!   ny-cook --ir .pocket-build/scene/ir --profile profiles/vita60.json --out .pocket-build/scene/vita60/city.pack

mod bins;
mod facade;
mod geom;
mod ir;
mod tex;
mod sim;

use bins::{Bin, Bins, Vx, DETAIL, FAR, MID, NEAR};
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
    /// Share of a part's triangles each level keeps (near, mid, far, detail), and the error (relative to the
    /// part's size) it may make.
    keep: [f32; 4],
    error: [f32; 4],
    /// An instance shows at the mid and the far level when its bounding radius is at least this.
    mid_radius: f32,
    far_radius: f32,
    /// An instance of something repeated smaller than this (a lamp, a sign, a hydrant) is a cell's detail.
    detail_radius: f32,
}

/// How the compiler treats an object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    /// Not part of the static city: what moves (players, traffic, people), effects, the sky.
    Skip,
    /// Coloured by its probed vertices.
    Solid,
    /// Facade boxes: walls under the baked pages, roofs by their probed vertices.
    Facade,
}

fn classify(ir: &Ir, o: &Object, facades: &facade::Facades) -> Class {
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
    let boxed = o.instances.as_ref().is_some_and(|i| i.attrs.contains_key("box") && i.attrs.contains_key("faces"));
    if boxed && ir.material_key(o.material(0)).is_some_and(|k| facades.styles.contains_key(&k)) {
        return Class::Facade;
    }
    Class::Solid
}

/// A geometry's vertices in its own frame, as floats.
struct Source {
    pos: Vec<[f32; 3]>,
    nrm: Vec<[f32; 3]>,
    /// Triangles by material slot, then simplified for the mid and far level.
    lods: [Vec<(usize, Vec<u32>)>; 4],
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
    // Vertices at one place are one vertex to the simplifier (merged geometry repeats them per triangle); what it
    // keeps is the first of each, whose own normal and colour stay.
    let mut first_at: HashMap<[i32; 3], u32> = HashMap::new();
    let weld: Vec<u32> = pos
        .iter()
        .enumerate()
        .map(|(i, p)| *first_at.entry(p.map(|x| (x * 1000.0).round() as i32)).or_insert(i as u32))
        .collect();
    let simplify = |t: &[u32], keep: f32, error: f32| -> Vec<u32> {
        if keep >= 1.0 || t.len() < 36 {
            return t.to_vec();
        }
        let welded: Vec<u32> = t.iter().map(|&i| weld[i as usize]).collect();
        let bytes: &[u8] = unsafe { std::slice::from_raw_parts(pos.as_ptr() as *const u8, pos.len() * 12) };
        let adapter = meshopt::VertexDataAdapter::new(bytes, 12, 0).unwrap();
        let target = ((t.len() / 3) as f32 * keep) as usize * 3;
        let mut out = meshopt::simplify(&welded, &adapter, target.max(3), error, meshopt::SimplifyOptions::None, None);
        // Merged clutter (curbs, kerbs, benches as separate pieces) has no edges between its pieces to collapse:
        // past the near level it goes by a grid, no finer than `sloppy` metres.
        if out.len() > target * 3 / 2 && keep < 0.5 {
            let extent = geom::length(bounds.size()).max(1.0);
            let sloppy = if keep < 0.2 { 6.0 } else { 1.5 };
            out = meshopt::simplify_sloppy(&welded, &adapter, target.max(3), sloppy / extent, None);
        }
        // Triangles that welding folded flat are gone.
        out.chunks(3).filter(|f| f[0] != f[1] && f[1] != f[2] && f[0] != f[2]).flatten().copied().collect()
    };
    let lods = [0, 1, 2, 3].map(|l| tris.iter().map(|(slot, t)| (*slot, simplify(t, profile.keep[l], profile.error[l]))).collect());
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

    let mut textures = tex::Textures::default();
    let facades = facade::load(&ir_dir, &mut textures).unwrap_or_else(|e| panic!("{e}"));
    eprintln!("facades: {} styles, {} pictures", facades.styles.len(), textures.entries.len());
    let classes: Vec<Class> = ir.objects.iter().map(|o| classify(&ir, o, &facades)).collect();
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
            if b.radius() < 1500.0 {
                world.union(&b);
            }
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
    let mut attrs_cache: HashMap<(usize, String), Vec<f32>> = HashMap::new();
    // Shapes of repeated things by (object, slot, level): the shape and its mean albedo; and the mean albedo of
    // each probed instance, by (object, slot, instance).
    let mut shapes: HashMap<(usize, usize, usize), (u32, [f32; 3])> = HashMap::new();
    let mut means: HashMap<(usize, usize, usize), [f32; 3]> = HashMap::new();
    let mut inst_colors: HashMap<usize, Vec<f32>> = HashMap::new();
    // Triangles by level and by what they come from, for the receipt.
    let mut spend: HashMap<(usize, String), usize> = HashMap::new();
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
        let class = classes[*oid];
        // Across the rivers the city is seen from afar: no near level.
        let distant = o.path.starts_with("Boroughs/BoroughBuildings") || o.path.starts_with("Boroughs/BoroughIsland");
        if class == Class::Facade {
            facade_instance(&ir, o, *i, m, b, &facades, &textures, probe, &mut bins, &profile, &mut attrs_cache, &mut spend);
        }
        // A facade box's far level is its height's to decide (see `facade_instance`).
        let tall = class == Class::Facade && b.size()[1] >= 20.0;
        // Small repeated things are a cell's detail, and only that.
        let small = o.instances.is_some() && class == Class::Solid && radius < profile.detail_radius;
        // What a building carries (cornices, fire escapes, rooftop plant) is seen near it alone.
        let carried = o.path.starts_with("ManhattanCityBlocks") && class == Class::Solid;
        let factory = ir.materials[o.material(0)].factory.as_deref();
        let near_only = carried || matches!(factory, Some("createCurbMaterial" | "createTreeGrateMaterial")) || o.path.starts_with("Boroughs/BoroughIslandTrees");
        let to_mid = !matches!(factory, Some("createSidewalkMaterial" | "createParkPavingMaterial"));
        for (lod, lod_id) in [(0usize, NEAR), (1, MID), (2, FAR), (3, DETAIL)] {
            let wanted = if small {
                lod == 3
            } else {
                match lod {
                    0 => !distant,
                    1 => radius >= profile.mid_radius && !near_only,
                    2 => (radius >= profile.far_radius || tall) && !near_only && to_mid && factory != Some("createRoadPaintMaterial"),
                    _ => false,
                }
            };
            if !wanted {
                continue;
            }
            // Something larger than a cell (the island's streets, its parks, the water) is cut by triangle.
            if o.instances.is_none() && radius > profile.cell * 2.0 && class == Class::Solid {
                for (slot, tris) in &src.lods[lod] {
                    let mat = &ir.materials[o.material(*slot)];
                    let unlit = mat.kind == "MeshBasicNodeMaterial";
                    let kind = if mat.side == 2 { pack::kind::OPEN } else { pack::kind::SOLID };
                    let mut groups: HashMap<(u32, u8), Vec<u32>> = HashMap::new();
                    for f in tris.chunks(3) {
                        let c = geom::scale(geom::add(geom::add(geom::point(m, src.pos[f[0] as usize]), geom::point(m, src.pos[f[1] as usize])), geom::point(m, src.pos[f[2] as usize])), 1.0 / 3.0);
                        groups.entry(bins.place(lod_id, c)).or_default().extend_from_slice(f);
                    }
                    let label = format!("{} {} (cut)", o.path.split('/').take(2).collect::<Vec<_>>().join("/"), mat.factory.clone().unwrap_or_else(|| format!("{}#{:06x}", mat.kind.replace("NodeMaterial", ""), mat.color.unwrap_or(0))));
                    for ((place, sub), part) in groups {
                        let (verts, idx) = vertices(src, &part, probe, *i, Some((m, &nm, mirror)), unlit, false);
                        if !idx.is_empty() {
                            *spend.entry((lod, label.clone())).or_default() += idx.len() / 3;
                            bins.bin(lod_id, place, kind, pack::NO_TEX).add(&verts, &idx, sub);
                        }
                    }
                }
                continue;
            }
            let (place, sub) = bins.place(lod_id, center);
            for (slot, tris) in &src.lods[lod] {
                if tris.is_empty() {
                    continue;
                }
                let mat = &ir.materials[o.material(*slot)];
                let unlit = mat.kind == "MeshBasicNodeMaterial";
                let label = format!("{} {}", o.path.split('/').take(2).collect::<Vec<_>>().join("/"), mat.factory.clone().unwrap_or_else(|| format!("{}#{:06x}", mat.kind.replace("NodeMaterial", ""), mat.color.unwrap_or(0))));
                // Something repeated is one shape drawn at each of its instances.
                if class == Class::Solid && o.instances.as_ref().is_some_and(|x| x.count >= 4) && src.pos.len() < 65536 {
                    let key = (*oid, *slot, lod);
                    // An object with instance colours (awnings) keeps them as its tint, over the shape's colour
                    // without them; any other per-instance difference rides as its mean albedo against the first's.
                    let colors = o.instances.as_ref().and_then(|x| x.color.as_ref()).map(|a| inst_colors.entry(*oid).or_insert_with(|| ir.floats(a, false)).clone());
                    let (shape, mean0) = *shapes.entry(key).or_insert_with(|| {
                        let (mut verts, idx) = vertices(src, tris, probe, 0, None, unlit, false);
                        if idx.is_empty() {
                            return (u32::MAX, [1.0; 3]);
                        }
                        if let Some(c) = &colors {
                            for v in &mut verts {
                                for k in 0..3 {
                                    v.a[k] = geom::srgb(geom::linear(v.a[k]) / c[k].max(1e-3));
                                }
                            }
                        }
                        (bins.shape(&verts, &idx), mean_albedo(probe, 0, &verts_of(tris)))
                    });
                    if shape == u32::MAX {
                        continue;
                    }
                    let tint: [u8; 3] = match &colors {
                        Some(c) => [0, 1, 2].map(|k| (c[*i * 3 + k] * 128.0).round().clamp(0.0, 255.0) as u8),
                        None => {
                            let mean = *means.entry((*oid, *slot, probe.map(|p| *i % p.instances).unwrap_or(0))).or_insert_with(|| mean_albedo(probe, *i, &verts_of(tris)));
                            [0, 1, 2].map(|k| ((mean[k] / mean0[k].max(1e-4)) * 128.0).round().clamp(0.0, 255.0) as u8)
                        }
                    };
                    let rows = [[m[0], m[4], m[8], m[12]], [m[1], m[5], m[9], m[13]], [m[2], m[6], m[10], m[14]]];
                    let kind = if mat.side == 2 { pack::kind::OPEN_INST } else { pack::kind::SOLID_INST };
                    bins.instance(lod_id, (place, sub), kind, shape, pack::Instance { rows, tint: [tint[0], tint[1], tint[2], 255] }, b);
                    *spend.entry((lod, label)).or_default() += tris.len() / 3;
                    continue;
                }
                let kind = if mat.side == 2 { pack::kind::OPEN } else { pack::kind::SOLID };
                // A facade box gives its roof only: the walls are `facade_instance`'s, the bottom is never seen.
                let (verts, idx) = vertices(src, tris, probe, *i, Some((m, &nm, mirror)), unlit, class == Class::Facade);
                if !idx.is_empty() {
                    *spend.entry((lod, label)).or_default() += idx.len() / 3;
                    let bin: &mut Bin = bins.bin(lod_id, place, kind, pack::NO_TEX);
                    bin.add(&verts, &idx, sub);
                }
            }
        }
    }
    let tris = bins.triangles();
    eprintln!("triangles: near {} mid {} far {} detail {}", tris[0], tris[1], tris[2], tris[3]);
    let mut spent: Vec<_> = spend.into_iter().collect();
    spent.sort_by(|a, b| b.1.cmp(&a.1));
    for want in [0usize, 1, 2, 3] {
        for ((_, label), n) in spent.iter().filter(|e| e.0 .0 == want).take(if want == 0 { 12 } else { 8 }) {
            eprintln!("  {} {:>9} {label}", ["near", "mid", "far", "detail"][want], n);
        }
    }

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
    pw.add(pack::INST, pack::slice_bytes(&w.instances).to_vec());
    pw.add(pack::SPAN, pack::slice_bytes(&w.spans).to_vec());
    pw.add(pack::TEXS, pack::slice_bytes(&textures.entries).to_vec());
    pw.add(pack::TDAT, textures.data.clone());
    sim::add(&mut pw, &ir_dir).unwrap_or_else(|e| panic!("{e}"));
    let bytes = pw.finish();
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    std::fs::write(&out, &bytes).unwrap();
    eprintln!(
        "pack: {:.1} MB ({} cells {}×{}, {} batches, {} solid / {} wall / {} ground vertices, {} indices, {} instances, {:.1} MB of pictures) in {:.1} s → {}",
        bytes.len() as f32 / 1e6,
        w.cells.len(),
        city.cells_x,
        city.cells_z,
        w.batches.len(),
        w.solids.len(),
        w.walls.len(),
        w.grounds.len(),
        w.indices.len(),
        w.instances.len(),
        textures.data.len() as f32 / 1e6,
        t0.elapsed().as_secs_f32(),
        out.display()
    );
}

/// One facade box: its walls under the pages at the near and the mid level, and at the far level a box painted
/// with the pages' mean colours (the roof comes from the probe, as for any solid).
#[allow(clippy::too_many_arguments)]
fn facade_instance(ir: &Ir, o: &Object, i: usize, m: &M4, b: &Aabb, facades: &facade::Facades, textures: &tex::Textures, probe: Option<&ir::Probe>, bins: &mut Bins, profile: &Profile, cache: &mut HashMap<(usize, String), Vec<f32>>, spend: &mut HashMap<(usize, String), usize>) {
    let _ = probe;
    let Some(style) = ir.material_key(o.material(0)).and_then(|k| facades.styles.get(&k)) else { return };
    let inst = o.instances.as_ref().unwrap();
    let mut attr = |name: &str| -> [f32; 4] {
        let a = &inst.attrs[name];
        let v = cache.entry((o.id, name.to_string())).or_insert_with(|| ir.floats(&a.array, a.normalized));
        [v[i * 4], v[i * 4 + 1], v[i * 4 + 2], v[i * 4 + 3]]
    };
    let (bx, faces) = (attr("box"), attr("faces"));
    let mut quads = Vec::new();
    facade::box_walls(style, m, bx, faces, &mut quads);
    let center = b.center();
    // Near: every quad.
    *spend.entry((0, "facade walls".into())).or_default() += quads.len() * 2;
    let (near, _) = bins.place(NEAR, center);
    for q in &quads {
        bins.bin(NEAR, near, pack::kind::WALL, q.tex).add(&q.v, &[0, 1, 2, 0, 2, 3], 0);
    }
    // Mid: one quad a wall, under the page of its upper storeys.
    let mut simple = Vec::new();
    facade::box_walls_simple(style, m, bx, faces, &mut simple);
    if b.radius() >= profile.mid_radius {
        *spend.entry((1, "facade walls".into())).or_default() += simple.len() * 2;
        let (mid, sub) = bins.place(MID, center);
        for q in &simple {
            bins.bin(MID, mid, pack::kind::WALL, q.tex).add(&q.v, &[0, 1, 2, 0, 2, 3], sub);
        }
    }
    // Far: tall boxes as painted walls, the colour of their pages.
    if b.size()[1] >= 20.0 || b.radius() >= profile.far_radius {
        *spend.entry((2, "facade walls".into())).or_default() += simple.len() * 2;
        let (far, sub) = bins.place(FAR, center);
        for q in &simple {
            let mean = textures.mean(q.tex);
            let night = textures.entries[q.tex as usize].night;
            let glow = if night != pack::NO_TEX { textures.mean(night).map(|c| c * textures.entries[night as usize].metres) } else { [0.0; 3] };
            let color = [geom::srgb(mean[0]), geom::srgb(mean[1]), geom::srgb(mean[2]), 0];
            let g = glow_bytes(glow);
            let v: Vec<Vx> = q.v.iter().map(|v| Vx { a: color, b: g, ..*v }).collect();
            bins.bin(FAR, far, pack::kind::SOLID, pack::NO_TEX).add(&v, &[0, 1, 2, 0, 2, 3], sub);
        }
    }
}

/// The vertices a slot's triangles use, once each, coloured by the probe of `inst`; in the world through `world`
/// (its matrix, normal matrix and whether it mirrors), or in the geometry's own frame.
fn vertices(src: &Source, tris: &[u32], probe: Option<&ir::Probe>, inst: usize, world: Option<(&M4, &[f32; 9], bool)>, unlit: bool, roof_only: bool) -> (Vec<Vx>, Vec<u32>) {
    let mut remap: HashMap<u32, u32> = HashMap::new();
    let mut verts: Vec<Vx> = Vec::new();
    let mut idx: Vec<u32> = Vec::with_capacity(tris.len());
    let mirror = world.is_some_and(|w| w.2);
    for f in tris.chunks(3) {
        if roof_only && src.nrm[f[0] as usize][1] < 0.5 {
            continue;
        }
        // A face the probe saw as see-through stays out (worn paint, cut-outs).
        if let Some(pr) = probe {
            if f.iter().all(|&v| pr.albedo(inst, v as usize)[3] < 40) {
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
                        let a = pr.albedo(inst, vi);
                        let e = pr.emission(inst, vi);
                        if unlit {
                            let lin = [geom::linear(a[0]), geom::linear(a[1]), geom::linear(a[2])];
                            ([0, 0, 0, pack::GLOW_ALWAYS], glow_bytes(lin))
                        } else {
                            ([a[0], a[1], a[2], 0], glow_bytes(e))
                        }
                    }
                    None => ([200, 0, 200, 0], [0; 4]),
                };
                let (p, nn) = match world {
                    Some((m, nm, _)) => (geom::point(m, src.pos[vi]), geom::normal(nm, src.nrm[vi])),
                    None => (src.pos[vi], geom::normalize(src.nrm[vi])),
                };
                verts.push(Vx { p, n: nn.map(geom::snorm8), ao: 255, a: color, b: glow, uv: [0.0; 2] });
                n
            });
            idx.push(j);
        }
    }
    (verts, idx)
}

fn verts_of(tris: &[u32]) -> Vec<u32> {
    let mut v: Vec<u32> = tris.to_vec();
    v.sort_unstable();
    v.dedup();
    v
}

/// The mean albedo (linear) of an instance's vertices, by its probe.
fn mean_albedo(probe: Option<&ir::Probe>, inst: usize, verts: &[u32]) -> [f32; 3] {
    let Some(pr) = probe else { return [1.0; 3] };
    let mut s = [0.0f32; 3];
    for &v in verts {
        let a = pr.albedo(inst, v as usize);
        for k in 0..3 {
            s[k] += geom::linear(a[k]);
        }
    }
    let n = verts.len().max(1) as f32;
    s.map(|x| x / n)
}
