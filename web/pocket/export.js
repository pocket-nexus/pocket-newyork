// SceneIR export. The page loads the reference's own scripts, builds Manhattan with its own builders and hands the
// compiler what they produce (tools/export.ts receives it). Nothing of the reference's look is written a second time:
// geometry is read from the scene graph, and a procedural material is drawn by the reference's own node graph into
// pictures (bake.js).
//
//   export.html?sink=http://127.0.0.1:5391[&only=survey|scene|bake|...]

import * as THREE from 'three/webgpu';
import * as TSL from 'three/tsl';
import { bloom } from 'three/addons/tsl/display/BloomNode.js';
import { ssr } from 'three/addons/tsl/display/SSRNode.js';
import { mergeGeometries, mergeVertices } from 'three/addons/utils/BufferGeometryUtils.js';
import { SkyMesh } from 'three/addons/objects/SkyMesh.js';
import { SunLight } from 'three/addons/lights/SunLight.js';
import { SunLightNode } from 'three/addons/lights/SunLightNode.js';
import { ClusteredLighting } from 'three/addons/lighting/ClusteredLighting.js';
import { describeMaterial, log, put } from './util.js';
import { exportScene } from './scene.js';
import { probe } from './probe.js';
import { exportWorld } from './world.js';
import { bake } from './bake.js';
import { exportSim } from './sim.js';

const params = new URLSearchParams(location.search);
const ONLY = params.get('only');

window.NYC = {
  THREE,
  TSL,
  PostFX: { bloom, ssr },
  Addons: { mergeGeometries, mergeVertices, SkyMesh, SunLight, SunLightNode, ClusteredLighting }
};

// The reference's scripts in index.html's order, without the page's own loop, HUD, map, culling and post-processing.
const SCRIPTS = [
  'nyc-geodata', 'city-config', 'tsl-materials', 'trees', 'actor-models', 'vehicle-lights', 'pedestrians', 'lighting',
  'city-grid', 'boroughs', 'buildings', 'landmarks', 'street-details', 'road-network', 'road-paint', 'traffic',
  'controllers', 'cameras'
];

// Every material factory of the reference (`create…Material`) is wrapped once it is defined, before the scripts that
// call it are loaded (they read window.NYC when they load): the material it returns carries its factory's name and
// arguments, which is how the compiler knows what drew a surface.
function plain(v, depth = 0) {
  if (v === null || typeof v !== 'object') return typeof v === 'function' ? '<fn>' : v;
  if (v.isColor) return { color: v.getHex() };
  if (v.isVector3) return [v.x, v.y, v.z];
  if (v.isTexture) return { texture: v.name || v.uuid };
  if (depth > 3) return '<deep>';
  if (Array.isArray(v)) return v.map((x) => plain(x, depth + 1));
  const o = {};
  for (const [k, x] of Object.entries(v)) o[k] = plain(x, depth + 1);
  return o;
}
function tagFactories() {
  for (const [name, fn] of Object.entries(window.NYC)) {
    if (typeof fn !== 'function' || fn.pocketTagged || !/^create\w*Material$/.test(name)) continue;
    const wrapped = function (...args) {
      const mat = fn.apply(this, args);
      if (mat && mat.isMaterial && !mat.userData.pocket) mat.userData.pocket = { factory: name, args: plain(args) };
      return mat;
    };
    wrapped.pocketTagged = true;
    window.NYC[name] = wrapped;
  }
}

async function loadScripts() {
  for (const name of SCRIPTS) {
    await new Promise((resolve, reject) => {
      const s = document.createElement('script');
      s.src = `../js/${name}.js`;
      s.async = false;
      s.onload = resolve;
      s.onerror = () => reject(new Error(`${name}.js did not load`));
      document.head.appendChild(s);
    });
    tagFactories();
  }
}

