// What moves needs from the reference, as the reference has it after it is built: the colliders its builders made,
// the ground the controllers stand on, the island's shore, the road network the traffic drives, the names the
// location label reads, the signal timing, the cameras and the four entities as they start. Writes `sim.json`.
//
// Then the oracle: the reference's own controllers, cameras, location label and signals driven headless by fixed
// scripts, recorded to `oracle/*.json` for `cargo test -p ny-sim` to replay through the port.

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
    network: network(app.traffic.network)
  };
  await put('sim.json', JSON.stringify(sim));
  log(`sim: ${sim.colliders.count} colliders, ${sim.blocks.length} blocks, ${sim.roads.length} roads, ${sim.network.nodes.length} nodes, ${sim.network.edges.length} edges`);

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
  log(`oracle: ${runs.length} controller runs`);
}
