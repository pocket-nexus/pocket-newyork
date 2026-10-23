// A compiled pack drawn in a browser tab, for looking at what the compiler made without a console: the same
// tables and levels of detail a device reads, under the reference's light. Not a device: its programs are three.js
// node materials, and its choice of levels follows the device's in outline only.
//
//   view.html?pack=vita60/city.pack&eye=x,y,z&at=x,y,z&fov=56&hour=golden&near=250&mid=1500

import * as THREE from 'three/webgpu';
import { attribute, dot, float, max, mix, normalize, pow, step, uniform, vec3 } from 'three/tsl';

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
    for (let i = 0; i < s.size / 32; i++) {
      const b = s.off + i * 32;
      out.push({ min: [0, 1, 2].map((k) => dv.getFloat32(b + k * 4, true)), max: [3, 4, 5].map((k) => dv.getFloat32(b + k * 4, true)), first: dv.getUint32(b + 24, true), count: dv.getUint32(b + 28, true) });
    }
    return out;
  };
  const bs = sec('BTCH');
  const batches = [];
  for (let i = 0; i < bs.size / 48; i++) {
    const b = bs.off + i * 48;
    const w = (k) => dv.getUint32(b + k * 4, true);
    const f = (k) => dv.getFloat32(b + k * 4, true);
    batches.push({ kind: w(0), tex: w(1), vtxFirst: w(2), vtxCount: w(3), idxFirst: w(4), idxCount: w(5), origin: [f(6), f(7), f(8)], span: [f(9), f(10), f(11)] });
  }
  return { buf, dv, sec, city, cells: places('CELL'), blocks: places('BLCK'), regions: places('REGN'), batches, meta: JSON.parse(new TextDecoder().decode(new Uint8Array(buf, sec('META').off, sec('META').size))) };
}

const hour = HOURS[q.get('hour') || 'golden'];
const U = {
  sunDir: uniform(new THREE.Vector3(...hour.sunDir).normalize()),
  sun: uniform(new THREE.Color(hour.sun).multiplyScalar(hour.sunI * 0.32)),
  sky: uniform(new THREE.Color(hour.sky).multiplyScalar(hour.hemiI * 0.6)),
  ground: uniform(new THREE.Color(hour.ground).multiplyScalar(hour.hemiI * 0.6)),
  glow: uniform(Math.max(0, (hour.glow - 0.3) / 1.25))
};

function solidMaterial(open) {
  const m = new THREE.MeshBasicNodeMaterial();
  const n = normalize(attribute('nrm', 'vec3'));
  const col = attribute('col', 'vec4');
  const glw = attribute('glw', 'vec4');
  const albedo = pow(col.rgb, vec3(2.2));
  const lit = mix(U.ground, U.sky, n.y.mul(0.5).add(0.5)).add(U.sun.mul(max(open ? dot(n, U.sunDir).abs() : dot(n, U.sunDir), 0.0)));
  const always = step(0.99, col.a);
  m.colorNode = albedo.mul(lit).add(glw.rgb.mul(glw.a).mul(16).mul(mix(U.glow, float(1), always)));
  m.side = open ? THREE.DoubleSide : THREE.FrontSide;
  m.fog = true;
  return m;
}

function geometryOf(pack, b) {
  const { dv, sec } = pack;
  const stride = 20;
  const s = sec('VSOL');
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
  const is = sec('IDX0');
  const idx = new Uint16Array(pack.buf.slice(is.off + b.idxFirst * 2, is.off + (b.idxFirst + b.idxCount) * 2));
  const g = new THREE.BufferGeometry();
  g.setAttribute('position', new THREE.BufferAttribute(pos, 3));
  g.setAttribute('nrm', new THREE.BufferAttribute(nrm, 3));
  g.setAttribute('col', new THREE.BufferAttribute(col, 4, true));
  g.setAttribute('glw', new THREE.BufferAttribute(glw, 4, true));
  g.setIndex(new THREE.BufferAttribute(idx, 1));
  g.computeBoundingSphere();
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
  const NEAR = Number(q.get('near') || 250);
  const MID = Number(q.get('mid') || 1500);
  const p = camera.position.toArray();
  const box = new THREE.Box3();
  const seen = (min, max) => frustum.intersectsBox(box.set(new THREE.Vector3(...min), new THREE.Vector3(...max)));
  const chosen = [];
  const counts = [0, 0, 0];
  const take = (place, lod) => {
    if (!place.count || !seen(place.min, place.max)) return;
    counts[lod]++;
    for (let i = place.first; i < place.first + place.count; i++) chosen.push(i);
  };
  for (let rz = 0; rz < city.regionsZ; rz++) {
    for (let rx = 0; rx < city.regionsX; rx++) {
      const r = pack.regions[rz * city.regionsX + rx];
      const rmin = [city.x0 + rx * city.cell * city.blockCells * city.regionBlocks, -100, city.z0 + rz * city.cell * city.blockCells * city.regionBlocks];
      const rmax = [rmin[0] + city.cell * city.blockCells * city.regionBlocks, 600, rmin[2] + city.cell * city.blockCells * city.regionBlocks];
      if (dist(p, rmin, rmax) > MID) {
        take(r, 2);
        continue;
      }
      for (let bz = rz * city.regionBlocks; bz < Math.min(city.blocksZ, (rz + 1) * city.regionBlocks); bz++) {
        for (let bx = rx * city.regionBlocks; bx < Math.min(city.blocksX, (rx + 1) * city.regionBlocks); bx++) {
          const b = pack.blocks[bz * city.blocksX + bx];
          const bmin = [city.x0 + bx * city.cell * city.blockCells, -100, city.z0 + bz * city.cell * city.blockCells];
          const bmax = [bmin[0] + city.cell * city.blockCells, 600, bmin[2] + city.cell * city.blockCells];
          if (dist(p, bmin, bmax) > NEAR) {
            take(b, 1);
            continue;
          }
          for (let cz = bz * city.blockCells; cz < Math.min(city.cellsZ, (bz + 1) * city.blockCells); cz++) {
            for (let cx = bx * city.blockCells; cx < Math.min(city.cellsX, (bx + 1) * city.blockCells); cx++) take(pack.cells[cz * city.cellsX + cx], 0);
          }
        }
      }
    }
  }
  const mats = [solidMaterial(false), solidMaterial(true)];
  let tris = 0;
  for (const i of chosen) {
    const b = pack.batches[i];
    if (b.kind !== 1 && b.kind !== 2) continue;
    const mesh = new THREE.Mesh(geometryOf(pack, b), mats[b.kind === 2 ? 1 : 0]);
    mesh.frustumCulled = false;
    scene.add(mesh);
    tris += b.idxCount / 3;
  }
  renderer.render(scene, camera);
  await renderer.backend.device?.queue.onSubmittedWorkDone();
  info.textContent = `${pack.meta.profile} · ${chosen.length} batches · ${Math.round(tris / 1000)}k triangles · places near ${counts[0]} mid ${counts[1]} far ${counts[2]}`;
  window.__VIEW = { ready: true, batches: chosen.length, tris, counts };
}

main().catch((e) => {
  info.textContent = String(e.stack || e);
  window.__VIEW = { ready: true, error: String(e) };
});
