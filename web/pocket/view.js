// A compiled pack drawn in a browser tab, for looking at what the compiler made without a console: the same
// tables and levels of detail a device reads, under the reference's light. Not a device: its programs are three.js
// node materials, and its choice of levels follows the device's in outline only.
//
//   view.html?pack=vita60/city.pack&eye=x,y,z&at=x,y,z&fov=56&hour=golden&near=250&mid=1500

import * as THREE from 'three/webgpu';
import { attribute, dot, float, max, mix, normalize, normalWorld, positionWorld, pow, step, texture, uniform, vec2, vec3, vec4 } from 'three/tsl';

const q = new URLSearchParams(location.search);
const info = document.getElementById('info');

const HOURS = {
  golden: { sunDir: [-0.44, 0.58, -0.68], sun: 0xffca92, sunI: 5.8, sky: 0x9abbe6, ground: 0xc88a62, hemiI: 1.48, fog: 0xdcc5ae, density: 0.00024, glow: 0.48, night: 0.0 },
  midday: { sunDir: [-0.34, 0.82, -0.46], sun: 0xfff5e8, sunI: 6.6, sky: 0xaad2ff, ground: 0xb89678, hemiI: 1.58, fog: 0xc6dcf2, density: 0.00018, glow: 0.24, night: 0.0 },
  twilight: { sunDir: [-0.52, 0.36, -0.77], sun: 0xff9466, sunI: 3.1, sky: 0x4c6ca8, ground: 0x6e5048, hemiI: 0.85, fog: 0x353c58, density: 0.00032, glow: 1.25, night: 0.28 },
  midnight: { sunDir: [0.42, 0.68, -0.6], sun: 0x8cb0e8, sunI: 1.45, sky: 0x263a62, ground: 0x382e36, hemiI: 0.72, fog: 0x0c1424, density: 0.00036, glow: 1.55, night: 0.86 }
};

function parse(buf) {
  const dv = new DataView(buf);
  if (dv.getUint32(0, true) !== 0x4b50594e) throw new Error('not a pack');
  const n = dv.getUint32(8, true);
  const sections = new Map();
  for (let i = 0; i < n; i++) {
    const at = 16 + i * 16;
    const tag = String.fromCharCode(...new Uint8Array(buf, at, 4));
    sections.set(tag, { off: dv.getUint32(at + 4, true), size: dv.getUint32(at + 8, true) });
  }
  const sec = (t) => sections.get(t);
  const f32 = (s, i) => dv.getFloat32(s.off + i * 4, true);
  const u32 = (s, i) => dv.getUint32(s.off + i * 4, true);
  const c = sec('CITY');
  const city = {
    x0: f32(c, 0), z0: f32(c, 1), cell: f32(c, 2), cellsX: u32(c, 3), cellsZ: u32(c, 4), blockCells: u32(c, 5), regionBlocks: u32(c, 6),
    yMin: f32(c, 7), yMax: f32(c, 8), view: [9, 10, 11, 12, 13, 14].map((i) => f32(c, i))
  };
  city.blocksX = Math.ceil(city.cellsX / city.blockCells);
  city.blocksZ = Math.ceil(city.cellsZ / city.blockCells);
  city.regionsX = Math.ceil(city.blocksX / city.regionBlocks);
  city.regionsZ = Math.ceil(city.blocksZ / city.regionBlocks);
  const places = (t) => {
    const s = sec(t);
    const out = [];
      const size = t === 'CELL' ? 40 : 32;
    for (let i = 0; i < s.size / size; i++) {
      const b = s.off + i * size;
      const e = { min: [0, 1, 2].map((k) => dv.getFloat32(b + k * 4, true)), max: [3, 4, 5].map((k) => dv.getFloat32(b + k * 4, true)), first: dv.getUint32(b + 24, true), count: dv.getUint32(b + 28, true) };
      if (t === 'CELL') e.detail = { first: dv.getUint32(b + 32, true), count: dv.getUint32(b + 36, true), min: e.min, max: e.max };
      out.push(e);
    }
    return out;
  };
  const bs = sec('BTCH');
  const batches = [];
  for (let i = 0; i < bs.size / 64; i++) {
    const b = bs.off + i * 64;
    const w = (k) => dv.getUint32(b + k * 4, true);
    const f = (k) => dv.getFloat32(b + k * 4, true);
    batches.push({ kind: w(0), tex: w(1), vtxFirst: w(2), vtxCount: w(3), idxFirst: w(4), idxCount: w(5), origin: [f(6), f(7), f(8)], span: [f(9), f(10), f(11)], instFirst: w(12), instCount: w(13), spans: w(14) });
  }
  const ts = sec('TEXS');
  const texs = [];
  for (let i = 0; ts && i < ts.size / 32; i++) {
    const b = ts.off + i * 32;
    const w = (k) => dv.getUint32(b + k * 4, true);
    texs.push({ format: w(0), width: w(1), height: w(2), mips: w(3), offset: w(4), size: w(5), night: w(6), metres: dv.getFloat32(b + 28, true) });
  }
  return { buf, dv, sec, texs, city, cells: places('CELL'), blocks: places('BLCK'), regions: places('REGN'), batches, meta: JSON.parse(new TextDecoder().decode(new Uint8Array(buf, sec('META').off, sec('META').size))) };
}

