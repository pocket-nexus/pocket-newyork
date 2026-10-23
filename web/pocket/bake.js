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
import { log, materialOrder, put } from './util.js';

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

/**
 * Ground: a ground material (the sidewalk's flags, the roadway's asphalt and manholes, park paving, lawn) drawn from
 * straight above over a square whose side its pattern repeats in (the flags' 1.5 m, the manholes' 14 m cells),
 * by its own colour graph. What the pattern does not repeat (the noise of stains and wear) is folded across the
 * square's edges, so that the picture repeats without a seam.
 */
const GROUND = {
  createSidewalkMaterial: { metres: 12, size: 512, y: 0.22 },
  createRoadSurfaceMaterial: { metres: 28, size: 512, y: 0.0 },
  createParkPavingMaterial: { metres: 7.2, size: 256, y: 0.22 },
  createParkLawnMaterial: { metres: 16, size: 256, y: 0.25 }
};

export async function bakeGround(app) {
  const { renderer, scene: city, lighting } = app;
  const mats = new Map();
  city.traverse((o) => {
    for (const m of Array.isArray(o.material) ? o.material : o.material ? [o.material] : []) {
      const f = m.userData.pocket?.factory;
      if (f && GROUND[f] && !mats.has(f)) mats.set(f, m);
    }
  });
  const records = [];
  lighting.applyPreset('midday', true);
  for (const [factory, m] of mats) {
    const g = GROUND[factory];
    // A margin of a quarter of the square, folded back over its far edges.
    const M = g.metres / 4;
    const S = g.size;
    const MS = Math.round((S * M) / g.metres);
    const full = S + MS;
    const x0 = 0, z0 = 380;
    const plane = new THREE.PlaneGeometry(g.metres + M, g.metres + M);
    plane.rotateX(-Math.PI / 2);
    plane.translate(x0 + (g.metres + M) / 2, g.y, z0 + (g.metres + M) / 2);
    const bm = new THREE.MeshBasicNodeMaterial();
    bm.fog = false;
    bm.colorNode = m.colorNode;
    const mesh = new THREE.Mesh(plane, bm);
    const scene = new THREE.Scene();
    scene.add(mesh);
    // From straight above, close enough that the material draws its finest detail.
    const cam = new THREE.OrthographicCamera(0, g.metres + M, 0, -(g.metres + M), 0.5, 50);
    cam.position.set(x0, g.y + 6, z0);
    cam.up.set(0, 0, -1);
    cam.lookAt(x0, g.y, z0);
    cam.updateMatrixWorld();
    const target = new THREE.RenderTarget(full * SS, full * SS, { type: THREE.FloatType, depthBuffer: true });
    renderer.setRenderTarget(target);
    renderer.setClearColor(0x000000, 1);
    renderer.clear();
    renderer.render(scene, cam);
    renderer.setRenderTarget(null);
    const px = await renderer.readRenderTargetPixelsAsync(target, 0, 0, full * SS, full * SS);
    target.dispose();
    const big = downsample(px, full, full, 1);
    // Fold: the margin past the square fades over its start, along x then along z.
    const out = new Uint8ClampedArray(S * S * 4);
    const at = (x, y) => (y * full + x) * 4;
    const lerp = (a, b, t) => a + (b - a) * t;
    const row = new Float32Array(full * S * 4);
    for (let y = 0; y < full; y++) for (let x = 0; x < S; x++) for (let c = 0; c < 4; c++) {
      let v = big[at(x, y) + c];
      if (x < MS) v = lerp(big[at(x + S, y) + c], v, x / MS);
      if (y < S) row[(y * S + x) * 4 + c] = v;
      else row[(y * S + x) * 4 + c] = v;
    }
    const rowAt = (x, y) => (y * S + x) * 4;
    for (let y = 0; y < S; y++) for (let x = 0; x < S; x++) for (let c = 0; c < 4; c++) {
      let v = row[rowAt(x, y) + c];
      if (y < MS) v = lerp(row[rowAt(x, y + S) + c], v, y / MS);
      out[(y * S + x) * 4 + c] = c === 3 ? 255 : Math.round(v);
    }
    const name = `ground_${factory.replace(/^create|Material$/g, '').toLowerCase()}`;
    await put(`bake/${name}.png`, await png(out, S, S));
    records.push({ factory, args: m.userData.pocket.args, file: `bake/${name}.png`, metres: g.metres });
    bm.dispose();
    plane.dispose();
  }
  lighting.applyPreset('golden', true);
  log(`bake: ${records.length} ground pictures`);
  return records;
}

/**
 * Leaves: the trees' leaf atlas (createLeafAtlas, a picture of coverage, brightness and twigs) drawn through the tree
 * material's own colour graph, as a leaf with a white tint deep in no crown would be: what each card shows, by its
 * texture coordinates. A device multiplies it by a tree's tint and cuts the card where its alpha is under a half.
 */
