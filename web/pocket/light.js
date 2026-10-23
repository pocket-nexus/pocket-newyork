// How the reference lights and shows its hours, read back from it for programs that light in linear light and end
// with one tone curve:
//
// - The light: a white diffuse sphere (MeshStandard, roughness 1) drawn under each hour's lights and environment,
//   its linear radiance read back, and fitted as a hemisphere (from the sky, from the ground) plus the sun.
// - The sky: the reference's SkyMesh (its sun disc off) drawn in six directions and resampled by longitude (x
//   along 0..2π from +x towards +z) and latitude (y from straight up to straight down), then graded and tone-mapped
//   as its post-processing does (post-processing.js, then ACES, then sRGB): `sky/<hour>.png`, 256 × 128.
// - The haze: the sky just over the horizon (the reference's aerial fog takes the sky's radiance there) towards the
//   sun, across from it and away from it, graded and tone-mapped likewise.
// - The tone: per hour, the grade and ACES of a grey to the display's value, fitted as y = x(ax + b) / (x(cx + d) + 1)
//   (within 0.013 of it from 0.0003 to 40).
// - The environment: the sky's linear radiance a surface reflects (the scene's environment is the same sky, at its
//   environmentIntensity), around the horizon just over it and a third of the way up.

import * as THREE from 'three/webgpu';
import { log, put } from './util.js';

const HOURS = ['golden', 'midday', 'twilight', 'midnight'];
const SKY = [256, 128];

/** The reference's camera looks (post-processing.js `LOOKS`), read from its source. */
async function looks() {
  const src = await (await fetch('../js/post-processing.js')).text();
  const m = src.match(/const LOOKS = (\{[\s\S]*?\n\});/);
  if (!m) throw new Error('post-processing.js: no LOOKS');
  return Function(`return ${m[1]}`)();
}

const ACES_IN = [
  [0.59719, 0.35458, 0.04823],
  [0.076, 0.90834, 0.01566],
  [0.0284, 0.13383, 0.83777]
];
const ACES_OUT = [
  [1.60475, -0.53108, -0.07367],
  [-0.10208, 1.10813, -0.00605],
  [-0.00327, -0.07276, 1.07602]
];
const mat = (m, c) => m.map((r) => r[0] * c[0] + r[1] * c[1] + r[2] * c[2]);
const fit = (v) => (v * (v + 0.0245786) - 0.000090537) / (v * (0.983729 * v + 0.432951) + 0.238081);
const srgb = (x) => {
  x = Math.min(1, Math.max(0, x));
  return x <= 0.0031308 ? x * 12.92 : 1.055 * Math.pow(x, 1 / 2.4) - 0.055;
};

/** Linear scene colour → the display's (0..1), as post-processing.js and three's ACESFilmic do (no bloom, no
 *  vignette). */
function display(c, look) {
  let color = c.map((x, k) => Math.max(x, 0) * look.exposure * look.balance[k]);
  const lum = (v) => 0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2];
  const smooth = (a, b, x) => {
    const t = Math.min(1, Math.max(0, (x - a) / (b - a)));
    return t * t * (3 - 2 * t);
  };
  const luma = lum(color);
  const s = smooth(0.02, 0.6, luma), h = smooth(0.6, 3.0, luma);
  const shadows = [0.965, 1.005, 1.045].map((v) => v + (1 - v) * s);
  const highlights = [1.035, 1.005, 0.965].map((v) => 1 + (v - 1) * h);
  color = color.map((x, k) => x * shadows[k] * highlights[k]);
  color = color.map((x) => x * Math.pow(Math.max(luma, 0.0001) / 0.18, look.contrast - 1));
  const gray = lum(color);
  const hi = Math.max(...color), lo = Math.min(...color);
  const vibrance = 0.1 * (1 - (hi - lo) / Math.max(hi, 0.0001));
  color = color.map((x) => Math.max(gray + (x - gray) * (look.saturation + vibrance), 0));
  color = mat(ACES_OUT, mat(ACES_IN, color.map((x) => x / 0.6)).map(fit));
  return color.map(srgb);
}

/** A grey through the grade and ACES, as the display shows it. */
function grey(x, look) {
  return display([x, x, x], { ...look, balance: [1, 1, 1] })[1];
}