const hour = HOURS[q.get('hour') || 'golden'];
const U = {
  sunDir: uniform(new THREE.Vector3(...hour.sunDir).normalize()),
  sun: uniform(new THREE.Color(hour.sun).multiplyScalar(hour.sunI * 0.32)),
  sky: uniform(new THREE.Color(hour.sky).multiplyScalar(hour.hemiI * 0.6)),
  ground: uniform(new THREE.Color(hour.ground).multiplyScalar(hour.hemiI * 0.6)),
  glow: uniform(Math.max(0, (hour.glow - 0.3) / 1.25))
};

function solidMaterial(open, instanced = false) {
  const m = new THREE.MeshBasicNodeMaterial();
  const n = normalize(normalWorld);
  const col = attribute('col', 'vec4');
  const glw = attribute('glw', 'vec4');
  const albedo = instanced ? pow(col.rgb, vec3(2.2)).mul(attribute('itint', 'vec4').rgb.mul(2)) : pow(col.rgb, vec3(2.2));
  const lit = mix(U.ground, U.sky, n.y.mul(0.5).add(0.5)).add(U.sun.mul(max(open ? dot(n, U.sunDir).abs() : dot(n, U.sunDir), 0.0)));
  const always = step(0.99, col.a);
  m.colorNode = albedo.mul(lit).add(glw.rgb.mul(glw.a).mul(16).mul(mix(U.glow, float(1), always)));
  m.side = open ? THREE.DoubleSide : THREE.FrontSide;
  m.fog = true;
  return m;
}

/** Where an item's streams are: the pack's own sections, or its cell's record of NEAR. */
function streamsOf(pack, cell) {
  const { sec } = pack;
  if (cell === undefined || cell === 0xffffffff) return { sol: sec('VSOL').off, wal: sec('VWAL').off, gnd: sec('VGND').off, idx: sec('IDX0').off, inst: sec('INST').off };
  const nc = sec('NCEL').off + cell * 32;
  const dv = pack.dv;
  const offset = dv.getUint32(nc, true);
  const parts = [0, 1, 2, 3, 4].map((k) => dv.getUint32(nc + 8 + k * 4, true));
  const base = sec('NEAR').off + offset;
  const starts = [];
  let o = 0;
  for (let k = 0; k < 5; k++) {
    starts.push(base + o);
    o = (o + parts[k] + 15) & ~15;
  }
  return { sol: starts[0], wal: starts[1], gnd: starts[2], idx: starts[3], inst: starts[4] };
}

function indices(pack, st, b) {
  return new Uint16Array(pack.buf.slice(st.idx + b.idxFirst * 2, st.idx + (b.idxFirst + b.idxCount) * 2));
}

