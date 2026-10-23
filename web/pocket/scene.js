// The scene graph as data: every drawn object, its geometry, its world matrix, its material and its instances.
//
// Systems of the reference that stream instances in around the camera (building details within 560 m, the trees'
// levels of detail) are run at points across the city, and the instances they produce are collected without
// duplicates: the export records the union of what any camera position would see.
//
// Writes `scene.json` (tables) and `buffers.bin` (every array, 16-byte aligned, little-endian), referenced as
// { off, len, type } with `len` in elements.

import * as THREE from 'three/webgpu';
import { describeMaterial, log, put } from './util.js';

const TYPES = new Map([
  [Float32Array, 'f32'], [Uint32Array, 'u32'], [Uint16Array, 'u16'], [Int32Array, 'i32'], [Int16Array, 'i16'],
  [Uint8Array, 'u8'], [Int8Array, 'i8']
]);

export class Buffers {
  constructor() {
    this.parts = [];
    this.size = 0;
  }
  add(array) {
    const type = TYPES.get(array.constructor);
    if (!type) throw new Error(`unsupported array ${array.constructor.name}`);
    const pad = (16 - (this.size % 16)) % 16;
    if (pad) this.parts.push(new Uint8Array(pad));
    this.size += pad;
    const off = this.size;
    const bytes = new Uint8Array(array.buffer, array.byteOffset, array.byteLength).slice();
    this.parts.push(bytes);
    this.size += bytes.byteLength;
    return { off, len: array.length, type };
  }
  blob() {
    return new Blob(this.parts);
  }
}

// 64-bit FNV-1a over the bytes of a range, as two 32-bit halves: the key under which an instance is kept once.
function fnv(bytes, h1 = 0x811c9dc5, h2 = 0x01000193) {
  for (let i = 0; i < bytes.length; i++) {
    h1 = Math.imul(h1 ^ bytes[i], 0x01000193) >>> 0;
    h2 = Math.imul(h2 ^ bytes[i], 0x5bd1e995) >>> 0;
  }
  return [h1, h2];
}

function pathOf(o) {
  const path = [];
  for (let p = o; p; p = p.parent) if (p.name) path.unshift(p.name);
  return path.join('/');
}

function shownInTree(o) {
  for (let p = o; p; p = p.parent) if (!p.visible) return false;
  return true;
}

/** Per-instance data of an instanced mesh, gathered over every camera position: matrices, colours, attributes. */
class InstanceSet {
  constructor(mesh) {
    this.mesh = mesh;
    this.keys = new Set();
    this.matrices = [];
    this.colors = mesh.instanceColor ? [] : null;
    this.attrs = {};
    for (const [name, a] of Object.entries(mesh.geometry.attributes)) {
      if (a.isInstancedBufferAttribute) this.attrs[name] = { itemSize: a.itemSize, type: a.array.constructor, values: [] };
    }
  }
  capture() {
    const m = this.mesh;
    const mats = m.instanceMatrix.array;
    const geo = m.geometry.attributes;
    for (let i = 0; i < m.count; i++) {
      const mb = new Uint8Array(mats.buffer, mats.byteOffset + i * 64, 64);
      let [h1, h2] = fnv(mb);
      for (const name of Object.keys(this.attrs)) {
        const a = geo[name];
        const s = a.itemSize;
        const arr = a.array;
        [h1, h2] = fnv(new Uint8Array(arr.buffer, arr.byteOffset + i * s * arr.BYTES_PER_ELEMENT, s * arr.BYTES_PER_ELEMENT), h1, h2);
      }
      const key = `${h1}:${h2}`;
      if (this.keys.has(key)) continue;
      this.keys.add(key);
      for (let k = 0; k < 16; k++) this.matrices.push(mats[i * 16 + k]);
      if (this.colors) {
        const c = m.instanceColor.array;
        this.colors.push(c[i * 3], c[i * 3 + 1], c[i * 3 + 2]);
      }
      for (const [name, e] of Object.entries(this.attrs)) {
        const a = geo[name];
        for (let k = 0; k < a.itemSize; k++) e.values.push(a.array[i * a.itemSize + k]);
      }
    }
  }
  get count() {
    return this.matrices.length / 16;
  }
}

/**
 * Runs the reference's streaming at every point of a grid over the city: the building details (cityBlocks.update)
 * and whatever follows the view camera through scene.onBeforeRender (the trees' levels).
 */
