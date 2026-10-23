// What moves needs from the reference, as the reference has it after it is built: the colliders its builders made,
// the ground the controllers stand on, the island's shore, the road network the traffic drives, where its
// pedestrians may walk, the highway's lanes, the names the location label reads, the signal timing, the cameras and
// the four entities as they start. Writes `sim.json`.
//
// Then the oracle: the reference's own controllers, cameras, location label, signals, highway traffic and sidewalk
// walkers driven headless, recorded to `oracle/*.json` for `cargo test -p ny-sim` to replay through the port.

import * as THREE from 'three/webgpu';
import { log, put } from './util.js';

const round = (x, k = 1e4) => Math.round(x * k) / k;

function colliders(list) {
  // minX, maxX, minZ, maxZ, maxY, then the oriented box (cx, cz, hx, hz, cs, sn) or six zeros; `obb` says which
  const box = [];
  const obb = [];
  for (const c of list) {
    box.push(c.minX, c.maxX, c.minZ, c.maxZ, c.maxY);
    if (c.obb) {
      obb.push(1);
      box.push(c.obb.cx, c.obb.cz, c.obb.hx, c.obb.hz, c.obb.cs, c.obb.sn);
    } else {
      obb.push(0);
      box.push(0, 0, 0, 0, 0, 0);
    }
  }
  return { count: list.length, box, obb };
}

/** The shore table getIslandBoundsAtZ interpolates (city-config.js SHORE): read back at its own points. */
function shore() {
  const NYC = window.NYC;
  let z0 = Infinity;
  let z1 = -Infinity;
  for (const p of NYC.MANHATTAN_COASTLINE) {
    z0 = Math.min(z0, p.z);
    z1 = Math.max(z1, p.z);
  }
  const step = 2;
  z0 = Math.ceil(z0 / step) * step;
  const n = Math.floor((z1 - z0) / step) + 1;
  const west = [];
  const east = [];
  for (let k = 0; k < n; k++) {
    const b = NYC.getIslandBoundsAtZ(z0 + k * step);
    west.push(b.westX);
    east.push(b.eastX);
  }
  return { z0, step, n, west, east };
}

function network(net) {
  const nodeId = new Map(net.nodes.map((n, i) => [n, i]));
  const edgeId = new Map(net.edges.map((e, i) => [e, i]));
  return {
    nodes: net.nodes.map((n) => ({ x: n.x, z: n.z, hx: n.hx, hz: n.hz, signal: !!n.signal, groups: n.groupCount ?? 2 })),
    edges: net.edges.map((e) => ({
      from: nodeId.get(e.from),
      to: nodeId.get(e.to),
      sx: e.sx, sz: e.sz, dx: e.dx, dz: e.dz, rx: e.rx, rz: e.rz, len: e.len,
      lanes: e.lanes,
      ave: e.axis === 'ave',
      group: e.axisIndex ?? 0,
      opposite: e.opposite ? edgeId.get(e.opposite) : -1,
      moves: e.moves.map((m) => ({ to: edgeId.get(m.to), turn: m.turn, out: m.out }))
    }))
  };
}

/** Steps of the offsets and lines below: the reference draws them from Math.random, the port from these. */
const OFF_STEPS = 53; // sidewalk offsets half + 1.4 + k × 0.05, k = 0..52 (traffic.js sidewalkTrack: + 0..2.6)
const LINE_STEPS = 12; // crosswalk lines 0.5 + k × 0.05, k = 0..11 (crosswalkTrack: + 0..0.55)

/** Where the reference's pedestrians may walk, as its own tests find it (traffic.js sidewalkTrack and
 *  crosswalkTrack, over road-network.js walkable and onSidewalk and city-config.js blockAt): per edge, which offsets
 *  to its right (`walk[0]`) and left (`walk[1]`) make a sidewalk track, bit k for half + 1.4 + k × 0.05; per
 *  signalized intersection and side 0..3, the road its crosswalk spans and which lines make one, bit k for
 *  0.5 + k × 0.05 on the side's own sign and bit 12 + k on the other. With what the vehicles need of the crosswalks:
 *  the sides an edge arrives and leaves over and the lateral axis lanes meet them along. */