function groundGeometryOf(pack, b, st) {
  const { dv } = pack;
  const n = b.vtxCount;
  const pos = new Float32Array(n * 3), nrm = new Float32Array(n * 3), tint = new Uint8Array(n * 4);
  for (let i = 0; i < n; i++) {
    const at = st.gnd + (b.vtxFirst + i) * 16;
    for (let k = 0; k < 3; k++) {
      pos[i * 3 + k] = b.origin[k] + (dv.getUint16(at + k * 2, true) / 65535) * b.span[k];
      nrm[i * 3 + k] = dv.getInt8(at + 6 + k) / 127;
    }
    for (let k = 0; k < 4; k++) tint[i * 4 + k] = dv.getUint8(at + 10 + k);
  }
  const g = new THREE.BufferGeometry();
  g.setAttribute('position', new THREE.BufferAttribute(pos, 3));
  g.setAttribute('normal', new THREE.BufferAttribute(nrm, 3));
  g.setAttribute('tint', new THREE.BufferAttribute(tint, 4, true));
  g.setIndex(new THREE.BufferAttribute(indices(pack, st, b), 1));
  return g;
}

const groundMaterials = new Map();
function groundMaterial(pack, tex) {
  if (groundMaterials.has(tex)) return groundMaterials.get(tex);
  const m = new THREE.MeshBasicNodeMaterial();
  const n = normalize(normalWorld);
  const uv = positionWorld.xz.div(pack.texs[tex].metres);
  const lit = mix(U.ground, U.sky, n.y.mul(0.5).add(0.5)).add(U.sun.mul(max(dot(n, U.sunDir), 0.0)));
  m.colorNode = texture(textureOf(pack, tex), uv).rgb.mul(pow(attribute('tint', 'vec4').rgb, vec3(1)).mul(2)).mul(lit);
  m.fog = true;
  groundMaterials.set(tex, m);
  return m;
}

const leafMaterials = new Map();
function leafMaterial(pack, tex) {
  if (leafMaterials.has(tex)) return leafMaterials.get(tex);
  const m = new THREE.MeshBasicNodeMaterial();
  const n = normalize(normalWorld);
  const t = texture(textureOf(pack, tex), attribute('tuv', 'vec2'));
  const bark = attribute('tint', 'vec4');
  const leaf = bark.a;
  const col = mix(bark.rgb, t.rgb.mul(attribute('itint', 'vec4').rgb.mul(2)), leaf);
  const lit = mix(U.ground, U.sky, n.y.mul(0.5).add(0.5)).add(U.sun.mul(dot(n, U.sunDir).abs().mul(0.7).add(0.3)));
  m.colorNode = pow(col, vec3(2.2)).mul(lit);
  m.opacityNode = mix(float(1), t.a, leaf);
  m.alphaTest = 0.5;
  m.side = THREE.DoubleSide;
  m.fog = true;
  leafMaterials.set(tex, m);
  return m;
}

function geometryOf(pack, b, st = streamsOf(pack)) {
  const { dv } = pack;
  const stride = 20;
  const s = { off: st.sol };
  const n = b.vtxCount;
  const pos = new Float32Array(n * 3);
  const nrm = new Float32Array(n * 3);
  const col = new Uint8Array(n * 4);
  const glw = new Uint8Array(n * 4);
  for (let i = 0; i < n; i++) {
    const at = s.off + (b.vtxFirst + i) * stride;
    for (let k = 0; k < 3; k++) {
      pos[i * 3 + k] = b.origin[k] + (dv.getUint16(at + k * 2, true) / 65535) * b.span[k];
      nrm[i * 3 + k] = dv.getInt8(at + 6 + k) / 127;
    }
    for (let k = 0; k < 4; k++) {
      col[i * 4 + k] = dv.getUint8(at + 10 + k);
      glw[i * 4 + k] = dv.getUint8(at + 14 + k);
    }
  }
  const idx = indices(pack, st, b);
  const g = new THREE.BufferGeometry();
  g.setAttribute('position', new THREE.BufferAttribute(pos, 3));
  g.setAttribute('normal', new THREE.BufferAttribute(nrm, 3));
  g.setAttribute('col', new THREE.BufferAttribute(col, 4, true));
  g.setAttribute('glw', new THREE.BufferAttribute(glw, 4, true));
  g.setIndex(new THREE.BufferAttribute(idx, 1));
  g.computeBoundingSphere();
  return g;
}