/** y = x(ax + b) / (x(cx + d) + 1) through `grey`, by Nelder-Mead from a few starts. */
function fitTone(look) {
  const xs = [];
  for (let e = -3.5; e <= 1.6; e += 0.025) xs.push(10 ** e);
  const want = xs.map((x) => grey(x, look));
  const err = (p) => {
    let s = 0;
    for (let i = 0; i < xs.length; i++) {
      const x = xs[i];
      const den = x * (p[2] * x + p[3]) + 1;
      if (den <= 0) return 1e9;
      const y = (x * (p[0] * x + p[1])) / den;
      s += (Math.max(y, 0) - want[i]) ** 2;
    }
    return s;
  };
  let best = null;
  for (const start of [[3, 3, 3, 3], [10, 5, 10, 8], [1, 10, 1, 10], [20, 20, 20, 20]]) {
    const p = simplexFit(err, start);
    if (!best || err(p) < err(best)) best = p;
  }
  const worst = Math.max(...xs.map((x, i) => Math.abs(Math.max((x * (best[0] * x + best[1])) / (x * (best[2] * x + best[3]) + 1), 0) - want[i])));
  return { tone: best, worst };
}

/** Nelder-Mead from `start`. */
function simplexFit(err, start) {
  let simplex = [start];
  for (let k = 0; k < 4; k++) simplex.push(simplex[0].map((v, i) => (i === k ? v * 1.5 + 0.1 : v)));
  let vals = simplex.map(err);
  for (let it = 0; it < 20000; it++) {
    const order = vals.map((v, i) => i).sort((a, b) => vals[a] - vals[b]);
    simplex = order.map((i) => simplex[i]);
    vals = order.map((i) => vals[i]);
    const c = [0, 1, 2, 3].map((k) => simplex.slice(0, 4).reduce((s, p) => s + p[k], 0) / 4);
    const at = (t) => c.map((v, k) => v + t * (simplex[4][k] - v));
    const r = at(-1), fr = err(r);
    if (fr < vals[0]) {
      const e = at(-2), fe = err(e);
      [simplex[4], vals[4]] = fe < fr ? [e, fe] : [r, fr];
    } else if (fr < vals[3]) {
      [simplex[4], vals[4]] = [r, fr];
    } else {
      const k = at(0.5), fk = err(k);
      if (fk < vals[4]) [simplex[4], vals[4]] = [k, fk];
      else for (let i = 1; i < 5; i++) {
        simplex[i] = simplex[i].map((v, j) => simplex[0][j] + 0.5 * (v - simplex[0][j]));
        vals[i] = err(simplex[i]);
      }
    }
  }
  return simplex[0];
}

/** Whether a render target's rows read back from the top of the picture. */
async function rowsFromTop(renderer) {
  const scene = new THREE.Scene();
  const quad = new THREE.Mesh(new THREE.PlaneGeometry(2, 1), new THREE.MeshBasicNodeMaterial({ color: 0xffffff }));
  quad.position.y = 0.5;
  scene.add(quad);
  const cam = new THREE.OrthographicCamera(-1, 1, 1, -1, 0.1, 10);
  cam.position.set(0, 0, 5);
  cam.updateMatrixWorld();
  const target = new THREE.RenderTarget(8, 8, { type: THREE.FloatType, depthBuffer: false });
  renderer.setRenderTarget(target);
  renderer.setClearColor(0x000000, 1);
  renderer.clear();
  renderer.render(scene, cam);
  renderer.setRenderTarget(null);
  const px = await renderer.readRenderTargetPixelsAsync(target, 0, 0, 8, 8);
  target.dispose();
  return px[0] > 0.5;
}

