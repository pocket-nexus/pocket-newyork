// What the interface shows of the reference's own HUD: its map of Manhattan, drawn by its own atlas
// (NYCMapSystem.drawWorld in nyc-map.js), and the names on its buttons (the modes and the hours of index.html, the
// players' labels of controllers.js, the hours' names of lighting.js, the top speeds of main.js). Writes `map.png`
// (grid north up, as the reference reads its map) and `interface.json`, which says where the picture lies in the
// city's metres.

import { log, put } from './util.js';

/** The square of city the picture shows: the island the reference builds (125th Street to the Battery), and a
 *  margin of river. */
const MARGIN = 250;
/** Pixels per metre of the picture; tools/ui.ts reduces it to the device's map. */
const SCALE = 0.12;

async function loadAtlas() {
  if (window.NYC.NYCMapSystem) return;
  await new Promise((resolve, reject) => {
    const s = document.createElement('script');
    s.src = '../js/nyc-map.js';
    s.onload = resolve;
    s.onerror = () => reject(new Error('nyc-map.js did not load'));
    document.head.appendChild(s);
  });
}

/** The atlas's ground (land, water, the island's streets, parks, blocks, landmark lots, highways, bridges). */
async function picture() {
  await loadAtlas();
  const island = window.NYC.ISLAND_BOUNDS;
  const x0 = Math.floor(island.minX - MARGIN), x1 = Math.ceil(island.maxX + MARGIN);
  const z0 = Math.floor(island.minZ - MARGIN), z1 = Math.ceil(island.maxZ + MARGIN);
  const w = Math.round((x1 - x0) * SCALE), h = Math.round((z1 - z0) * SCALE);
  const canvas = new OffscreenCanvas(w, h);
  const ctx = canvas.getContext('2d');
  // The atlas's own viewport class is module-private: this is its shape (centre, pixels per metre, size, ratio).
  const view = {
    cx: (x0 + x1) / 2, cz: (z0 + z1) / 2, s: SCALE, w, h, dpr: 1,
    minX: x0, maxX: x1, minZ: z0, maxZ: z1,
    world(c) {
      c.setTransform(this.s, 0, 0, this.s, this.w * 0.5 - this.cx * this.s, this.h * 0.5 - this.cz * this.s);
    },
    screen(c) {
      c.setTransform(1, 0, 0, 1, this.w * 0.5, this.h * 0.5);
    },
    toX(x) { return (x - this.cx) * this.s; },
    toY(z) { return (z - this.cz) * this.s; },
    sees(b) { return b && b.maxX >= this.minX && b.minX <= this.maxX && b.maxZ >= this.minZ && b.minZ <= this.maxZ; }
  };
  ctx.fillStyle = '#0a1118'; // the atlas's water
  ctx.fillRect(0, 0, w, h);
  window.NYC.NYCMapSystem.prototype.drawWorld.call({ searchPath: null }, ctx, view, false);
  const blob = await canvas.convertToBlob({ type: 'image/png' });
  return { blob, map: { x0, z0, metresPerPixel: 1 / SCALE, width: w, height: h } };
}

/** The names the reference's HUD shows, in ny_sim's orders (Mode: walk, drive, cycle, fly; Hour: golden, midday,
 *  twilight, midnight). */
async function names(app) {
  const text = async (path) => (await fetch(path)).text();
  const doc = new DOMParser().parseFromString(await text('../index.html'), 'text/html');
  const main = await text('../js/main.js');
  const lighting = await text('../js/lighting.js');
  const top = /topSpeedMph\s*=\s*\{([^}]*)\}/.exec(main);
  const mph = Object.fromEntries([...(top?.[1] ?? '').matchAll(/(\w+):\s*([\d.]+)/g)].map((m) => [m[1], Number(m[2])]));
  const entities = app.controllers.entities;
  const modes = ['person', 'car', 'cyclist', 'helicopter'].map((id) => {
    const button = doc.querySelector(`[data-mode="${id}"]`);
    return {
      id,
      name: button?.querySelector('.mode-label')?.textContent.trim() ?? id,
      label: entities[id]?.label ?? id,
      topMph: mph[id] ?? 0
    };
  });
  const presets = Object.fromEntries([...lighting.matchAll(/id:\s*'(\w+)',\s*name:\s*'([^']+)'/g)].map((m) => [m[1], m[2]]));
  const hours = ['golden', 'midday', 'twilight', 'midnight'].map((id) => {
    const button = doc.querySelector(`[data-time="${id}"]`);
    const full = presets[id] ?? id;
    const [name, time] = full.split(' · ');
    return { id, name, time: time ?? '', short: button?.querySelector('.seg-label')?.textContent.trim() ?? name };
  });
  return { modes, hours };
}

export async function exportMap(app) {
  const { blob, map } = await picture();
  await put('map.png', blob);
  const labels = await names(app);
  await put('interface.json', JSON.stringify({ map, ...labels }, null, 1));
  log(`map: ${map.width} × ${map.height} px at ${map.metresPerPixel.toFixed(1)} m a pixel; ${labels.modes.length} modes, ${labels.hours.length} hours`);
}