// BC1 level 0 → RGBA.
function decodeBc1(bytes, w, h) {
  const out = new Uint8Array(w * h * 4);
  const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const c565 = (c) => [((c >> 11) & 31) * 255 / 31, ((c >> 5) & 63) * 255 / 63, (c & 31) * 255 / 31];
  let at = 0;
  for (let by = 0; by < h / 4; by++) {
    for (let bx = 0; bx < w / 4; bx++) {
      const a = dv.getUint16(at, true), b = dv.getUint16(at + 2, true), bits = dv.getUint32(at + 4, true);
      at += 8;
      const ca = c565(a), cb = c565(b);
      const pal = a > b
        ? [ca, cb, ca.map((v, k) => (2 * v + cb[k]) / 3), ca.map((v, k) => (v + 2 * cb[k]) / 3)]
        : [ca, cb, ca.map((v, k) => (v + cb[k]) / 2), [0, 0, 0]];
      for (let y = 0; y < 4; y++) for (let x = 0; x < 4; x++) {
        const c = pal[(bits >> (2 * (y * 4 + x))) & 3];
        const o = ((by * 4 + y) * w + bx * 4 + x) * 4;
        out[o] = c[0]; out[o + 1] = c[1]; out[o + 2] = c[2]; out[o + 3] = 255;
      }
    }
  }
  return out;
}

const textures = new Map();
function textureOf(pack, i) {
  if (textures.has(i)) return textures.get(i);
  const e = pack.texs[i];
  const td = pack.sec('TDAT');
  const rgba = decodeBc1(new Uint8Array(pack.buf, td.off + e.offset, e.width * e.height / 2), e.width, e.height);
  const t = new THREE.DataTexture(rgba, e.width, e.height, THREE.RGBAFormat);
  t.colorSpace = THREE.SRGBColorSpace;
  t.wrapS = t.wrapT = THREE.RepeatWrapping;
  t.magFilter = THREE.LinearFilter;
  t.minFilter = THREE.LinearMipmapLinearFilter;
  t.generateMipmaps = true;
  t.anisotropy = 4;
  t.flipY = false;
  t.needsUpdate = true;
  textures.set(i, t);
  return t;
}

const wallMaterials = new Map();
function wallMaterial(pack, tex) {
  if (wallMaterials.has(tex)) return wallMaterials.get(tex);
  const m = new THREE.MeshBasicNodeMaterial();
  const n = normalize(normalWorld);
  const uv = attribute('tuv', 'vec2');
  const tint = attribute('tint', 'vec4').rgb.mul(2);
  const lit = mix(U.ground, U.sky, n.y.mul(0.5).add(0.5)).add(U.sun.mul(max(dot(n, U.sunDir), 0.0)));
  let c = texture(textureOf(pack, tex), uv).rgb.mul(tint).mul(lit);
  const night = pack.texs[tex].night;
  if (night !== 0xffffffff) c = c.add(texture(textureOf(pack, night), uv).rgb.mul(pack.texs[night].metres).mul(U.glow));
  m.colorNode = c;
  m.fog = true;
  wallMaterials.set(tex, m);
  return m;
}

/** Kinds drawn at each of their instances (`ny_pack::kind::instanced`). */
const instanced = (kind) => kind === 4 || kind === 5 || kind === 6 || kind === 8;