function walks(net) {
  const { walkable, onSidewalk } = net;
  const { blockAt } = window.NYC;
  const walkable2D = (ax, az, bx, bz) => walkable(Math.min(ax, bx) - 0.4, Math.max(ax, bx) + 0.4, Math.min(az, bz) - 0.4, Math.max(az, bz) + 0.4);
  const edgeId = new Map(net.edges.map((e, i) => [e, i]));
  const sidewalk = (e, off) => {
    const ax = e.sx + e.rx * off;
    const az = e.sz + e.rz * off;
    if (e.general) {
      for (let s = 0; s < e.len + 3; s += 3) {
        const t = Math.min(s, e.len);
        const x = ax + e.dx * t;
        const z = az + e.dz * t;
        if (!walkable(x - 0.4, x + 0.4, z - 0.4, z + 0.4) || !blockAt(x, z)) return false;
      }
      return true;
    }
    if (!walkable2D(ax, az, ax + e.dx * e.len, az + e.dz * e.len)) return false;
    for (let s = 0; s < e.len + 3; s += 3) {
      const t = Math.min(s, e.len);
      if (!onSidewalk(ax + e.dx * t, az + e.dz * t)) return false;
    }
    return true;
  };
  const edges = net.edges.map((e) => ({
    half: e.half,
    general: !!e.general,
    inSide: e.inSide,
    outSide: e.outSide,
    clx: e.clx,
    clz: e.clz,
    walk: [1, -1].map((sign) => {
      let bits = 0;
      for (let k = 0; k < OFF_STEPS; k++) if (sidewalk(e, sign * (e.half + 1.4 + k * 0.05))) bits += 2 ** k;
      return bits;
    })
  }));
  const nodes = net.nodes.map((n) => {
    if (!n.signal) return null;
    return [0, 1, 2, 3].map((side) => {
      const road = n.road?.[side];
      if (!road) return null;
      const acrossAvenue = side >= 2;
      const sign = side % 2 === 0 ? 1 : -1;
      let bits = 0;
      for (const [b, ls] of [[0, sign], [LINE_STEPS, -sign]]) {
        for (let k = 0; k < LINE_STEPS; k++) {
          const line = ls * (0.5 + k * 0.05);
          const ax = acrossAvenue ? road.x0 - road.half - 2 : n.x + sign * (n.hx + 1.25) + line;
          const az = acrossAvenue ? n.z + sign * (n.hz + 1.25) + line : road.z0 - road.half - 2;
          const len = 2 * road.half + 4;
          const ux = acrossAvenue ? 1 : 0;
          const uz = acrossAvenue ? 0 : 1;
          const ok = walkable2D(ax, az, ax + ux * len, az + uz * len) && onSidewalk(ax, az) && onSidewalk(ax + ux * len, az + uz * len);
          if (ok) bits += 2 ** (b + k);
        }
      }
      return { road: edgeId.get(road), across: acrossAvenue ? road.x0 : road.z0, half: road.half, lines: bits };
    });
  });
  return { offSteps: OFF_STEPS, lineSteps: LINE_STEPS, edges, nodes };
}

/** The highway's lanes (city-grid.js highwayLanes, as createHighwayTraffic took them): centre-line points and speed. */
function highway(h) {
  return h.lanes.map((l) => ({ speed: l.speed, pts: l.pts.flatMap((p) => [p.x, p.y, p.z]) }));
}

function entities(controllers) {
  const out = {};
  for (const [mode, e] of Object.entries(controllers.entities)) {
    out[mode] = {
      type: e.type,
      label: e.label,
      position: e.position.toArray(),
      heading: e.heading,
      chaseDistance: e.chaseDistance,
      chaseHeight: e.chaseHeight,
      targetY: e.cameraTargetOffset.y
    };
  }
  return out;
}

// ---------------------------------------------------------------------------------------------------- the oracle

/** The highway's cars from the start (createHighwayTraffic's seeded fleet), stepped with no player for `frames`:
 *  every car's kind, paint, place and speed at the start and every tenth frame; then, from a camera over the lanes,
 *  the transforms the reference gives the cars it draws, in its order. */
