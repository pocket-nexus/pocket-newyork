// Pictures of a compiled pack from the viewer page (web/pocket/view.html), beside the reference's own.
//
//   bun tools/newyork.ts view [--pack vita60/city.pack] [--eye x,y,z --at x,y,z] [--fov 56] [--hour golden]
//                             [--near 250 --mid 1500 --detail 110 --far 7000] [--size 960x544] [--name NAME] [--out DIR]

import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { chromium } from "playwright-core";
import { PORT, ROOT, serveDetached } from "./web";

export async function view(argv: string[]) {
  const arg = (name: string) => {
    const i = argv.indexOf(`--${name}`);
    return i >= 0 ? argv[i + 1] : undefined;
  };
  const out = arg("out") ?? join(ROOT, ".pocket-build/validation/view");
  mkdirSync(out, { recursive: true });
  const [w, h] = (arg("size") ?? "960x544").split("x").map(Number);
  await serveDetached();
  const query = new URLSearchParams();
  for (const k of ["pack", "eye", "at", "fov", "hour", "near", "mid", "detail", "far"]) if (arg(k)) query.set(k, arg(k)!);
  const browser = await chromium.launch({
    channel: "chrome",
    headless: !argv.includes("--headed"),
    args: ["--use-angle=metal", "--enable-gpu", "--ignore-gpu-blocklist", "--enable-unsafe-webgpu", "--no-proxy-server"],
  });
  try {
    const page = await browser.newPage({ viewport: { width: w, height: h }, deviceScaleFactor: 1 });
    const errors: string[] = [];
    page.on("pageerror", (e: Error) => errors.push(String(e)));
    page.on("console", (m: { type(): string; text(): string }) => m.type() === "error" && errors.push(m.text()));
    await page.goto(`http://127.0.0.1:${PORT}/pocket/view.html?${query}`);
    await page.waitForFunction(() => (window as any).__VIEW?.ready, null, { timeout: 300_000, polling: 250 });
    const state = await page.evaluate(() => (window as any).__VIEW);
    const name = arg("name") ?? "view";
    await page.screenshot({ path: join(out, `${name}.png`) });
    console.log(`  ${join(out, `${name}.png`)}: ${JSON.stringify(state)}`);
    if (errors.length) console.log(`  page errors: ${errors.slice(0, 4).join(" | ")}`);
  } finally {
    await browser.close();
  }
}