const pictureMaterials = new Map();
/** A sign or a billboard: its picture over its scale, lit, or as it is where its vertices' tint alpha says so. */
function pictureMaterial(pack, tex) {
  if (pictureMaterials.has(tex)) return pictureMaterials.get(tex);
  const m = new THREE.MeshBasicNodeMaterial();
  const n = normalize(normalWorld);
  const lit = mix(U.ground, U.sky, n.y.mul(0.5).add(0.5)).add(U.sun.mul(max(dot(n, U.sunDir), 0.0)));
  const light = mix(lit, vec3(1), step(float(0.5), attribute('tint', 'vec4').a));
  m.colorNode = texture(textureOf(pack, tex), attribute('tuv', 'vec2')).rgb.mul(pack.texs[tex].metres).mul(light);
  m.side = THREE.DoubleSide;
  m.fog = true;
  pictureMaterials.set(tex, m);
  return m;
}

function wallGeometryOf(pack, b, st = streamsOf(pack)) {
  const { dv } = pack;
  const s = { off: st.wal };
  const n = b.vtxCount;
  const pos = new Float32Array(n * 3);
  const nrm = new Float32Array(n * 3);
  const tuv = new Float32Array(n * 2);
  const tint = new Uint8Array(n * 4);
  for (let i = 0; i < n; i++) {
    const at = s.off + (b.vtxFirst + i) * 20;
    for (let k = 0; k < 3; k++) {
      pos[i * 3 + k] = b.origin[k] + (dv.getUint16(at + k * 2, true) / 65535) * b.span[k];
      nrm[i * 3 + k] = dv.getInt8(at + 6 + k) / 127;
    }
    tuv[i * 2] = (dv.getInt16(at + 10, true) / 32767) * 64;
    tuv[i * 2 + 1] = (dv.getInt16(at + 12, true) / 32767) * 64;
    for (let k = 0; k < 4; k++) tint[i * 4 + k] = dv.getUint8(at + 14 + k);
  }
  const g = new THREE.BufferGeometry();
  g.setAttribute('position', new THREE.BufferAttribute(pos, 3));
  g.setAttribute('normal', new THREE.BufferAttribute(nrm, 3));
  g.setAttribute('tuv', new THREE.BufferAttribute(tuv, 2));
  g.setAttribute('tint', new THREE.BufferAttribute(tint, 4, true));
  g.setIndex(new THREE.BufferAttribute(indices(pack, st, b), 1));
  return g;
}

const dist = (p, min, max) => {
  const d = [0, 1, 2].map((k) => Math.max(min[k] - p[k], 0, p[k] - max[k]));
  return Math.hypot(d[0], d[1], d[2]);
};

