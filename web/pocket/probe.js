// The material of every vertex, as the reference's own node graph computes it there.
//
// Each object of scene.json is drawn again as points (its instances as exported, the union of every camera position),
// one pixel a vertex: point `instance × vertices + vertex` of a float target. The probe material is the object's own
// colour graph (colorNode, or color × map × vertex colours) and opacity, without light; a second pass draws its
// emission graph under the night preset. The compiler gives a surface without a picture of its own these colours.
//
// Writes `probe/<object>.bin`: per point 4 bytes of albedo (sRGB, alpha = opacity), then per point 4 bytes of
// emission (RGBM: rgb × m × 16, linear); `probe.json` says how many instances of each object were probed.

import * as THREE from 'three/webgpu';
import { float, floor, instanceIndex, mod, uniform, vec4, vertexIndex } from 'three/tsl';
import { log, put } from './util.js';

const W = 2048;
const H = 2048;
const CAP = W * H;

const toSrgb = (x) => {
  x = Math.min(1, Math.max(0, x));
  const s = x <= 0.0031308 ? x * 12.92 : 1.055 * Math.pow(x, 1 / 2.4) - 0.055;
  return Math.round(s * 255);
};

/** A material that writes `graph` at the pixel of each vertex. */
function probeMaterial(orig, which, uVerts, uFirst) {
  const m = new THREE.MeshBasicNodeMaterial();
  m.fog = false;
  m.transparent = false;
  m.side = THREE.DoubleSide;
  m.toneMapped = false;
  if (which === 'albedo') {
    if (orig.color) m.color.copy(orig.color);
    m.map = orig.map ?? null;
    m.vertexColors = !!orig.vertexColors;
    m.colorNode = orig.colorNode ?? null;
    m.opacity = orig.opacity ?? 1;
    m.opacityNode = orig.opacityNode ?? null;
  } else {
    const e = orig.emissive ? orig.emissive.clone().multiplyScalar(orig.emissiveIntensity ?? 1) : new THREE.Color(0, 0, 0);
    m.color.copy(e);
    m.map = orig.emissiveMap ?? null;
    m.colorNode = orig.emissiveNode ?? null;
    m.opacityNode = float(1);
  }
  m.positionNode = orig.positionNode ?? null;
  const p = float(instanceIndex).sub(uFirst).mul(uVerts).add(float(vertexIndex));
  const x = mod(p, W).add(0.5).div(W).mul(2).sub(1);
  const y = float(1).sub(floor(p.div(W)).add(0.5).div(H).mul(2));
  m.vertexNode = vec4(x, y, 0, 1);
  return m;
}

/** The object as the export recorded it: its own geometry, and the instances of scene.json. */
function rebuild(o, entry, set, material) {
  const src = o.geometry;
  const geo = new THREE.BufferGeometry();
  for (const [name, a] of Object.entries(src.attributes)) if (!a.isInstancedBufferAttribute) geo.setAttribute(name, a);
  if (src.index) geo.setIndex(src.index);
  let mesh;
  if (set) {
    for (const [name, e] of Object.entries(set.attrs)) geo.setAttribute(name, new THREE.InstancedBufferAttribute(new e.type(e.values), e.itemSize));
    mesh = new THREE.InstancedMesh(geo, material, set.count);
    mesh.instanceMatrix = new THREE.InstancedBufferAttribute(new Float32Array(set.matrices), 16);
    if (set.colors) mesh.instanceColor = new THREE.InstancedBufferAttribute(new Float32Array(set.colors), 3);
  } else {
    mesh = new THREE.Mesh(geo, material);
  }
  mesh.matrixAutoUpdate = false;
  mesh.matrix.fromArray(entry.matrixWorld);
  mesh.matrixWorld.fromArray(entry.matrixWorld);
  mesh.frustumCulled = false;
  mesh.isPoints = true; // one pixel a vertex (getPrimitiveTopology reads this before isMesh)
  return mesh;
}