/** Builds the city as js/main.js does, steps 1 to 4. */
async function build() {
  const NYC = window.NYC;
  const renderer = new THREE.WebGPURenderer({ antialias: false, reversedDepthBuffer: false });
  renderer.setPixelRatio(1);
  renderer.setSize(512, 512);
  document.body.appendChild(renderer.domElement);
  await renderer.init();
  const scene = new THREE.Scene();
  const t0 = performance.now();
  const lighting = new NYC.LightingSystem(scene, renderer);
  const cityGrid = NYC.buildCityGrid(scene);
  const boroughs = NYC.buildBoroughs(scene, { sidewalks: cityGrid.sidewalks });
  const { colliders: landmarkColliders } = NYC.buildLandmarks(scene);
  const cityBlocks = NYC.buildCityBlocks(scene);
  const colliders = [...landmarkColliders, ...cityBlocks.colliders, ...boroughs.colliders, ...(cityGrid.colliders || [])];
  const streetDetails = NYC.buildStreetDetails(scene);
  const traffic = NYC.createTrafficSystem(scene, cityGrid.sidewalks, [
    ...cityGrid.footprints,
    ...streetDetails.footprints,
    ...colliders.filter((c) => !c.obb)
  ]);
  const highway = NYC.createHighwayTraffic(scene, cityGrid.highwayLanes);
  NYC.buildRoadPaint(scene, traffic.network);
  const controllers = new NYC.ThirdPersonControllerManager(scene, colliders);
  const cameras = new NYC.CameraSystem(renderer.domElement, controllers);
  scene.updateMatrixWorld(true);
  log(`built in ${Math.round(performance.now() - t0)} ms`);
  return { renderer, scene, lighting, cityGrid, boroughs, cityBlocks, colliders, streetDetails, traffic, highway, controllers, cameras };
}

async function survey(app) {
  const mats = new Map();
  const objects = [];
  app.scene.traverse((o) => {
    if (!o.isMesh && !o.isLine && !o.isPoints && !o.isSprite) return;
    const list = Array.isArray(o.material) ? o.material : [o.material];
    const g = o.geometry;
    const idx = g.index ? g.index.count : g.attributes.position?.count ?? 0;
    const tris = o.isMesh ? Math.floor(idx / 3) : 0;
    const n = o.isInstancedMesh ? o.count : 1;
    const path = [];
    for (let p = o; p; p = p.parent) if (p.name) path.unshift(p.name);
    for (const m of list) {
      let e = mats.get(m);
      if (!e) mats.set(m, (e = { ...describeMaterial(m), objects: 0, instances: 0, tris: 0, attrs: new Set(), paths: new Set() }));
      e.objects++;
      e.instances += n;
      e.tris += tris * n;
      for (const a of Object.keys(g.attributes)) e.attrs.add(a);
      if (e.paths.size < 6) e.paths.add(path.slice(0, 3).join('/') || o.type);
    }
    objects.push({
      path: path.join('/'), type: o.type, n, capacity: o.isInstancedMesh ? o.instanceMatrix.count : 1, tris,
      visible: o.visible, layers: o.layers.mask, userData: Object.keys(o.userData)
    });
  });
  const materials = [...mats.values()].map((e) => ({ ...e, attrs: [...e.attrs], paths: [...e.paths] })).sort((a, b) => b.tris - a.tris);
  await put('survey.json', JSON.stringify({ materials, objects }, null, 1));
  log(`survey: ${materials.length} materials, ${objects.length} objects`);
}

async function main() {
  await loadScripts();
  const app = await build();
  window.__POCKET_EXPORT = app;
  if (ONLY === 'survey') await survey(app);
  if (!ONLY || ONLY === 'world') await exportWorld(app);
  if (!ONLY || ONLY === 'bake') await bake(app);
  if (!ONLY || ONLY === 'map') await (await import('./map.js')).exportMap(app);
  if (!ONLY || ONLY === 'sim') await exportSim(app);
  if (!ONLY || ONLY === 'scene' || ONLY === 'probe') {
    const exported = await exportScene(app, ONLY !== 'probe');
    if (!ONLY || ONLY === 'probe') await probe(app, exported);
  }
  await put('done', '');
}

main().catch((e) => {
  log(`error: ${e.stack || e}`);
  console.error(e);
});