export async function bakeLeaves(app) {
  const { renderer, scene: city, lighting } = app;
  let tree = null;
  city.traverse((o) => {
    if (!tree && o.material?.userData?.pocket?.factory === 'createTreeMaterial') tree = o.material;
  });
  if (!tree) return null;
  lighting.applyPreset('midday', true);
  const S = 1024;
  const quad = new THREE.PlaneGeometry(1, 1);
  quad.translate(0.5, 0.5, 0);
  const n = quad.attributes.position.count;
  quad.setAttribute('aTree', new THREE.BufferAttribute(new Float32Array(n * 4).map((_, i) => (i % 4 < 2 ? 1 : 0)), 4));
  quad.setAttribute('aTreeData', new THREE.BufferAttribute(new Float32Array(n * 4).map((_, i) => (i % 4 < 3 ? 1 : 1.5)), 4));
  const bm = new THREE.MeshBasicNodeMaterial();
  bm.fog = false;
  // (transparent, so the graph's alpha is what the target receives; nothing under it to blend with)
  bm.transparent = true;
  bm.blending = THREE.NoBlending;
  bm.colorNode = tree.colorNode;
  const scene = new THREE.Scene();
  scene.add(new THREE.Mesh(quad, bm));
  const cam = new THREE.OrthographicCamera(0, 1, 1, 0, 0.1, 10);
  cam.position.set(0, 0, 5);
  cam.updateMatrixWorld();
  const target = new THREE.RenderTarget(S, S, { type: THREE.FloatType, depthBuffer: false });
  renderer.setRenderTarget(target);
  renderer.setClearColor(0x000000, 0);
  renderer.clear();
  renderer.render(scene, cam);
  renderer.setRenderTarget(null);
  const px = await renderer.readRenderTargetPixelsAsync(target, 0, 0, S, S);
  target.dispose();
  // Rows top first: the top row is the atlas's v = 1.
  const out = new Uint8ClampedArray(S * S * 4);
  const srgb = (x) => Math.round((x <= 0.0031308 ? x * 12.92 : 1.055 * Math.pow(Math.min(1, Math.max(0, x)), 1 / 2.4) - 0.055) * 255);
  for (let i = 0; i < S * S; i++) {
    for (let c = 0; c < 3; c++) out[i * 4 + c] = srgb(px[i * 4 + c]);
    out[i * 4 + 3] = Math.round(Math.min(1, Math.max(0, px[i * 4 + 3])) * 255);
  }
  await put('bake/leaves.png', await png(out, S, S));
  bm.dispose();
  lighting.applyPreset('golden', true);
  log('bake: the leaves');
  return { file: 'bake/leaves.png', flipV: true };
}

/**
 * Pictures: a material that shows a picture over its texture coordinates (a sign, a billboard, a street-name blade,
 * a bus's destination) drawn through its own colour graph over its texture coordinates, 0 to 1 each way, as one
 * picture. Its look past the picture (an LED panel's scan lines, the border) comes with it; what moves in it (a
 * billboard's sweep) stands still.
 */
export async function bakePictures(app) {
  const { renderer, scene: city, lighting } = app;
  lighting.applyPreset('midday', true);
  const records = [];
  const order = materialOrder(city);
  for (let id = 0; id < order.length; id++) {
    const m = order[id];
    const billboard = m.userData.pocket?.factory === 'createBillboardMaterial';
    if (!m.map && !billboard) continue;
    const img = m.map?.image;
    const aspect = img && img.width && img.height ? img.width / img.height : billboard ? 2 : 1;
    const w = aspect >= 1 ? 512 : Math.max(64, 2 ** Math.round(Math.log2(512 * aspect)));
    const h = aspect >= 1 ? Math.max(64, 2 ** Math.round(Math.log2(512 / aspect))) : 512;
    const quad = new THREE.PlaneGeometry(1, 1);
    quad.translate(0.5, 0.5, 0);
    const bm = new THREE.MeshBasicNodeMaterial();
    bm.fog = false;
    if (m.color) bm.color.copy(m.color);
    bm.map = m.map ?? null;
    bm.colorNode = m.colorNode ?? null;
    const scene = new THREE.Scene();
    scene.add(new THREE.Mesh(quad, bm));
    const cam = new THREE.OrthographicCamera(0, 1, 1, 0, 0.1, 10);
    cam.position.set(0, 0, 5);
    cam.updateMatrixWorld();
    const target = new THREE.RenderTarget(w * SS, h * SS, { type: THREE.FloatType, depthBuffer: false });
    renderer.setRenderTarget(target);
    renderer.setClearColor(0x000000, 1);
    renderer.clear();
    renderer.render(scene, cam);
    renderer.setRenderTarget(null);
    const px = await renderer.readRenderTargetPixelsAsync(target, 0, 0, w * SS, h * SS);
    target.dispose();
    // Brighter than white (a lit poster) is kept over its scale.
    const scale = Math.max(1, Math.min(4, peak(px)));
    const name = `picture_${id}`;
    await put(`bake/${name}.png`, await png(downsample(px, w, h, scale), w, h));
    records.push({ material: id, type: m.type, factory: m.userData.pocket?.factory ?? null, file: `bake/${name}.png`, scale, unlit: m.type === 'MeshBasicNodeMaterial' });
    bm.dispose();
    quad.dispose();
  }
  lighting.applyPreset('golden', true);
  log(`bake: ${records.length} pictures`);
  return records;
}

export async function bake(app) {
  const facades = await bakeFacades(app);
  const ground = await bakeGround(app);
  const leaves = await bakeLeaves(app);
  const pictures = await bakePictures(app);
  await put('bake.json', JSON.stringify({ facades, ground, leaves, pictures }, null, 1));
}
