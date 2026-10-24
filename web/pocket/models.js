// The things that move, as models in their own frames, coloured by their own materials.
//
// Traffic: the reference's own stand-in cars (buildCarProxyGeometry, which it draws past 30 m: about 250 triangles
// with the wheels built in) for the taxi, the sedan and the SUV, the sedan with the NYPD light bar, and the bus with
// its four wheels where placeWheels puts them. The player: the taxi, the bicycle and the helicopter as the
// controllers built them (the groups PlayerTaxiCar, PlayerCyclist, PlayerHelicopter), each part in the group's
// frame, but the helicopter's two rotors, each a model in the frame of the pivot it turns about.
//
// Every vertex is drawn twice through its material's colour graph, with white paint and with black paint
// (`paintColor`), so the compiler knows how much of it is paint, and once through its emission graph by night.
// Writes `models.json` and `models.bin` (arrays as scene.js writes them).
//
// Cars also get their look as pictures (`boxed`): the car material paints lamps, grille, trim and windows from where a
// point is on the body, finer than its vertices, so the car is drawn flat from each of its axes into one atlas
// (`bake/car_<name>.png`: sRGB with white paint; `_paint.png`: how much of it is paint, as grey; `_glow.png`: its
// emission by night over a scale; `_gloss.png`: how much of the sky it mirrors, (1 − roughness) / 0.85 as grey),
// and a program picks the view by the surface's normal. The near traffic models are the reference's full
// cars (buildCarGeometry), which it draws within 90 m.

import * as THREE from 'three/webgpu';
import { float, floor, mod, positionLocal, positionWorld, uniform, vec3, vec4, vertexIndex } from 'three/tsl';
import { Buffers } from './scene.js';
import { log, put } from './util.js';

const W = 1024;
/** Metres a position is raised by on its way through a colour target, and lowered by after. */
const LIFT = 100;

const toSrgb = (x) => {
  x = Math.min(1, Math.max(0, x));
  return Math.round((x <= 0.0031308 ? x * 12.92 : 1.055 * Math.pow(x, 1 / 2.4) - 0.055) * 255);
};