/** Linear radiance of a white diffuse sphere under the hour, fitted: hemisphere (sky, ground) and sun. */
async function lightOf(app, top) {
  const { renderer, scene: city, lighting } = app;
  const N = 64;
  // The city's lights (without their shadows) and environment, around the sphere alone.
  const scene = new THREE.Scene();
  scene.environment = city.environment;
  scene.environmentIntensity = city.environmentIntensity;
  const lights = [];
  city.traverse((o) => o.isLight && lights.push(o));
  for (const o of lights) {
    const c = o.clone();
    c.castShadow = false;
    c.position.copy(o.getWorldPosition(new THREE.Vector3()));
    scene.add(c);
    if (o.target) {
      c.target = new THREE.Object3D();
      c.target.position.copy(o.target.getWorldPosition(new THREE.Vector3()));
      scene.add(c.target);
    }
  }
  const ball = new THREE.Mesh(new THREE.SphereGeometry(1, 96, 48), new THREE.MeshStandardNodeMaterial({ color: 0xffffff, roughness: 1, metalness: 0 }));
  ball.position.set(0, 3000, 0);
  scene.add(ball);
  scene.updateMatrixWorld(true);
  const rows = [];
  const sunL = lighting.sunLight;
  const l = sunL.position.clone().sub(sunL.target?.position ?? new THREE.Vector3()).normalize();
  for (const side of [1, -1]) {
    const cam = new THREE.OrthographicCamera(-1.05, 1.05, 1.05, -1.05, 0.1, 20);
    cam.position.set(0, 3000, 5 * side);
    cam.lookAt(0, 3000, 0);
    cam.updateMatrixWorld();
    const target = new THREE.RenderTarget(N, N, { type: THREE.FloatType, depthBuffer: true });
    renderer.setRenderTarget(target);
    renderer.render(scene, cam);
    renderer.setRenderTarget(null);
    const px = await renderer.readRenderTargetPixelsAsync(target, 0, 0, N, N);
    target.dispose();
    for (let j = 0; j < N; j++) for (let i = 0; i < N; i++) {
      const x = ((i + 0.5) / N) * 2.1 - 1.05;
      const yy = ((j + 0.5) / N) * 2.1 - 1.05;
      const y = top ? -yy : yy;
      const r2 = x * x + y * y;
      if (r2 > 0.85) continue;
      // The camera's right is +x when it looks down −z, −x when it looks down +z.
      const n = [x * side, y, Math.sqrt(1 - r2) * side];
      const o = (j * N + i) * 4;
      const w = n[1] * 0.5 + 0.5;
      const c = Math.max(0, n[0] * l.x + n[1] * l.y + n[2] * l.z);
      rows.push({ f: [1 - w, w, c], v: [px[o], px[o + 1], px[o + 2]] });
    }
  }
  ball.geometry.dispose();
  ball.material.dispose();
  // Least squares per channel over [ground, sky, sun].
  const solve = (k) => {
    const A = [[0, 0, 0], [0, 0, 0], [0, 0, 0]], b = [0, 0, 0];
    for (const r of rows) for (let a = 0; a < 3; a++) {
      b[a] += r.f[a] * r.v[k];
      for (let c = 0; c < 3; c++) A[a][c] += r.f[a] * r.f[c];
    }
    // Gauss-Jordan.
    for (let p = 0; p < 3; p++) {
      const d = A[p][p];
      for (let c = 0; c < 3; c++) A[p][c] /= d;
      b[p] /= d;
      for (let q = 0; q < 3; q++) if (q !== p) {
        const m = A[q][p];
        for (let c = 0; c < 3; c++) A[q][c] -= m * A[p][c];
        b[q] -= m * b[p];
      }
    }
    return b;
  };
  const per = [0, 1, 2].map(solve);
  const residual = Math.sqrt(rows.reduce((s, r) => s + [0, 1, 2].reduce((t, k) => t + (r.f[0] * per[k][0] + r.f[1] * per[k][1] + r.f[2] * per[k][2] - r.v[k]) ** 2, 0), 0) / (rows.length * 3));
  return { ground: per.map((p) => p[0]), sky: per.map((p) => p[1]), sun: per.map((p) => p[2]), residual };
}

/** The sky's linear radiance by direction: six views from the origin, sampled through their own projections. */
async function skyOf(app, top) {
  const { renderer, scene, lighting } = app;
  const sky = lighting.sky;
  const parent = sky.parent;
  const disc = sky.showSunDisc.value;
  sky.showSunDisc.value = 0;
  const only = new THREE.Scene();
  only.add(sky);
  const N = 128;
  const views = [];
  const dirs = [[1, 0, 0], [-1, 0, 0], [0, 1, 0], [0, -1, 0], [0, 0, 1], [0, 0, -1]];
  for (const d of dirs) {
    const cam = new THREE.PerspectiveCamera(92, 1, 1, 400000);
    cam.position.set(0, 0, 0);
    cam.up.set(...(Math.abs(d[1]) > 0.5 ? [0, 0, 1] : [0, 1, 0]));
    cam.lookAt(d[0], d[1], d[2]);
    cam.updateMatrixWorld();
    cam.updateProjectionMatrix();
    const target = new THREE.RenderTarget(N, N, { type: THREE.FloatType, depthBuffer: true });
    renderer.setRenderTarget(target);
    renderer.render(only, cam);
    renderer.setRenderTarget(null);
    const px = await renderer.readRenderTargetPixelsAsync(target, 0, 0, N, N);
    target.dispose();
    views.push({ d, cam, px, vp: new THREE.Matrix4().multiplyMatrices(cam.projectionMatrix, cam.matrixWorldInverse) });
  }
  sky.showSunDisc.value = disc;
  parent.add(sky);
  const v = new THREE.Vector4();
  return (dir) => {
    // The view that looks most along `dir`.
    let best = views[0], m = -2;
    for (const w of views) {
      const dot = w.d[0] * dir[0] + w.d[1] * dir[1] + w.d[2] * dir[2];
      if (dot > m) [m, best] = [dot, w];
    }
    v.set(dir[0] * 1000, dir[1] * 1000, dir[2] * 1000, 1).applyMatrix4(best.vp);
    const x = Math.min(N - 1, Math.max(0, Math.floor(((v.x / v.w) * 0.5 + 0.5) * N)));
    const yy = Math.min(N - 1, Math.max(0, Math.floor(((v.y / v.w) * 0.5 + 0.5) * N)));
    const y = top ? N - 1 - yy : yy;
    const o = (y * N + x) * 4;
    return [best.px[o], best.px[o + 1], best.px[o + 2]];
  };
}

