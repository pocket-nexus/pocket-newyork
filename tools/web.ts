// The reference as a page on this machine: three.js at a pinned commit, served beside web/.
//
// The reference's index.html imports three.js from the dev branch on jsDelivr, which moves. `three` downloads the
// modules it imports at THREE_COMMIT (the dev branch when the reference was last pushed) into web/vendor/three, and
// `serve` answers web/ with the CDN prefix of every HTML file replaced by that directory. web/ itself is unchanged.
//
//   bun tools/newyork.ts three          # fetch the pinned modules (once)
//   bun tools/newyork.ts serve          # http://127.0.0.1:5390/ is the reference, /pocket/… the export and viewer
//                                       # pages, /build/… the compiled packs

import { existsSync, mkdirSync } from "node:fs";
import { dirname, join, normalize, resolve } from "node:path";

export const ROOT = resolve(import.meta.dir, "..");
export const WEB = join(ROOT, "web");
export const PORT = 5390;

/** mrdoob/three.js dev on 2026-10-22 06:30 UTC, the last commit before the reference's last push (07:25 UTC). */
export const THREE_COMMIT = "bc7d0b3c3838d5da9495b5e603afd77ed3154d57";
const CDN = "https://cdn.jsdelivr.net/gh/mrdoob/three.js@dev/";
const PINNED = `https://cdn.jsdelivr.net/gh/mrdoob/three.js@${THREE_COMMIT}/`;
const VENDOR = join(WEB, "vendor/three");

/** The modules index.html imports; their own imports are followed. */
const ENTRIES = [
  "build/three.webgpu.js",
  "build/three.tsl.js",
  "examples/jsm/tsl/display/BloomNode.js",
  "examples/jsm/tsl/display/SSRNode.js",
  "examples/jsm/utils/BufferGeometryUtils.js",
  "examples/jsm/objects/SkyMesh.js",
  "examples/jsm/lights/SunLight.js",
  "examples/jsm/lights/SunLightNode.js",
  "examples/jsm/lighting/ClusteredLighting.js",
];

export async function fetchThree() {
  const seen = new Set<string>();
  const queue = [...ENTRIES];
  while (queue.length) {
    const path = queue.shift()!;
    if (seen.has(path)) continue;
    seen.add(path);
    const file = join(VENDOR, path);
    let text: string;
    if (existsSync(file)) text = await Bun.file(file).text();
    else {
      const res = await fetch(PINNED + path);
      if (!res.ok) throw new Error(`${path}: HTTP ${res.status}`);
      text = await res.text();
      mkdirSync(dirname(file), { recursive: true });
      await Bun.write(file, text);
    }
    // Relative imports; the bare ones ('three', 'three/webgpu', 'three/tsl') are the import map's.
    for (const m of text.matchAll(/(?:import|export)\s[^'"]*?from\s*['"](\.{1,2}\/[^'"]+)['"]|import\s*\(\s*['"](\.{1,2}\/[^'"]+)['"]\s*\)|import\s*['"](\.{1,2}\/[^'"]+)['"]/g)) {
      const rel = m[1] ?? m[2] ?? m[3];
      queue.push(normalize(join(dirname(path), rel)));
    }
  }
  console.log(`three.js ${THREE_COMMIT.slice(0, 10)}: ${seen.size} modules in ${VENDOR}`);
}

const TYPES: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json",
  ".png": "image/png",
  ".svg": "image/svg+xml",
};

async function up(): Promise<boolean> {
  try {
    return (await fetch(`http://127.0.0.1:${PORT}/index.html`)).ok;
  } catch {
    return false;
  }
}

/** Serves web/ until the process ends. */
export function serve() {
  if (!existsSync(join(VENDOR, "build/three.webgpu.js"))) throw new Error("run `bun tools/newyork.ts three` first");
  Bun.serve({
    port: PORT,
    hostname: "127.0.0.1",
    async fetch(req) {
      let path = decodeURIComponent(new URL(req.url).pathname);
      if (path.endsWith("/")) path += "index.html";
      // Compiled packs, for the viewer (web/pocket/view.html).
      if (path.startsWith("/build/")) {
        const file = normalize(join(ROOT, ".pocket-build/scene", path.slice(7)));
        if (!file.startsWith(join(ROOT, ".pocket-build/scene")) || !existsSync(file)) return new Response("not found", { status: 404 });
        return new Response(Bun.file(file), { headers: { "Cache-Control": "no-store" } });
      }
      const file = normalize(join(WEB, path));
      if (!file.startsWith(WEB) || !existsSync(file)) return new Response("not found", { status: 404 });
      const ext = file.slice(file.lastIndexOf("."));
      const headers = { "Content-Type": TYPES[ext] ?? "application/octet-stream", "Cache-Control": "no-store" };
      if (ext === ".html") return new Response((await Bun.file(file).text()).replaceAll(CDN, "/vendor/three/"), { headers });
      return new Response(Bun.file(file), { headers });
    },
  });
  console.log(`web/ on http://127.0.0.1:${PORT}/ (three.js ${THREE_COMMIT.slice(0, 10)})`);
}

/** Starts `serve` in a detached process unless one already answers on the port. */
export async function serveDetached() {
  if (await up()) return;
  const child = Bun.spawn(["bun", join(ROOT, "tools/newyork.ts"), "serve"], { cwd: ROOT, stdio: ["ignore", "ignore", "ignore"] });
  child.unref();
  for (let i = 0; i < 100; i++) {
    if (await up()) return;
    await Bun.sleep(100);
  }
  throw new Error("the web server did not start");
}