/** The parts of every model: geometry, material, and its matrix in the model's frame. */
function parts(app) {
  const NYC = window.NYC;
  const S = NYC.BODY_SPECS;
  const id = new THREE.Matrix4();
  const out = [];
  for (const kind of ['taxi', 'sedan', 'suv']) {
    const mat = NYC.createLoftedCarMaterial(S[kind]);
    out.push({ model: kind, geo: NYC.buildCarProxyGeometry(S[kind]), mat, m: id, boxed: kind });
    out.push({ model: `${kind}-near`, geo: NYC.buildCarGeometry(S[kind], true), mat, m: id, boxed: kind });
  }
  const police = NYC.createLoftedCarMaterial(S.sedan);
  out.push({ model: 'nypd', geo: NYC.buildCarProxyGeometry(S.sedan), mat: police, m: id, boxed: 'nypd' });
  out.push({ model: 'nypd', geo: NYC.buildLightbarGeometry(S.sedan), mat: NYC.createLightbarMaterial(), m: id });
  out.push({ model: 'nypd-near', geo: NYC.buildCarGeometry(S.sedan, true), mat: police, m: id, boxed: 'nypd' });
  out.push({ model: 'nypd-near', geo: NYC.buildLightbarGeometry(S.sedan), mat: NYC.createLightbarMaterial(), m: id });
  const B = NYC.BUS_SPEC;
  out.push({ model: 'bus', geo: NYC.buildLoftedBusBodyGeometry(), mat: NYC.createLoftedBusMaterial(), m: id });
  const wheelMat = NYC.createBusWheelMaterial();
  for (const side of [-1, 1]) {
    const geo = NYC.buildBusWheelGeo(side);
    for (const wz of [B.wheelZ, -B.wheelZ]) out.push({ model: 'bus', geo, mat: wheelMat, m: new THREE.Matrix4().makeTranslation(side * B.wheelX, B.wheelRadius, wz) });
  }
  const count = (geo) => geo.attributes.position.count;
  const heliBody = count(NYC.buildHelicopterBodyGeometry());
  // The helicopter's rotors, models of their own in the frame of their pivots (mainRotorPivot, tailRotorPivot), which
  // the runtime turns as the controllers do.
  const rotors = new Map([[count(NYC.buildHelicopterMainRotorGeometry()), 'player-heli-rotor'], [count(NYC.buildHelicopterTailRotorGeometry()), 'player-heli-tail']]);
  // The player's vehicles, in the frame of their group.
  for (const [model, name] of [['player-car', 'PlayerTaxiCar'], ['player-cyclist', 'PlayerCyclist'], ['player-helicopter', 'PlayerHelicopter']]) {
    const root = app.scene.getObjectByName(name);
    if (!root) continue;
    root.updateMatrixWorld(true);
    const inv = root.matrixWorld.clone().invert();
    root.traverse((o) => {
      if (!o.isMesh || o.material?.userData?.pocket?.factory === 'createContactShadowMaterial') return;
      const mat = Array.isArray(o.material) ? o.material[0] : o.material;
      // (the helicopter's body alone: its rotors' blades would lie over the body's view from above)
      const factory = mat.userData?.pocket?.factory;
      const boxed =
        model === 'player-car' && factory === 'createLoftedCarMaterial'
          ? 'player-car'
          : model === 'player-helicopter' && factory === 'createLoftedHelicopterMaterial' && o.geometry.attributes.position.count === heliBody
            ? 'player-helicopter'
            : undefined;
      const rotor = model === 'player-helicopter' && factory === 'createLoftedHelicopterMaterial' ? rotors.get(count(o.geometry)) : undefined;
      if (rotor) {
        const pivot = inv.clone().multiply(o.parent.matrixWorld);
        log(`  ${rotor}: pivot at ${new THREE.Vector3().setFromMatrixPosition(pivot).toArray().map((x) => x.toFixed(3))}`);
        // (cut where the material's yellow tips begin and end, its radius 4.35 to 4.42 m on the main rotor and its
        // height 0.65 to 0.68 m on the tail rotor: a blade has vertices at its ends alone, and a tip's colour there
        // would run down the whole blade)
        let geo = slice(o.geometry, (x, y, z) => Math.hypot(x, z), [4.35, 4.42]);
        geo = slice(geo, (x, y, z) => Math.abs(y), [0.65, 0.68]);
        out.push({ model: rotor, geo, mat, m: o.parent.matrixWorld.clone().invert().multiply(o.matrixWorld), name: o.name });
        return;
      }
      out.push({ model, geo: o.geometry, mat, m: inv.clone().multiply(o.matrixWorld), name: o.name, boxed });
    });
  }
  return out;
}

/** `geo` with its triangles cut where `f` (of a vertex's position) crosses each of `levels`, every attribute taken
 *  along the cut edges. */
function slice(geo, f, levels) {
  let src = geo.index ? geo.toNonIndexed() : geo;
  for (const level of levels) {
    const names = Object.keys(src.attributes).filter((k) => !src.attributes[k].isInstancedBufferAttribute);
    const attrs = names.map((k) => src.attributes[k]);
    const pos = src.attributes.position;
    const out = names.map(() => []);
    const vertex = (i) => attrs.map((a) => Array.from({ length: a.itemSize }, (_, c) => a.getComponent(i, c)));
    const mix = (a, b, t) => a.map((v, k) => v.map((x, c) => x + (b[k][c] - x) * t));
    const push = (...vs) => vs.forEach((v) => v.forEach((x, k) => out[k].push(...x)));
    for (let t = 0; t < pos.count; t += 3) {
      const v = [vertex(t), vertex(t + 1), vertex(t + 2)];
      const d = [0, 1, 2].map((k) => f(pos.getX(t + k), pos.getY(t + k), pos.getZ(t + k)) - level);
      // The corner alone on its side of the level, if any.
      const lone = [0, 1, 2].find((k) => d[k] > 0 !== d[(k + 1) % 3] > 0 && d[k] > 0 !== d[(k + 2) % 3] > 0);
      if (lone === undefined) {
        push(...v);
        continue;
      }
      const [a, b, c] = [lone, (lone + 1) % 3, (lone + 2) % 3];
      const ab = mix(v[a], v[b], d[a] / (d[a] - d[b]));
      const ac = mix(v[a], v[c], d[a] / (d[a] - d[c]));
      push(v[a], ab, ac, ab, v[b], v[c], ab, v[c], ac);
    }
    const next = new THREE.BufferGeometry();
    names.forEach((k, i) => next.setAttribute(k, new THREE.BufferAttribute(new Float32Array(out[i]), attrs[i].itemSize)));
    src = next;
  }
  return src;
}

