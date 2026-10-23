// SceneIR export: opens web/pocket/export.html in Chrome over WebGPU and writes what the page sends into
// .pocket-build/scene/ir.
//
//   bun tools/newyork.ts export [--only survey|scene|bake|...] [--headed]

import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { chromium } from "playwright-core";
import { PORT, ROOT, serveDetached } from "./web";

const SINK = Number(process.env.NY_SINK_PORT ?? 5391);

export function irDir(): string {
  return join(ROOT, ".pocket-build/scene/ir");
}

export async function exportScene(argv: string[]) {
  const arg = (name: string) => {
    const i = argv.indexOf(`--${name}`);
    return i >= 0 ? argv[i + 1] : undefined;
  };
  const out = irDir();
  mkdirSync(out, { recursive: true });

  let received = 0;
  let bytes = 0;
  let finished: () => void = () => {};
  const done = new Promise<void>((r) => (finished = r));
  const cors = { "Access-Control-Allow-Origin": "*", "Access-Control-Allow-Methods": "PUT, OPTIONS", "Access-Control-Allow-Headers": "*" };
  const server = Bun.serve({
    port: SINK,
    hostname: "127.0.0.1",
    maxRequestBodySize: 1 << 30,
    async fetch(req) {
      if (req.method === "OPTIONS") return new Response(null, { headers: cors });
      const name = decodeURIComponent(new URL(req.url).pathname.slice(1));
      if (req.method !== "PUT" || !/^[\w.-]+(\/[\w.-]+)*$/.test(name) || name.includes("..")) return new Response("no", { status: 400, headers: cors });
      if (name === "done") finished();
      else {
        const body = await req.arrayBuffer();
        const file = join(out, name);
        mkdirSync(join(file, ".."), { recursive: true });
        await Bun.write(file, body);
        received++;
        bytes += body.byteLength;
      }
      return new Response("ok", { headers: cors });
    },
  });

  await serveDetached();
  const query = new URLSearchParams({ sink: `http://127.0.0.1:${SINK}` });
  for (const k of ["only", "at", "size"]) if (arg(k)) query.set(k, arg(k)!);
  const browser = await chromium.launch({
    channel: "chrome",
    headless: !argv.includes("--headed"),
    args: ["--use-angle=metal", "--enable-gpu", "--ignore-gpu-blocklist", "--enable-unsafe-webgpu", "--no-proxy-server"],
  });
  const t0 = performance.now();
  try {
    const page = await browser.newPage({ viewport: { width: 900, height: 700 }, deviceScaleFactor: 1 });
    const errors: string[] = [];
    page.on("pageerror", (e: Error) => errors.push(String(e)));
    page.on("console", (m: { type(): string; text(): string }) => {
      if (m.type() === "error") errors.push(m.text());
      else if (m.type() === "log") console.log(`  ${m.text()}`);
    });
    await page.goto(`http://127.0.0.1:${PORT}/pocket/export.html?${query}`);
    let over = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const failed = (async () => {
      while (!over) {
        await Bun.sleep(500);
        const fatal = errors.filter((e) => !/404|Failed to load resource|\[compile\]/.test(e));
        if (fatal.length) throw new Error(fatal.join(" | "));
      }
    })();
    const late = new Promise<void>((_, reject) => (timer = setTimeout(() => reject(new Error("export timed out")), 60 * 60_000)));
    try {
      await Promise.race([done, failed, late]);
    } finally {
      over = true;
      clearTimeout(timer);
    }
    if (errors.length) console.log("page errors:", errors.slice(0, 4).join(" | "));
  } finally {
    await browser.close();
    server.stop(true);
  }
  console.log(`export: ${received} files, ${(bytes / 1e6).toFixed(1)} MB in ${((performance.now() - t0) / 1000).toFixed(1)} s → ${out}`);
}