/** The direction of the panorama at (u, v), each 0..1. */
function skyDir(u, v) {
  const lon = u * 2 * Math.PI, lat = (0.5 - v) * Math.PI;
  return [Math.cos(lat) * Math.cos(lon), Math.sin(lat), Math.cos(lat) * Math.sin(lon)];
}

async function pngOf(bytes, w, h) {
  const c = new OffscreenCanvas(w, h);
  c.getContext('2d').putImageData(new ImageData(bytes, w, h), 0, 0);
  return c.convertToBlob({ type: 'image/png' });
}

/** Per hour: `{ light, haze, tone, sky }` (see the head of this file). */
export async function readLight(app) {
  const { renderer, lighting } = app;
  const LOOKS = await looks();
  const top = await rowsFromTop(renderer);
  const out = {};
  for (const id of HOURS) {
    lighting.applyPreset(id, true);
    const look = LOOKS[id];
    const light = await lightOf(app, top);
    const radiance = await skyOf(app, top);
    // The sky, graded, by longitude and latitude.
    const [W, H] = SKY;
    const bytes = new Uint8ClampedArray(W * H * 4);
    for (let t = 0; t < H; t++) for (let s = 0; s < W; s++) {
      const c = display(radiance(skyDir((s + 0.5) / W, (t + 0.5) / H)), look);
      const o = (t * W + s) * 4;
      bytes[o] = c[0] * 255; bytes[o + 1] = c[1] * 255; bytes[o + 2] = c[2] * 255; bytes[o + 3] = 255;
    }
    await put(`sky/${id}.png`, await pngOf(bytes, W, H));
    // The haze at the horizon (as the reference's fog takes it, 0.015 up), blurred over a few directions.
    const sun = lighting.sunLight.position.clone().normalize();
    const az = Math.atan2(sun.z, sun.x);
    const haze = [0, Math.PI / 2, Math.PI].map((off) => {
      const sum = [0, 0, 0];
      let n = 0;
      for (const da of [-0.12, 0, 0.12]) for (const y of [0.015, 0.05]) {
        const a = az + off + da;
        const c = radiance([Math.cos(a), y, Math.sin(a)]);
        for (let k = 0; k < 3; k++) sum[k] += c[k];
        n++;
      }
      return display(sum.map((x) => x / n), look);
    });
    const { tone, worst } = fitTone(look);
    const ring = (y) => {
      const sum = [0, 0, 0];
      for (let k = 0; k < 16; k++) {
        const a = (k / 16) * 2 * Math.PI, c = Math.sqrt(1 - y * y);
        const r = radiance([c * Math.cos(a), y, c * Math.sin(a)]);
        for (let j = 0; j < 3; j++) sum[j] += r[j] / 16;
      }
      return sum.map((x, j) => x * app.scene.environmentIntensity * look.balance[j]);
    };
    // The camera's white balance rides on the light.
    const balance = (c) => c.map((x, k) => x * look.balance[k]);
    out[id] = {
      light: { sky: balance(light.sky), ground: balance(light.ground), sun: balance(light.sun) },
      haze,
      fogHeight: lighting.uFogHeight?.value ?? 900,
      tone,
      env: { horizon: ring(0.05), up: ring(0.5) },
      sky: `sky/${id}.png`
    };
    log(`light ${id}: sky ${light.sky.map((x) => x.toFixed(3))} ground ${light.ground.map((x) => x.toFixed(3))} sun ${light.sun.map((x) => x.toFixed(3))} (residual ${light.residual.toFixed(4)}); tone ${tone.map((x) => x.toFixed(4))} (worst ${worst.toFixed(4)})`);
  }
  lighting.applyPreset('golden', true);
  return out;
}