/** The views of a boxed model, in the atlas's cells (two across, four down, 512 × 256 each): the axis looked along
 *  (towards the model from the + side when `sign` is 1), and the axes that run across and down the cell. */
const VIEWS = [
  { axis: 0, sign: 1, across: 2, down: 1 },
  { axis: 0, sign: -1, across: 2, down: 1 },
  { axis: 1, sign: 1, across: 2, down: 0 },
  { axis: 1, sign: -1, across: 2, down: 0 },
  { axis: 2, sign: 1, across: 0, down: 1 },
  { axis: 2, sign: -1, across: 0, down: 1 }
];
const CELL = [512, 256];
const ATLAS = 1024;

/** A boxed model's look: its parts drawn flat along each axis into the atlas (see the head of this file). A vertex goes
 *  where the program will look it up: `(p - min) / size` across and down its view's cell, its depth along the view's
 *  axis, so the nearest surface along it keeps its texel. */
async function bakeBox(app, renderer, camera, name, list) {
  const box = new THREE.Box3();
  for (const p of list) {
    p.geo.computeBoundingBox();
    box.union(p.geo.boundingBox.clone().applyMatrix4(p.m));
  }
  const min = box.min.toArray();
  const size = box.getSize(new THREE.Vector3()).toArray().map((x) => Math.max(x, 1e-3));
  const draw = async (view, which, paint) => {
    const scene = new THREE.Scene();
    for (const part of list) {
      const geo = new THREE.BufferGeometry();
      for (const [k, a] of Object.entries(part.geo.attributes)) if (!a.isInstancedBufferAttribute) geo.setAttribute(k, a);
      if (part.geo.index) geo.setIndex(part.geo.index);
      const n = geo.attributes.position.count;
      geo.setAttribute('paintColor', new THREE.BufferAttribute(new Float32Array(n * 3).fill(paint), 3));
      geo.setAttribute('carState', new THREE.BufferAttribute(new Float32Array(n * 4), 4));
      geo.setAttribute('busState', new THREE.BufferAttribute(new Float32Array(n * 4), 4));
      const orig = part.mat;
      const m = new THREE.MeshBasicNodeMaterial();
      m.fog = false;
      m.side = THREE.DoubleSide;
      if (which === 'albedo') {
        if (orig.color) m.color.copy(orig.color);
        m.map = orig.map ?? null;
        m.colorNode = orig.colorNode ?? null;
      } else if (which === 'gloss') {
        m.colorNode = vec3(orig.roughnessNode ?? float(orig.roughness ?? 1), 0, 0);
      } else {
        m.color.copy(orig.emissive ? orig.emissive.clone().multiplyScalar(orig.emissiveIntensity ?? 1) : new THREE.Color(0, 0, 0));
        m.colorNode = orig.emissiveNode ?? null;
      }
      const q = uniform(part.m).mul(vec4(positionLocal, 1)).xyz.sub(uniform(new THREE.Vector3(...min))).div(uniform(new THREE.Vector3(...size)));
      const at = (k) => [q.x, q.y, q.z][k];
      const depth = view.sign > 0 ? float(1).sub(at(view.axis)) : at(view.axis);
      m.vertexNode = vec4(at(view.across).mul(2).sub(1), float(1).sub(at(view.down).mul(2)), depth.mul(0.98).add(0.01), 1);
      const mesh = new THREE.Mesh(geo, m);
      mesh.frustumCulled = false;
      scene.add(mesh);
    }
    const target = new THREE.RenderTarget(CELL[0], CELL[1], { type: THREE.FloatType, depthBuffer: true });
    renderer.setRenderTarget(target);
    renderer.setClearColor(0x000000, 0);
    renderer.clear();
    renderer.render(scene, camera);
    renderer.setRenderTarget(null);
    const px = await renderer.readRenderTargetPixelsAsync(target, 0, 0, CELL[0], CELL[1]);
    target.dispose();
    scene.traverse((o) => o.material?.dispose?.());
    return px;
  };
  const albedo = new Float32Array(ATLAS * ATLAS * 4);
  const glow = new Float32Array(ATLAS * ATLAS * 4);
  const shine = new Float32Array(ATLAS * ATLAS);
  const { lighting } = app;
  for (let k = 0; k < VIEWS.length; k++) {
    lighting.applyPreset('midday', true);
    const white = await draw(VIEWS[k], 'albedo', 1);
    const black = await draw(VIEWS[k], 'albedo', 0);
    const rough = await draw(VIEWS[k], 'gloss', 1);
    lighting.applyPreset('midnight', true);
    const lit = await draw(VIEWS[k], 'emission', 1);
    const [cx, cy] = [(k % 2) * CELL[0], Math.floor(k / 2) * CELL[1]];
    for (let y = 0; y < CELL[1]; y++) for (let x = 0; x < CELL[0]; x++) {
      const i = (y * CELL[0] + x) * 4, o = ((cy + y) * ATLAS + cx + x) * 4;
      if (white[i + 3] <= 0) continue;
      const lw = white[i] + white[i + 1] + white[i + 2], lb = black[i] + black[i + 1] + black[i + 2];
      for (let c = 0; c < 3; c++) {
        albedo[o + c] = white[i + c];
        glow[o + c] = Math.max(0, lit[i + c]);
      }
      albedo[o + 3] = lw > 1e-4 ? Math.min(1, Math.max(0, 1 - lb / lw)) : 0;
      glow[o + 3] = 1;
      shine[(cy + y) * ATLAS + cx + x] = Math.min(1, Math.max(0, (1 - rough[i]) / 0.85));
    }
  }
  // Texels no surface reached take their neighbours' (a mip or a filter would read black past an edge).
  const covered = new Uint8Array(ATLAS * ATLAS);
  for (let i = 0; i < covered.length; i++) covered[i] = glow[i * 4 + 3] > 0 ? 1 : 0;
  for (let pass = 0; pass < 12; pass++) {
    const next = covered.slice();
    for (let y = 0; y < ATLAS; y++) for (let x = 0; x < ATLAS; x++) {
      const i = y * ATLAS + x;
      if (covered[i]) continue;
      let n = 0;
      const sum = [0, 0, 0, 0, 0, 0, 0];
      for (const [dx, dy] of [[1, 0], [-1, 0], [0, 1], [0, -1]]) {
        const xx = x + dx, yy = y + dy;
        if (xx < 0 || yy < 0 || xx >= ATLAS || yy >= ATLAS || !covered[yy * ATLAS + xx]) continue;
        const j = (yy * ATLAS + xx) * 4;
        for (let c = 0; c < 4; c++) sum[c] += albedo[j + c];
        for (let c = 0; c < 3; c++) sum[4 + c] += glow[j + c];
        shine[i] = Math.max(shine[i], shine[yy * ATLAS + xx]);
        n++;
      }
      if (!n) continue;
      for (let c = 0; c < 4; c++) albedo[i * 4 + c] = sum[c] / n;
      for (let c = 0; c < 3; c++) glow[i * 4 + c] = sum[4 + c] / n;
      next[i] = 1;
    }
    covered.set(next);
  }
  let peak = 1e-3;
  for (let i = 0; i < ATLAS * ATLAS; i++) peak = Math.max(peak, glow[i * 4], glow[i * 4 + 1], glow[i * 4 + 2]);
  const scale = Math.max(1, peak);
  // (three pictures, each opaque: a canvas keeps colour premultiplied by alpha, so a mask in alpha loses the colour)
  const a8 = new Uint8ClampedArray(ATLAS * ATLAS * 4);
  const p8 = new Uint8ClampedArray(ATLAS * ATLAS * 4);
  const g8 = new Uint8ClampedArray(ATLAS * ATLAS * 4);
  const s8 = new Uint8ClampedArray(ATLAS * ATLAS * 4);
  let paintSum = 0;
  for (let i = 0; i < ATLAS * ATLAS; i++) {
    for (let c = 0; c < 3; c++) {
      a8[i * 4 + c] = toSrgb(albedo[i * 4 + c]);
      g8[i * 4 + c] = toSrgb(glow[i * 4 + c] / scale);
      p8[i * 4 + c] = Math.round(albedo[i * 4 + 3] * 255);
    }
    paintSum += albedo[i * 4 + 3];
    s8[i * 4] = s8[i * 4 + 1] = s8[i * 4 + 2] = Math.round(shine[i] * 255);
    a8[i * 4 + 3] = p8[i * 4 + 3] = g8[i * 4 + 3] = s8[i * 4 + 3] = 255;
  }
  const png = async (bytes) => {
    const c = new OffscreenCanvas(ATLAS, ATLAS);
    c.getContext('2d').putImageData(new ImageData(bytes, ATLAS, ATLAS), 0, 0);
    return c.convertToBlob({ type: 'image/png' });
  };
  await put(`bake/car_${name}.png`, await png(a8));
  await put(`bake/car_${name}_paint.png`, await png(p8));
  await put(`bake/car_${name}_glow.png`, await png(g8));
  await put(`bake/car_${name}_gloss.png`, await png(s8));
  log(`  ${name}: box ${min.map((x) => x.toFixed(2))} + ${size.map((x) => x.toFixed(2))}, ${((paintSum / (ATLAS * ATLAS)) * 100).toFixed(0)}% paint, glow over ${scale.toFixed(2)}`);
  return { file: `bake/car_${name}.png`, paint: `bake/car_${name}_paint.png`, glow: `bake/car_${name}_glow.png`, gloss: `bake/car_${name}_gloss.png`, glowScale: scale, min, size };
}