export async function probe(app, exported) {
  const { renderer, lighting } = app;
  const target = new THREE.RenderTarget(W, H, { type: THREE.FloatType, depthBuffer: false });
  const camera = new THREE.PerspectiveCamera(60, 1, 0.5, 100000);
  camera.position.set(0, 40, 300);
  camera.lookAt(0, 20, 0);
  camera.updateMatrixWorld();
  const uVerts = uniform(0);
  const uFirst = uniform(0);
  const scene = new THREE.Scene();
  const t0 = performance.now();
  let points = 0;
  let objects = 0;
  const index = [];

  const { entries, sets } = exported;
  for (let i = 0; i < entries.length; i++) {
    const entry = entries[i];
    const o = exported.objects[i];
    if (!o.isMesh || o.isSprite) continue;
    const orig = Array.isArray(o.material) ? o.material[0] : o.material;
    const set = sets.get(o) ?? null;
    const verts = o.geometry.attributes.position.count;
    // An object of many instances is probed for as many of its first instances as fit 4 M points: the compiler
    // reads the rest's colour through their own attributes, or repeats these.
    const instances = set ? Math.min(set.count, Math.max(1, Math.floor(CAP / verts))) : 1;
    const total = verts * instances;
    if (total === 0) continue;
    const out = new Uint8Array(total * 8);
    for (const which of ['albedo', 'emission']) {
      lighting.applyPreset(which === 'albedo' ? 'midday' : 'midnight', true);
      const mat = probeMaterial(orig, which, uVerts, uFirst);
      const mesh = rebuild(o, entry, set, mat);
      scene.add(mesh);
      uVerts.value = verts;
      // Instances in runs that fit the target.
      const per = Math.max(1, Math.floor(CAP / verts));
      for (let first = 0; first < instances; first += per) {
        const n = Math.min(per, instances - first);
        uFirst.value = first;
        if (set) mesh.count = first + n;
        // (instances before `first` land at negative indices: off the target)
        renderer.setRenderTarget(target);
        renderer.setClearColor(0x000000, 0);
        renderer.clear();
        renderer.render(scene, camera);
        renderer.setRenderTarget(null);
        const rows = Math.ceil((n * verts) / W);
        const px = await renderer.readRenderTargetPixelsAsync(target, 0, 0, W, rows);
        const base = first * verts;
        for (let k = 0; k < n * verts; k++) {
          const r = px[k * 4], g = px[k * 4 + 1], b = px[k * 4 + 2], a = px[k * 4 + 3];
          const at = which === 'albedo' ? (base + k) * 4 : (total + base + k) * 4;
          if (which === 'albedo') {
            out[at] = toSrgb(r);
            out[at + 1] = toSrgb(g);
            out[at + 2] = toSrgb(b);
            out[at + 3] = Math.round(Math.min(1, Math.max(0, a)) * 255);
          } else {
            const top = Math.max(r, g, b, 0);
            const m = Math.min(255, Math.max(1, Math.ceil((top / 16) * 255)));
            const scale = 255 / ((m / 255) * 16);
            out[at] = Math.min(255, Math.round(Math.max(0, r) * scale));
            out[at + 1] = Math.min(255, Math.round(Math.max(0, g) * scale));
            out[at + 2] = Math.min(255, Math.round(Math.max(0, b) * scale));
            out[at + 3] = m;
          }
        }
      }
      scene.remove(mesh);
      mat.dispose();
      mesh.geometry.dispose?.();
    }
    await put(`probe/${entry.id}.bin`, out);
    index.push({ object: entry.id, verts, instances });
    points += total;
    objects++;
    if (objects % 50 === 0) log(`probe: ${objects} objects, ${(points / 1e6).toFixed(1)} M points`);
  }
  lighting.applyPreset('golden', true);
  target.dispose();
  await put('probe.json', JSON.stringify(index));
  log(`probe: ${objects} objects, ${(points / 1e6).toFixed(1)} M points in ${Math.round(performance.now() - t0)} ms`);
}