function highwayRun(app) {
  const h = app.highway;
  const dt = 1 / 60;
  const kinds = ['taxi', 'sedan', 'suv'];
  const start = h.cars.map((c) => [h.lanes.indexOf(c.lane), kinds.indexOf(c.kind), ...c.col, c.u, c.v]);
  const camera = new THREE.PerspectiveCamera(55, 960 / 544, 0.5, 30000);
  camera.position.set(-2000, 120, 1800);
  camera.lookAt(-2100, 0, 1500);
  camera.updateMatrixWorld();
  const frames = [];
  for (let f = 1; f <= 900; f++) {
    h.update(dt, camera, null, 'helicopter');
    if (f % 10 === 0) frames.push([f, ...h.cars.flatMap((c) => [round(c.u, 1e5), round(c.v, 1e5)])]);
  }
  // Which car got which instance: the reference's order and culling (traffic.js update, step 2), read back from its
  // meshes' matrices.
  const group = app.scene.getObjectByName('HighwayTraffic');
  const meshes = group.children.filter((o) => o.isInstancedMesh);
  const fwd = new THREE.Vector3();
  camera.getWorldDirection(fwd);
  const used = new Map(meshes.map((m) => [m, 0]));
  const drawn = [];
  const kindMeshes = Object.fromEntries(kinds.map((k, i) => [k, meshes.slice(i * 3, i * 3 + 3)]));
  const m4 = new THREE.Matrix4();
  for (const [i, c] of h.cars.entries()) {
    const lane = c.lane;
    const seg = c.seg;
    const a = lane.pts[seg], b = lane.pts[seg + 1];
    const segLen = lane.cum[seg + 1] - lane.cum[seg];
    const t = Math.min(1, Math.max(0, (c.u - lane.cum[seg]) / segLen));
    const x = a.x + (b.x - a.x) * t, z = a.z + (b.z - a.z) * t;
    const toEnd = lane.len - c.u;
    const dip = Math.min(c.u, toEnd);
    const e = Math.min(1, Math.max(0, dip / 32));
    const y = a.y + (b.y - a.y) * t - 2.4 * (1 - e * e * (3 - 2 * e));
    const dx = x - camera.position.x, dy = y - camera.position.y, dz = z - camera.position.z;
    const d2 = dx * dx + dy * dy + dz * dz;
    if (d2 > 1600 * 1600) continue;
    const lod = d2 < 170 * 170 ? 0 : d2 < 450 * 450 ? 1 : 2;
    if (lod > 0 && dx * fwd.x + dz * fwd.z < -40) continue;
    if (y < -3.6) continue;
    const mesh = kindMeshes[c.kind][lod];
    const slot = used.get(mesh);
    used.set(mesh, slot + 1);
    mesh.getMatrixAt(slot, m4);
    drawn.push([i, ...m4.elements.map((v) => round(v, 1e5))]);
  }
  return { dt, start, frames, camera: { pos: camera.position.toArray(), fwd: fwd.toArray() }, drawn };
}

/** The reference's pedestrians around the start for `frames` with the camera fixed: those on sidewalks (not
 *  crosswalks), from the first frame on their track to the last, as they walk it (traffic.js walk). */
function walkersRun(app) {
  const t = app.traffic;
  const dt = 1 / 60;
  const camera = new THREE.PerspectiveCamera(56, 960 / 544, 0.5, 30000);
  camera.position.set(3.6, 2.6, 403.7);
  camera.lookAt(3.6, 1.5, 415.8);
  camera.updateMatrixWorld();
  const focus = camera.position;
  // A run: one pedestrian on one sidewalk track, from the first frame it is seen there to the last.
  const current = new Map();
  const runs = [];
  let elapsed = 3.0;
  for (let f = 0; f < 1200; f++) {
    elapsed += dt;
    t.update(dt, elapsed, focus, null, 'helicopter', camera);
    for (const [i, p] of t.peds.entries()) {
      const key = p.placed && !p.node ? `${p.ax},${p.az},${p.len}` : null;
      let r = current.get(i);
      if (r && r.key !== key) {
        runs.push(r);
        current.delete(i);
        r = null;
      }
      if (!key) continue;
      if (!r) {
        r = { key, ped: i, from: f, start: { ax: p.ax, az: p.az, ux: p.ux, uz: p.uz, len: p.len, t: p.t, dir: p.dir, v: p.v, pause: p.pause, speed: p.speed }, t: [] };
        current.set(i, r);
      }
      r.t.push([round(p.t, 1e5), round(p.v, 1e5), p.dir]);
    }
  }
  runs.push(...current.values());
  return { dt, runs: runs.filter((r) => r.t.length > 120).map(({ key, ...r }) => r) };
}