/** Each vertex's value of `graph` (a colour node, or a plain colour) with `paint`; `which` 'position' reads where the
 *  part's own position graph puts the vertex (a person posed by its rig), in the world. */
async function evaluate(renderer, camera, part, which, paint) {
  const geo = new THREE.BufferGeometry();
  for (const [k, a] of Object.entries(part.geo.attributes)) if (!a.isInstancedBufferAttribute) geo.setAttribute(k, a);
  if (part.geo.index) geo.setIndex(part.geo.index);
  const n = geo.attributes.position.count;
  // What the traffic gives each car: its paint, and its lamps (off).
  geo.setAttribute('paintColor', new THREE.BufferAttribute(new Float32Array(n * 3).fill(paint), 3));
  geo.setAttribute('carState', new THREE.BufferAttribute(new Float32Array(n * 4), 4));
  geo.setAttribute('busState', new THREE.BufferAttribute(new Float32Array(n * 4), 4));
  const orig = part.mat;
  const m = new THREE.MeshBasicNodeMaterial();
  m.fog = false;
  m.side = THREE.DoubleSide;
  if (which === 'position') {
    // (raised: what the material writes is clamped at zero)
    m.colorNode = positionWorld.add(LIFT);
  } else if (which === 'albedo') {
    if (orig.color) m.color.copy(orig.color);
    m.map = orig.map ?? null;
    m.vertexColors = !!orig.vertexColors;
    m.colorNode = orig.colorNode ?? null;
  } else {
    m.color.copy(orig.emissive ? orig.emissive.clone().multiplyScalar(orig.emissiveIntensity ?? 1) : new THREE.Color(0, 0, 0));
    m.colorNode = orig.emissiveNode ?? null;
  }
  m.positionNode = orig.positionNode ?? null;
  const p = float(vertexIndex);
  m.vertexNode = vec4(mod(p, W).add(0.5).div(W).mul(2).sub(1), float(1).sub(floor(p.div(W)).add(0.5).div(W).mul(2)), 0, 1);
  const mesh = new THREE.Mesh(geo, m);
  // (a person's mesh draws its people as instances; one is posed)
  if (part.count) mesh.count = part.count;
  mesh.isPoints = true;
  mesh.frustumCulled = false;
  const scene = new THREE.Scene();
  scene.add(mesh);
  const target = new THREE.RenderTarget(W, W, { type: THREE.FloatType, depthBuffer: false });
  renderer.setRenderTarget(target);
  renderer.setClearColor(0x000000, 0);
  renderer.clear();
  renderer.render(scene, camera);
  renderer.setRenderTarget(null);
  const px = await renderer.readRenderTargetPixelsAsync(target, 0, 0, W, Math.ceil(n / W));
  target.dispose();
  m.dispose();
  if (which === 'position') for (let i = 0; i < px.length; i++) if (i % 4 < 3) px[i] -= LIFT;
  return px;
}

