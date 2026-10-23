// The things that move, as models in their own frames, coloured by their own materials.
//
// Traffic: the reference's own stand-in cars (buildCarProxyGeometry, which it draws past 30 m: about 250 triangles
// with the wheels built in) for the taxi, the sedan and the SUV, the sedan with the NYPD light bar, and the bus with
// its four wheels where placeWheels puts them. The player: the taxi, the bicycle and the helicopter as the
// controllers built them (the groups PlayerTaxiCar, PlayerCyclist, PlayerHelicopter), each part in the group's
// frame.
//
// Every vertex is drawn twice through its material's colour graph, with white paint and with black paint
// (`paintColor`), so the compiler knows how much of it is paint, and once through its emission graph by night.
// Writes `models.json` and `models.bin` (arrays as scene.js writes them).

import * as THREE from 'three/webgpu';
import { float, floor, mod, uniform, vec4, vertexIndex } from 'three/tsl';
import { Buffers } from './scene.js';
import { log, put } from './util.js';

const W = 1024;

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
  for (const kind of ['taxi', 'sedan', 'suv']) out.push({ model: kind, geo: NYC.buildCarProxyGeometry(S[kind]), mat: NYC.createLoftedCarMaterial(S[kind]), m: id });
  out.push({ model: 'nypd', geo: NYC.buildCarProxyGeometry(S.sedan), mat: NYC.createLoftedCarMaterial(S.sedan), m: id });
  out.push({ model: 'nypd', geo: NYC.buildLightbarGeometry(S.sedan), mat: NYC.createLightbarMaterial(), m: id });
  const B = NYC.BUS_SPEC;
  out.push({ model: 'bus', geo: NYC.buildLoftedBusBodyGeometry(), mat: NYC.createLoftedBusMaterial(), m: id });
  const wheelMat = NYC.createBusWheelMaterial();
  for (const side of [-1, 1]) {
    const geo = NYC.buildBusWheelGeo(side);
    for (const wz of [B.wheelZ, -B.wheelZ]) out.push({ model: 'bus', geo, mat: wheelMat, m: new THREE.Matrix4().makeTranslation(side * B.wheelX, B.wheelRadius, wz) });
  }
  // The player's vehicles, in the frame of their group.
  for (const [model, name] of [['player-car', 'PlayerTaxiCar'], ['player-cyclist', 'PlayerCyclist'], ['player-helicopter', 'PlayerHelicopter']]) {
    const root = app.scene.getObjectByName(name);
    if (!root) continue;
    root.updateMatrixWorld(true);
    const inv = root.matrixWorld.clone().invert();
    root.traverse((o) => {
      if (!o.isMesh || o.material?.userData?.pocket?.factory === 'createContactShadowMaterial') return;
      const mat = Array.isArray(o.material) ? o.material[0] : o.material;
      out.push({ model, geo: o.geometry, mat, m: inv.clone().multiply(o.matrixWorld), name: o.name });
    });
  }
  return out;
}

/** Each vertex's value of `graph` (a colour node, or a plain colour) with `paint`. */
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
  if (which === 'albedo') {
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
  return px;
}

export async function exportModels(app) {
  const { renderer, lighting } = app;
  const camera = new THREE.PerspectiveCamera(60, 1, 0.1, 1000);
  camera.position.set(0, 3, 8);
  camera.lookAt(0, 0.5, 0);
  camera.updateMatrixWorld();
  const buffers = new Buffers();
  const models = new Map();
  for (const part of parts(app)) {
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
    const nm = new THREE.Matrix3().getNormalMatrix(part.m);
    for (let i = 0; i < n; i++) {
      v.fromBufferAttribute(g.attributes.position, i).applyMatrix4(part.m).toArray(pos, i * 3);
      if (g.attributes.normal) v.fromBufferAttribute(g.attributes.normal, i).applyMatrix3(nm).normalize().toArray(nrm, i * 3);
    }
    // Albedo with white paint (sRGB), how much of it the paint is, and the night's emission (RGBM, × 16).
    const color = new Uint8Array(n * 4);
    const emit = new Uint8Array(n * 4);
    for (let i = 0; i < n; i++) {
      const w = [white[i * 4], white[i * 4 + 1], white[i * 4 + 2]];
      const b = [black[i * 4], black[i * 4 + 1], black[i * 4 + 2]];
      const lw = w[0] + w[1] + w[2];
      const paint = lw > 1e-4 ? Math.min(1, Math.max(0, 1 - (b[0] + b[1] + b[2]) / lw)) : 0;
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
      double: part.mat.side === THREE.DoubleSide
    });
    models.set(part.model, list);
  }
  lighting.applyPreset('golden', true);
  await put('models.json', JSON.stringify({ models: [...models].map(([name, parts]) => ({ name, parts })) }));
  await put('models.bin', buffers.blob());
  log(`models: ${models.size} models, ${[...models.values()].reduce((s, p) => s + p.length, 0)} parts`);
}