function sweepStreams(app, sets, step) {
  const { scene, renderer, cityBlocks } = app;
  const box = new THREE.Box3();
  for (const blk of cityBlocks.allBlocks) box.expandByPoint(new THREE.Vector3(blk.cx, 0, blk.cz));
  const cam = new THREE.PerspectiveCamera(60, 1, 0.5, 5000);
  let n = 0;
  const t0 = performance.now();
  for (let z = box.min.z - step / 2; z <= box.max.z + step; z += step) {
    for (let x = box.min.x - step / 2; x <= box.max.x + step; x += step) {
      cityBlocks.update(x, z, true);
      cam.position.set(x, 2, z);
      cam.updateMatrixWorld();
      scene.onBeforeRender(renderer, scene, cam, null);
      for (const s of sets) s.capture();
      n++;
    }
  }
  log(`streams: ${n} points of a ${step} m grid in ${Math.round(performance.now() - t0)} ms`);
}

export async function exportScene(app, write = true) {
  const { scene } = app;
  const buffers = new Buffers();
  const geometries = new Map();
  const materials = new Map();
  const objects = [];
  const sets = [];

  const objs = [];
  scene.traverse((o) => {
    if (o.isMesh || o.isLine || o.isPoints || o.isSprite) objs.push(o);
  });
  for (const o of objs) if (o.isInstancedMesh) sets.push(new InstanceSet(o));
  for (const s of sets) s.capture();
  sweepStreams(app, sets, 400);
  const setOf = new Map(sets.map((s) => [s.mesh, s]));

  const geometryId = (g) => {
    let e = geometries.get(g);
    if (e) return e.id;
    const attributes = {};
    for (const [name, a] of Object.entries(g.attributes)) {
      if (a.isInstancedBufferAttribute) continue;
      const src = a.isInterleavedBufferAttribute ? Float32Array.from({ length: a.count * a.itemSize }, (_, i) => a.getComponent(Math.floor(i / a.itemSize), i % a.itemSize)) : a.array;
      attributes[name] = { itemSize: a.itemSize, normalized: a.normalized, ...buffers.add(src) };
    }
    e = {
      id: geometries.size,
      type: g.type,
      vertexCount: g.attributes.position?.count ?? 0,
      attributes,
      index: g.index ? buffers.add(g.index.array) : null,
      groups: g.groups.map((x) => ({ start: x.start, count: x.count, materialIndex: x.materialIndex ?? 0 })),
      drawRange: { start: g.drawRange.start, count: Number.isFinite(g.drawRange.count) ? g.drawRange.count : -1 },
      bounds: g.boundingBox ? [g.boundingBox.min.toArray(), g.boundingBox.max.toArray()] : null
    };
    geometries.set(g, e);
    return e.id;
  };
  const materialId = (m) => {
    let e = materials.get(m);
    if (e) return e.id;
    e = { id: materials.size, uuid: m.uuid, ...describeMaterial(m) };
    materials.set(m, e);
    return e.id;
  };

  for (const o of objs) {
    o.geometry.computeBoundingBox?.();
    const set = setOf.get(o);
    const entry = {
      id: objects.length,
      name: o.name,
      path: pathOf(o),
      type: o.type,
      geometry: geometryId(o.geometry),
      material: Array.isArray(o.material) ? o.material.map(materialId) : materialId(o.material),
      matrixWorld: o.matrixWorld.toArray(),
      shown: shownInTree(o),
      castShadow: o.castShadow,
      receiveShadow: o.receiveShadow,
      renderOrder: o.renderOrder,
      frustumCulled: o.frustumCulled,
      userData: Object.keys(o.userData)
    };
    if (set) {
      const inst = { count: set.count, matrix: buffers.add(new Float32Array(set.matrices)), attrs: {} };
      if (set.colors) inst.color = buffers.add(new Float32Array(set.colors));
      for (const [name, e] of Object.entries(set.attrs)) inst.attrs[name] = { itemSize: e.itemSize, ...buffers.add(new e.type(e.values)) };
      entry.instances = inst;
    }
    objects.push(entry);
  }

  const json = {
    version: 1,
    geometries: [...geometries.values()],
    materials: [...materials.values()],
    objects
  };
  if (write) {
    await put('scene.json', JSON.stringify(json));
    await put('buffers.bin', buffers.blob());
  }
  const inst = objects.reduce((s, o) => s + (o.instances?.count ?? 1), 0);
  log(`scene: ${objects.length} objects (${inst} instances), ${geometries.size} geometries, ${materials.size} materials, ${(buffers.size / 1e6).toFixed(1)} MB of arrays`);
  return { entries: objects, objects: objs, sets: setOf };
}