/**
 * The people: the player on foot (the controllers' own PersonRig and PersonCrowd) walking and running on flat
 * ground, caught at eight points of the gait cycle each, and standing; the cyclist's rider (riderRig.ride) at eight
 * turns of the pedals. Each is its crowd mesh as its own position graph poses it, in the frame of the person (x
 * across, z ahead) or of the bicycle.
 */
async function people(app, renderer, camera) {
  const out = [];
  const c = app.controllers;
  const p = c.entities.person;
  if (p?.rig && p?.crowd) {
    const ground = (owner, x, z) => 0;
    // `far`: the crowd's far level of detail (PersonCrowd's second mesh, beyond its farDistance); `coat`: the coat
    // as paint (see exportModels), for a crowd whose people each wear their own.
    const pose = async (name, { far = false, coat = false } = {}) => {
      const mesh = p.crowd.meshes[far ? 1 : 0];
      const keep = p.crowd.farDistance;
      p.crowd.farDistance = far ? 0 : Infinity;
      p.crowd.begin();
      p.crowd.add(p.rig, far ? 1 : 0);
      p.crowd.end();
      p.crowd.farDistance = keep;
      const part = { model: name, geo: mesh.geometry, mat: mesh.material, count: 1, coat };
      const pos = await evaluate(renderer, camera, part, 'position', 1);
      // Into the person's frame: about its root, turned by its heading.
      const n = mesh.geometry.attributes.position.count;
      const local = new Float32Array(n * 3);
      const yaw = p.rig.yaw, s = Math.sin(-yaw), co = Math.cos(-yaw);
      for (let i = 0; i < n; i++) {
        const x = pos[i * 4] - p.rig.x, y = pos[i * 4 + 1], z = pos[i * 4 + 2] - p.rig.z;
        local[i * 3] = x * co + z * s;
        local[i * 3 + 1] = y;
        local[i * 3 + 2] = -x * s + z * co;
      }
      out.push({ model: name, geo: mesh.geometry, mat: mesh.material, count: 1, positions: local, coat });
    };
    for (const [gait, speed] of [['walk', 1.6], ['run', 5.5]]) {
      let x = 0, z = 0;
      p.rig.reset(x, z, 0, ground, null);
      const dt = 1 / 240;
      // Into a steady gait, then the eight points of one cycle.
      for (let k = 0; k < 240 * 4; k++) {
        z += speed * dt;
        p.rig.update(dt, x, z, 0, ground, null, 0);
      }
      for (let f = 0; f < 8; f++) {
        const want = f / 8;
        for (let k = 0; k < 2400; k++) {
          const before = p.rig.phase;
          z += speed * dt;
          p.rig.update(dt, x, z, 0, ground, null, 0);
          const d = (p.rig.phase - want + 1) % 1;
          if (d < (p.rig.phase - before + 1) % 1) break;
        }
        await pose(`person-${gait}-${f}`);
        // The sidewalks' crowd walks only: near and far, coats as paint.
        if (gait === 'walk') {
          await pose(`crowd-walk-${f}`, { coat: true });
          await pose(`crowd-far-walk-${f}`, { far: true, coat: true });
        }
      }
    }
    {
      let x = 0, z = 0;
      p.rig.reset(x, z, 0, ground, null);
      for (let k = 0; k < 480; k++) p.rig.update(1 / 240, x, z, 0, ground, null, 0);
      await pose('person-stand');
      await pose('crowd-stand', { coat: true });
      await pose('crowd-far-stand', { far: true, coat: true });
    }
  }
  const b = c.entities.cyclist;
  const root = app.scene.getObjectByName('PlayerCyclist');
  if (b?.riderRig && b?.riderCrowd && root) {
    root.updateMatrixWorld(true);
    const inv = root.matrixWorld.clone().invert();
    const mesh = b.riderCrowd.meshes[0];
    const v = new THREE.Vector3();
    for (let f = 0; f < 8; f++) {
      b.riderRig.ride(b.leanPivot, (f / 8) * Math.PI * 2, b.bikeFrame);
      b.riderCrowd.begin();
      b.riderCrowd.add(b.riderRig, 0);
      b.riderCrowd.end();
      const part = { model: `rider-${f}`, geo: mesh.geometry, mat: mesh.material, count: 1 };
      const pos = await evaluate(renderer, camera, part, 'position', 1);
      const n = mesh.geometry.attributes.position.count;
      const local = new Float32Array(n * 3);
      for (let i = 0; i < n; i++) v.set(pos[i * 4], pos[i * 4 + 1], pos[i * 4 + 2]).applyMatrix4(inv).toArray(local, i * 3);
      out.push({ model: `rider-${f}`, geo: mesh.geometry, mat: mesh.material, count: 1, positions: local });
    }
  }
  return out;
}