async function main() {
  const name = q.get('pack') || 'vita60/city.pack';
  const pack = parse(await (await fetch(`/build/${name}`)).arrayBuffer());
  const { city } = pack;
  const renderer = new THREE.WebGPURenderer({ antialias: true });
  renderer.setSize(innerWidth, innerHeight);
  renderer.toneMapping = THREE.ACESFilmicToneMapping;
  document.body.appendChild(renderer.domElement);
  await renderer.init();
  const scene = new THREE.Scene();
  scene.background = new THREE.Color(hour.fog);
  scene.fog = new THREE.FogExp2(hour.fog, hour.density);
  const v = city.view;
  const eye = (q.get('eye') || '').split(',').map(Number);
  const at = (q.get('at') || '').split(',').map(Number);
  const camera = new THREE.PerspectiveCamera(Number(q.get('fov') || 56), innerWidth / innerHeight, 0.5, 30000);
  camera.position.fromArray(eye.length === 3 ? eye : v.slice(0, 3));
  camera.lookAt(new THREE.Vector3().fromArray(at.length === 3 ? at : v.slice(3, 6)));
  camera.updateMatrixWorld();
  const frustum = new THREE.Frustum().setFromProjectionMatrix(new THREE.Matrix4().multiplyMatrices(camera.projectionMatrix, camera.matrixWorldInverse));
  const reach = { detail: Number(q.get('detail') || 110), near: Number(q.get('near') || 260), mid: Number(q.get('mid') || 1400), far: Number(q.get('far') || 7000) };
  const p = camera.position.toArray();
  const box = new THREE.Box3();
  const seen = (min, max) => frustum.intersectsBox(box.set(new THREE.Vector3(...min), new THREE.Vector3(...max)));
  const items = [];
  const counts = [0, 0, 0, 0];
  const spans = new Uint32Array(pack.buf, pack.sec('SPAN').off, pack.sec('SPAN').size / 4);
  const whole = (b) => (instanced(b.kind) ? b.instCount : b.idxCount);
  const all = (first, count, lvl, cell = 0xffffffff) => {
    for (let i = first; i < first + count; i++) if (whole(pack.batches[i]) > 0) items.push({ batch: i, from: 0, to: whole(pack.batches[i]), cell, lvl });
  };
  const runs = (first, count, chosen, lvl) => {
    for (let i = first; i < first + count; i++) {
      const b = pack.batches[i];
      if (b.spans === 0xffffffff) continue;
      for (let k = 0; k < chosen.length;) {
        if (!chosen[k]) { k++; continue; }
        const s0 = k;
        while (k < chosen.length && chosen[k]) k++;
        const from = spans[b.spans + s0], to = spans[b.spans + k];
        if (to > from) items.push({ batch: i, from, to, lvl });
      }
    }
  };
  // ny-view's select, line for line.
  const { cell: cs, blockCells: bc, regionBlocks: rb } = city;
  const bw = cs * bc, rw = cs * bc * rb, y0 = city.yMin, y1 = city.yMax;
  for (let rz = 0; rz < city.regionsZ; rz++) for (let rx = 0; rx < city.regionsX; rx++) {
    const r = pack.regions[rz * city.regionsX + rx];
    const rmin = [city.x0 + rx * rw, y0, city.z0 + rz * rw], rmax = [rmin[0] + rw, y1, rmin[2] + rw];
    if (!seen(rmin, rmax)) continue;
    if (dist(p, rmin, rmax) >= reach.far) continue;
    if (dist(p, rmin, rmax) >= reach.mid) {
      if (r.count) { counts[2]++; all(r.first, r.count, 'far'); }
      continue;
    }
    const farBlocks = new Array(rb * rb).fill(false);
    let anyFar = false;
    for (let j = 0; j < rb; j++) for (let i = 0; i < rb; i++) {
      const bx = rx * rb + i, bz = rz * rb + j;
      if (bx >= city.blocksX || bz >= city.blocksZ) continue;
      const bmin = [city.x0 + bx * bw, y0, city.z0 + bz * bw], bmax = [bmin[0] + bw, y1, bmin[2] + bw];
      if (!seen(bmin, bmax)) continue;
      const d = dist(p, bmin, bmax);
      if (d >= reach.far) continue;
      if (d >= reach.mid) { farBlocks[j * rb + i] = true; anyFar = true; continue; }
      const block = pack.blocks[bz * city.blocksX + bx];
      if (d >= reach.near) {
        if (block.count) { counts[1]++; all(block.first, block.count, 'mid'); }
        continue;
      }
      const midCells = new Array(bc * bc).fill(false);
      let anyMid = false;
      for (let cj = 0; cj < bc; cj++) for (let ci = 0; ci < bc; ci++) {
        const cx = bx * bc + ci, cz = bz * bc + cj;
        if (cx >= city.cellsX || cz >= city.cellsZ) continue;
        const cmin = [city.x0 + cx * cs, y0, city.z0 + cz * cs], cmax = [cmin[0] + cs, y1, cmin[2] + cs];
        const dc = dist(p, cmin, cmax);
        if (dc >= reach.near) { midCells[cj * bc + ci] = true; anyMid = true; continue; }
        const cell = pack.cells[cz * city.cellsX + cx];
        if (cell.count + cell.detail.count === 0 || !seen(cell.min, cell.max)) continue;
        const cellIndex = cz * city.cellsX + cx;
        counts[0]++;
        all(cell.first, cell.count, 'near', cellIndex);
        if (dc < reach.detail && cell.detail.count) { counts[3]++; all(cell.detail.first, cell.detail.count, 'detail', cellIndex); }
      }
      if (anyMid && block.count) { counts[1]++; runs(block.first, block.count, midCells, 'mid'); }
    }
    if (anyFar && r.count) { counts[2]++; runs(r.first, r.count, farBlocks, 'far'); }
  }
  const mats = [solidMaterial(false), solidMaterial(true)];
  const instMats = [solidMaterial(false, true), solidMaterial(true, true)];
  const shapes = new Map();
  const geoms = new Map();
  const inst = pack.sec('INST');
  let tris = 0;
  const byKind = {};
  const byLevel = {};
  for (const it of items) {
    const b = pack.batches[it.batch];
    const n = instanced(b.kind) ? (b.idxCount / 3) * (it.to - it.from) : (it.to - it.from) / 3;
    byKind[b.kind] = (byKind[b.kind] ?? 0) + n;
    const lvl = `${it.lvl}:${b.kind}`;
    byLevel[lvl] = (byLevel[lvl] ?? 0) + n;
    let mesh;
    const st = streamsOf(pack, it.cell);
    if (instanced(b.kind)) {
      const key = `${b.kind}:${b.vtxFirst}:${b.idxFirst}`;
      if (!shapes.has(key)) shapes.set(key, b.kind === 6 || b.kind === 8 ? wallGeometryOf(pack, b) : geometryOf(pack, b));
      const g = shapes.get(key).clone();
      const n = it.to - it.from;
      const tint = new Uint8Array(n * 4);
      const im = new THREE.InstancedMesh(g, b.kind === 6 ? leafMaterial(pack, b.tex) : b.kind === 8 ? pictureMaterial(pack, b.tex) : instMats[b.kind === 5 ? 1 : 0], n);
      const mm = new THREE.Matrix4();
      for (let k = 0; k < n; k++) {
        const at = st.inst + (b.instFirst + it.from + k) * 52;
        const r = (row, col) => pack.dv.getFloat32(at + (row * 4 + col) * 4, true);
        mm.set(r(0, 0), r(0, 1), r(0, 2), r(0, 3), r(1, 0), r(1, 1), r(1, 2), r(1, 3), r(2, 0), r(2, 1), r(2, 2), r(2, 3), 0, 0, 0, 1);
        im.setMatrixAt(k, mm);
        for (let c = 0; c < 4; c++) tint[k * 4 + c] = pack.dv.getUint8(at + 48 + c);
      }
      g.setAttribute('itint', new THREE.InstancedBufferAttribute(tint, 4, true));
      mesh = im;
      tris += (b.idxCount / 3) * n;
    } else {
      if (!geoms.has(it.batch)) geoms.set(it.batch, b.kind === 0 || b.kind === 7 ? wallGeometryOf(pack, b, st) : b.kind === 3 ? groundGeometryOf(pack, b, st) : geometryOf(pack, b, st));
      const g0 = geoms.get(it.batch);
      if (!g0) continue;
      const g = g0.clone();
      g.setDrawRange(it.from, it.to - it.from);
      mesh = new THREE.Mesh(g, b.kind === 0 ? wallMaterial(pack, b.tex) : b.kind === 7 ? pictureMaterial(pack, b.tex) : b.kind === 3 ? groundMaterial(pack, b.tex) : mats[b.kind === 2 ? 1 : 0]);
      tris += (it.to - it.from) / 3;
    }
    mesh.frustumCulled = false;
    scene.add(mesh);
  }
  renderer.render(scene, camera);
  await renderer.backend.device?.queue.onSubmittedWorkDone();
  info.textContent = `${pack.meta.profile} · ${items.length} draws · ${Math.round(tris / 1000)}k triangles · places near ${counts[0]} mid ${counts[1]} far ${counts[2]} detail ${counts[3]}`;
  window.__VIEW = { ready: true, draws: items.length, tris, counts, byKind, byLevel };
}

main().catch((e) => {
  info.textContent = String(e.stack || e);
  window.__VIEW = { ready: true, error: String(e) };
});