const KEYS = ['forward', 'backward', 'left', 'right', 'up', 'down', 'boost'];

/** Per mode, a script of [frames, keys held] that drives along, turns into buildings and uses every key. */
const SCRIPTS = {
  car: [[90, 'forward'], [60, 'forward boost'], [80, 'forward left'], [60, 'forward'], [50, ''], [70, 'backward'], [40, 'backward right'], [90, 'forward right'], [120, 'forward boost left'], [60, '']],
  person: [[60, 'forward'], [40, 'forward up'], [60, 'forward boost'], [50, 'forward left'], [90, 'forward right'], [30, 'up'], [60, 'backward'], [80, 'forward boost left'], [40, '']],
  cyclist: [[90, 'forward'], [60, 'forward boost'], [70, 'forward left'], [40, ''], [60, 'backward'], [90, 'forward right'], [120, 'forward boost'], [40, '']],
  helicopter: [[60, 'up'], [90, 'forward'], [60, 'forward boost left'], [40, 'down'], [90, 'forward right'], [60, 'backward'], [60, 'down'], [90, 'forward boost'], [60, '']]
};

function drive(app, mode, preset) {
  const { controllers, cameras } = app;
  const dt = 1 / 60;
  controllers.setMode(mode);
  controllers.resetInput();
  const e = controllers.getActiveEntity();
  // Every number of the entity's state (and the car's chassis), so the replay starts where this run does
  const start = { position: e.position.toArray() };
  for (const [k, v] of Object.entries(e)) if (typeof v === 'number') start[k] = v;
  if (e.chassisPivot) start.chassis = [e.chassisPivot.rotation.x, e.chassisPivot.rotation.z, e.chassisPivot.position.y];
  cameras.resetForTeleport();
  cameras.setCameraPreset(preset, true);
  const frames = [];
  let elapsed = 0;
  for (const [count, held] of SCRIPTS[mode]) {
    const on = new Set(held.split(' ').filter(Boolean));
    for (const k of KEYS) controllers.keys[k] = on.has(k);
    for (let i = 0; i < count; i++) {
      elapsed += dt;
      controllers.update(dt, elapsed);
      cameras.update(dt, elapsed);
      frames.push([
        KEYS.map((k) => (controllers.keys[k] ? 1 : 0)).join(''),
        ...e.position.toArray().map((v) => round(v)),
        round(e.heading, 1e6),
        round(e.speed, 1e5),
        ...cameras.currentPos.toArray().map((v) => round(v)),
        ...cameras.currentLookAt.toArray().map((v) => round(v))
      ]);
    }
  }
  for (const k of KEYS) controllers.keys[k] = false;
  return { mode, preset, start, frames };
}

function scenic(app) {
  const { cameras } = app;
  const dt = 1 / 60;
  const out = [];
  for (const p of window.NYC.CAMERA_PRESETS) {
    if (p.category !== 'scenic') continue;
    cameras.resetForTeleport();
    cameras.setCameraPreset(p.id, true);
    const frames = [];
    let elapsed = 3.0;
    for (let i = 0; i < 240; i++) {
      elapsed += dt;
      cameras.update(dt, elapsed);
      frames.push([round(elapsed, 1e6), ...cameras.currentPos.toArray().map((v) => round(v)), ...cameras.currentLookAt.toArray().map((v) => round(v))]);
    }
    out.push({ id: p.id, start: 3.0, frames });
  }
  cameras.resetForTeleport();
  return out;
}

function labels() {
  const NYC = window.NYC;
  const b = NYC.ISLAND_BOUNDS;
  const out = [];
  // A grid over the island and the water around it, and every landmark's spot
  for (let z = b.minZ - 200; z <= b.maxZ + 200; z += 97) {
    // (points at whole metres: what the port is asked is exactly what the reference was asked)
    for (let x = b.minX - 300; x <= b.maxX + 300; x += 83) out.push([Math.round(x), Math.round(z), NYC.getLocationLabel(Math.round(x), Math.round(z))]);
  }
  for (const l of NYC.NYC_LANDMARKS) out.push([Math.round(l.x + 11), Math.round(l.z - 7), NYC.getLocationLabel(Math.round(l.x + 11), Math.round(l.z - 7))]);
  return out;
}

