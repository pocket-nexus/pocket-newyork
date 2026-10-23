// Pictures the reference's materials draw, for surfaces a device draws under a picture that repeats.
//
// Facades: a box-local facade material (createPreWarBuildingMaterial, createCurtainWallMaterial with boxLocal) lays
// a wall out in whole bays of `windowWidth` and storeys of `floorHeight` (facadeCell in tsl-materials.js). A page
// is that material drawn flat on a wall of eight bays, by the reference's own createFacadeBoxes, through its own
// colour graph (by day) and emission graph (by night, rooms at full glow): upper storeys of a street front and of a
// rear wall, the ground storey of a shop front, an entrance front and a rear wall, the windowless walls (lot line,
// stone), and the solid corner piers of a masonry wall. A device repeats a page along the wall and up it.
//
// Writes `bake/<name>.png` and `bake.json` (per page: material, wall kind, first storey, storeys, bays, metres).

import * as THREE from 'three/webgpu';
import { float } from 'three/tsl';
import { log, put } from './util.js';

const SS = 2; // samples per texel along each axis

/** The pages of one facade material. */
function pagesOf(m) {
  const a = m.userData.pocket.args[0] ?? {};
  const prewar = m.userData.pocket.factory === 'createPreWarBuildingMaterial';
  const W = a.windowWidth ?? (prewar ? 2.7 : 2.8);
  const F = a.floorHeight ?? (prewar ? 3.8 : 3.8);
  const P = prewar ? (a.cornerPier ?? 1.2) : 0;
  // Storeys a page holds: masonry has a cornice belt every 6 storeys, a curtain wall a plant storey every 16.
  const NF = prewar ? 12 : 16;
  const K = window.NYC.WALL_KINDS;
  const pages = [
    { page: 'up_front', kind: K.shop, base: 1, rows: NF, w: 512, h: 1024, night: true },
    { page: 'up_rear', kind: K.rear, base: 1, rows: NF, w: 512, h: 1024, night: true },
    { page: 'ground_shop', kind: K.shop, base: 0, rows: 1, w: 512, h: 128, night: true },
    { page: 'ground_entry', kind: K.entry, base: 0, rows: 1, w: 512, h: 128, night: true },
    { page: 'ground_rear', kind: K.rear, base: 0, rows: 1, w: 512, h: 128, night: true },
    { page: 'lot', kind: K.lotLine, base: 1, rows: NF, w: 256, h: 512, night: false },
    { page: 'stone', kind: K.blankStone, base: 1, rows: NF, w: 256, h: 512, night: false }
  ];
  if (P > 0) {
    pages.push({ page: 'pier_front', kind: K.shop, base: 1, rows: NF, w: 64, h: 1024, night: false, pier: true });
    pages.push({ page: 'pier_rear', kind: K.rear, base: 1, rows: NF, w: 64, h: 1024, night: false, pier: true });
  }
  return { W, F, P, NF, pages };
}

/** Linear float RGBA rows (top first) at SS × the size → sRGB bytes, box-filtered. */
function downsample(px, w, h, scale) {
  const out = new Uint8ClampedArray(w * h * 4);
  const W = w * SS;
  const srgb = (x) => {
    x = Math.min(1, Math.max(0, x / scale));
    return Math.round((x <= 0.0031308 ? x * 12.92 : 1.055 * Math.pow(x, 1 / 2.4) - 0.055) * 255);
  };
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      let r = 0, g = 0, b = 0;
      for (let j = 0; j < SS; j++) for (let i = 0; i < SS; i++) {
        const o = ((y * SS + j) * W + x * SS + i) * 4;
        r += px[o]; g += px[o + 1]; b += px[o + 2];
      }
      const n = SS * SS, o = (y * w + x) * 4;
      out[o] = srgb(r / n); out[o + 1] = srgb(g / n); out[o + 2] = srgb(b / n); out[o + 3] = 255;
    }
  }
  return out;
}

async function png(bytes, w, h) {
  const c = new OffscreenCanvas(w, h);
  c.getContext('2d').putImageData(new ImageData(bytes, w, h), 0, 0);
  return c.convertToBlob({ type: 'image/png' });
}

function peak(px) {
  let m = 0;
  for (let i = 0; i < px.length; i += 4) m = Math.max(m, px[i], px[i + 1], px[i + 2]);
  return m;
}

