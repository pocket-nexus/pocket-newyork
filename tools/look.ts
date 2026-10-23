// Pictures of the reference as it runs in Chrome over WebGPU, and what its scene holds.
//
//   bun tools/newyork.ts look [--time golden|day|...] [--camera ID] [--teleport KEY] [--mode car|...] [--size 1280x720]
//                             [--inventory] [--out DIR] [--headed]
//
// Writes `<out>/<name>.png` and, with --inventory, `<out>/inventory.json`: every drawn object of the scene by kind,
// with its triangles, instances and material.

import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { chromium } from "playwright-core";
import { PORT, ROOT, serveDetached } from "./web";

export async function look(argv: string[]) {
  const arg = (name: string) => {
    const i = argv.indexOf(`--${name}`);
    return i >= 0 ? argv[i + 1] : undefined;
  };
  const out = arg("out") ?? join(ROOT, ".pocket-build/validation/reference", new Date().toISOString().replace(/[:.]/g, "-"));
  mkdirSync(out, { recursive: true });
  const [w, h] = (arg("size") ?? "1280x720").split("x").map(Number);
  await serveDetached();
  const browser = await chromium.launch({
    channel: "chrome",
    headless: !argv.includes("--headed"),
    args: ["--use-angle=metal", "--enable-gpu", "--ignore-gpu-blocklist", "--enable-unsafe-webgpu", "--no-proxy-server"],
  });
  try {
    const page = await browser.newPage({ viewport: { width: w, height: h }, deviceScaleFactor: 1 });
    const errors: string[] = [];
    page.on("pageerror", (e: Error) => errors.push(String(e)));
    page.on("console", (m: { type(): string; text(): string }) => {
      if (m.type() === "error") errors.push(m.text());
      else if (/\[init\]/.test(m.text())) console.log(`  ${m.text()}`);
    });
    const t0 = performance.now();
    await page.goto(`http://127.0.0.1:${PORT}/index.html`);
    await page.waitForFunction(() => (window as any).__NYC_APP?.ready, null, { timeout: 300_000, polling: 500 });
    console.log(`  app up in ${((performance.now() - t0) / 1000).toFixed(1)} s, backend ${await page.evaluate(() => ((window as any).__NYC_APP.renderer.backend.isWebGPUBackend ? "WebGPU" : "WebGL2"))}`);
    const steps: string[] = [];
    if (arg("mode")) steps.push(`[data-mode="${arg("mode")}"]`);
    if (arg("time")) steps.push(`[data-time="${arg("time")}"]`);
    if (arg("teleport")) steps.push(`[data-teleport="${arg("teleport")}"]`);
    if (arg("camera")) steps.push(`[data-camera="${arg("camera")}"]`);
    for (const sel of steps) await page.evaluate((s) => (document.querySelector(s) as HTMLElement | null)?.click(), sel);
    await page.waitForTimeout(Number(arg("wait") ?? 6000));
    const name = arg("name") ?? ([arg("mode"), arg("teleport"), arg("camera"), arg("time")].filter(Boolean).join("-") || "start");
    await page.screenshot({ path: join(out, `${name}.png`) });
    console.log(`  ${join(out, `${name}.png`)}`);
    if (argv.includes("--inventory")) {
      const inv = await page.evaluate(() => {
        const app = (window as any).__NYC_APP;
        const rows: any[] = [];
        app.scene.traverse((o: any) => {
          if (!o.isMesh && !o.isLine && !o.isPoints) return;
          const g = o.geometry;
          const idx = g.index ? g.index.count : g.attributes.position?.count ?? 0;
          const tris = o.isMesh ? Math.floor((g.drawRange?.count !== Infinity ? Math.min(idx, g.drawRange.count) : idx) / 3) : 0;
          const path: string[] = [];
          for (let p = o; p; p = p.parent) if (p.name) path.unshift(p.name);
          const mats = (Array.isArray(o.material) ? o.material : [o.material]).map((m: any) => `${m.type}:${m.name || ""}`);
          rows.push({
            path: path.join("/"),
            name: o.name,
            kind: o.isInstancedMesh ? "instanced" : o.isMesh ? "mesh" : o.isLine ? "line" : "points",
            tris,
            instances: o.isInstancedMesh ? o.count : 1,
            verts: g.attributes.position?.count ?? 0,
            attrs: Object.keys(g.attributes).join(","),
            mats: mats.join(" "),
            visible: o.visible,
            shadow: o.castShadow,
          });
        });
        return { info: app.renderer.info.render, rows };
      });
      await Bun.write(join(out, "inventory.json"), JSON.stringify(inv, null, 1));
      const total = inv.rows.reduce((s: number, r: any) => s + r.tris * r.instances, 0);
      console.log(`  inventory: ${inv.rows.length} objects, ${(total / 1e6).toFixed(2)} M triangles with instances`);
    }
    if (errors.length) console.log(`  page errors: ${errors.slice(0, 6).join(" | ")}`);
  } finally {
    await browser.close();
  }
}