export async function exportModels(app) {
  const { renderer, lighting } = app;
  const camera = new THREE.PerspectiveCamera(60, 1, 0.1, 1000);
  camera.position.set(0, 3, 8);
  camera.lookAt(0, 0.5, 0);
  camera.updateMatrixWorld();
  const buffers = new Buffers();
  const models = new Map();
  const all = [...parts(app), ...(await people(app, renderer, camera))];
  const boxes = {};
  for (const name of new Set(all.filter((p) => p.boxed).map((p) => p.boxed))) {
    boxes[name] = await bakeBox(app, renderer, camera, name, all.filter((p) => p.boxed === name));
  }
  for (const part of all) {
    const g = part.geo;
    const n = g.attributes.position.count;
    lighting.applyPreset('midday', true);
    const white = await evaluate(renderer, camera, part, 'albedo', 1);
    const black = await evaluate(renderer, camera, part, 'albedo', 0);
    lighting.applyPreset('midnight', true);
    const glow = await evaluate(renderer, camera, part, 'emission', 1);
    const pos = new Float32Array(n * 3);
    const nrm = new Float32Array(n * 3);
    const v = new THREE.Vector3();
    const nm = new THREE.Matrix3().getNormalMatrix(part.m ?? new THREE.Matrix4());
    for (let i = 0; i < n; i++) {
      if (part.positions) {
        pos.set(part.positions.subarray(i * 3, i * 3 + 3), i * 3);
        continue;
      }
      v.fromBufferAttribute(g.attributes.position, i).applyMatrix4(part.m).toArray(pos, i * 3);
      if (g.attributes.normal) v.fromBufferAttribute(g.attributes.normal, i).applyMatrix3(nm).normalize().toArray(nrm, i * 3);
    }
    // Albedo with white paint (sRGB), how much of it the paint is, and the night's emission (RGBM, × 16). A crowd's
    // person (`coat`) has its coat and sleeves (pedestrians.js PART.BODY and PART.ARM) as white paint, which each
    // person's tint colours.
    const color = new Uint8Array(n * 4);
    const emit = new Uint8Array(n * 4);
    const partOf = part.coat && g.attributes.skin ? (i) => Math.round(g.attributes.skin.getW(i)) : null;
    for (let i = 0; i < n; i++) {
      const w = [white[i * 4], white[i * 4 + 1], white[i * 4 + 2]];
      const b = [black[i * 4], black[i * 4 + 1], black[i * 4 + 2]];
      const lw = w[0] + w[1] + w[2];
      let paint = lw > 1e-4 ? Math.min(1, Math.max(0, 1 - (b[0] + b[1] + b[2]) / lw)) : 0;
      const coat = partOf && (partOf(i) === 0 || partOf(i) === 3);
      if (coat) {
        paint = 1;
        w[0] = w[1] = w[2] = 1;
      }
      for (let k = 0; k < 3; k++) color[i * 4 + k] = toSrgb(w[k]);
      color[i * 4 + 3] = Math.round(paint * 254);
      const e = [glow[i * 4], glow[i * 4 + 1], glow[i * 4 + 2]].map((x) => Math.max(0, x));
      const top = Math.max(...e);
      const mm = Math.min(255, Math.max(1, Math.ceil((top / 16) * 255)));
      const s = 255 / ((mm / 255) * 16);
      for (let k = 0; k < 3; k++) emit[i * 4 + k] = Math.min(255, Math.round(e[k] * s));
      emit[i * 4 + 3] = top > 1 / 512 ? mm : 0;
    }
    const index = g.index ? Uint32Array.from(g.index.array) : Uint32Array.from({ length: n }, (_, i) => i);
    const list = models.get(part.model) ?? [];
    list.push({
      name: part.name ?? '',
      verts: n,
      position: buffers.add(pos),
      normal: buffers.add(nrm),
      color: buffers.add(color),
      glow: buffers.add(emit),
      index: buffers.add(index),
      unlit: part.mat.type === 'MeshBasicNodeMaterial',
      double: part.mat.side === THREE.DoubleSide,
      opacity: part.mat.transparent ? part.mat.opacity ?? 1 : 1,
      roughness: part.mat.roughness ?? 0.8,
      metalness: part.mat.metalness ?? 0,
      // A posed person's normals are the compiler's to work out from where its vertices are.
      posed: !!part.positions,
      boxed: part.boxed ?? null
    });
    models.set(part.model, list);
  }
  lighting.applyPreset('golden', true);
  await put('models.json', JSON.stringify({ models: [...models].map(([name, parts]) => ({ name, parts })), boxes }));
  await put('models.bin', buffers.blob());
  log(`models: ${models.size} models, ${[...models.values()].reduce((s, p) => s + p.length, 0)} parts`);
}