export async function bakeFacades(app) {
  const { renderer, scene: city, lighting } = app;
  const NYC = window.NYC;
  const mats = new Set();
  city.traverse((o) => {
    const m = o.material;
    if (o.isInstancedMesh && m && !Array.isArray(m) && /^create(PreWarBuilding|CurtainWall)Material$/.test(m.userData.pocket?.factory ?? '') && o.geometry.attributes.box) mats.add(m);
  });
  const uuidIds = new Map();
  let id = 0;
  city.traverse((o) => {
    for (const m of Array.isArray(o.material) ? o.material : o.material ? [o.material] : []) if (!uuidIds.has(m.uuid)) uuidIds.set(m.uuid, null);
  });
  const records = [];
  const t0 = performance.now();
  for (const m of mats) {
    const { W, F, P, NF, pages } = pagesOf(m);
    for (const pg of pages) {
      const bays = 8;
      const wall = 2 * P + bays * W;
      // Whole storeys and two to spare above the page, so no partial storey or coping reaches it.
      const storeys = pg.base + pg.rows + 2;
      const h = storeys * F;
      const d = 12;
      const mesh = NYC.createFacadeBoxes(m, [{ x: 0, y: h / 2, z: 0, w: wall, h, d, faces: [pg.kind, pg.kind, pg.kind, pg.kind], tone: 0.5 }]);
      const scene = new THREE.Scene();
      scene.add(mesh);
      // What is drawn: the bays between the piers, or the left pier (reading the +z wall from the front, x grows to the right).
      const x0 = pg.pier ? -wall / 2 : -wall / 2 + P;
      const x1 = pg.pier ? -wall / 2 + P : wall / 2 - P;
      const y0 = pg.base * F;
      const y1 = (pg.base + pg.rows) * F;
      // The eye far in front and below the page's middle, as someone in the street sees a wall: glass reflects
      // what is above. The projection is still a straight-on picture of the wall.
      const camY = (y0 + y1) / 2 - 600;
      const cam = new THREE.OrthographicCamera(x0, x1, y1 - camY, y0 - camY, 1, 5000);
      cam.position.set(0, camY, 2000);
      cam.lookAt(0, camY, 0);
      cam.updateMatrixWorld();
      const target = new THREE.RenderTarget(pg.w * SS, pg.h * SS, { type: THREE.FloatType, depthBuffer: true });
      const record = { material: m.uuid, factory: m.userData.pocket.factory, args: m.userData.pocket.args, page: pg.page, kind: pg.kind, base: pg.base, rows: pg.rows, bays: pg.pier ? 0 : bays, windowWidth: W, floorHeight: F, pier: P, width: pg.w, height: pg.h, metres: [x1 - x0, y1 - y0] };
      for (const pass of pg.night ? ['day', 'night'] : ['day']) {
        lighting.applyPreset(pass === 'day' ? 'midday' : 'midnight', true);
        if (pass === 'night') NYC.cityUniforms.uInteriorGlow.value = 1;
        const bm = new THREE.MeshBasicNodeMaterial();
        bm.fog = false;
        bm.colorNode = pass === 'day' ? m.colorNode : (m.emissiveNode ?? float(0));
        mesh.material = bm;
        renderer.setRenderTarget(target);
        renderer.setClearColor(0x000000, 1);
        renderer.clear();
        renderer.render(scene, cam);
        renderer.setRenderTarget(null);
        const px = await renderer.readRenderTargetPixelsAsync(target, 0, 0, pg.w * SS, pg.h * SS);
        const scale = pass === 'day' ? 1 : Math.min(8, Math.max(1, peak(px)));
        const name = `facade_${m.uuid.slice(0, 8)}_${pg.page}_${pass}`;
        await put(`bake/${name}.png`, await png(downsample(px, pg.w, pg.h, scale), pg.w, pg.h));
        record[pass] = { file: `bake/${name}.png`, scale };
        bm.dispose();
      }
      target.dispose();
      mesh.geometry.dispose();
      records.push(record);
    }
  }
  lighting.applyPreset('golden', true);
  log(`bake: ${records.length} facade pages of ${mats.size} materials in ${Math.round(performance.now() - t0)} ms`);
  return records;
}

export async function bake(app) {
  const facades = await bakeFacades(app);
  await put('bake.json', JSON.stringify({ facades }, null, 1));
}