function signals() {
  const NYC = window.NYC;
  const out = [];
  for (let t = -30; t < 250; t += 0.37) out.push([round(t, 1e4), NYC.signalPhase(t, true), NYC.signalPhase(t, false), NYC.walkSignal(t, true), NYC.walkSignal(t, false)]);
  return out;
}

function teleports(app) {
  const { controllers } = app;
  const out = [];
  for (const mode of ['person', 'car', 'cyclist', 'helicopter']) {
    controllers.setMode(mode);
    for (const [id, loc] of Object.entries(window.NYC.TELEPORT_LOCATIONS)) {
      // where every entity stands before the jump: the arrival keeps clear of the others
      const before = Object.fromEntries(Object.entries(controllers.entities).map(([m, e]) => [m, e.position.toArray()]));
      const d = controllers.teleportActiveTo(loc.x, loc.z, loc.heading);
      const e = controllers.getActiveEntity();
      out.push({ mode, id, x: loc.x, z: loc.z, heading: loc.heading, before, to: [round(d.x), round(e.position.y), round(d.z)] });
    }
  }
  return out;
}

export async function exportSim(app) {
  const NYC = window.NYC;
  // The four entities as they start, before the oracle moves them
  const ents = entities(app.controllers);
  const sim = {
    version: 1,
    colliders: colliders(app.colliders),
    island: NYC.ISLAND_BOUNDS,
    centralPark: NYC.CENTRAL_PARK_BOUNDS,
    bryantLawn: NYC.bryantParkLawn ?? null,
    cutZ: NYC.CUT_Z,
    shore: shore(),
    blocks: NYC.BLOCKS.map((b) => ({ poly: b.poly.flatMap((p) => [p.x, p.z]), rect: b.rect, park: b.park, median: b.median, minX: b.minX, maxX: b.maxX, minZ: b.minZ, maxZ: b.maxZ })),
    roads: NYC.ROADS.map((r) => ({ name: r.name, pts: r.pts.flatMap((p) => [p.x, p.z]) })),
    streets: NYC.ALL_STREETS.filter((s) => s.number).map((s) => ({ number: s.number, short: s.short, z: s.z })),
    landmarks: NYC.NYC_LANDMARKS.map((l) => ({ id: l.id, name: l.name, x: l.x, z: l.z })),
    teleports: Object.entries(NYC.TELEPORT_LOCATIONS).map(([id, t]) => ({ id, x: t.x, z: t.z, heading: t.heading })),
    cameras: NYC.CAMERA_PRESETS.map((c) => ({ id: c.id, fov: c.fov, scenic: c.category === 'scenic' })),
    signal: NYC.SIGNAL_TIMING,
    // The names getLocationLabel writes itself (city-config.js), besides the roads', streets' and landmarks'
    specials: ['Central Park', 'Hudson River Waterfront', 'East River Waterfront', 'New York Harbor'],
    entities: ents,
    network: network(app.traffic.network),
    walks: walks(app.traffic.network),
    highway: highway(app.highway)
  };
  await put('sim.json', JSON.stringify(sim));
  log(`sim: ${sim.colliders.count} colliders, ${sim.blocks.length} blocks, ${sim.roads.length} roads, ${sim.network.nodes.length} nodes, ${sim.network.edges.length} edges, ${sim.highway.length} highway lanes`);

  // The oracle runs on the reference's own objects; it moves the entities, so it comes after the record above.
  const runs = [];
  for (const mode of ['car', 'person', 'cyclist', 'helicopter']) runs.push(drive(app, mode, 'chase'));
  runs.push(drive(app, 'car', 'action'));
  runs.push(drive(app, 'cyclist', 'pov'));
  await put('oracle/controllers.json', JSON.stringify({ dt: 1 / 60, keys: KEYS, scripts: SCRIPTS, runs }));
  await put('oracle/scenic.json', JSON.stringify({ dt: 1 / 60, runs: scenic(app) }));
  await put('oracle/labels.json', JSON.stringify(labels()));
  await put('oracle/signals.json', JSON.stringify(signals()));
  await put('oracle/teleports.json', JSON.stringify(teleports(app)));
  await put('oracle/highway.json', JSON.stringify(highwayRun(app)));
  await put('oracle/walkers.json', JSON.stringify(walkersRun(app)));
  log(`oracle: ${runs.length} controller runs`);
}
