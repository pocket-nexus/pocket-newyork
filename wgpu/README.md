# Pocket New York in a browser tab

The game of the PS Vita build, drawn with [wgpu](https://wgpu.rs) 25: over WebGPU in a tab (wasm32), and over
Metal on the machine that builds it, where frames go to files. It reads **the PS Vita's pack** (`city.pack`,
`cargo run --release -p ny-cook`) as it is: the same tables, records and pictures, in BC blocks where the adapter
reads them. What moves is `crates/ny-sim`, the flow `crates/ny-interface`, what a frame draws `crates/ny-view` and
what it works out on the processor `crates/ny-frame` (the hour's light, what moves as instances, what the street
hides, the cells' records to read, the shadow window, the sun's map), as the PS Vita links them. The game's own
interface (`ui/`, the bundle the PS Vita loads) runs over the city, and the page shows the game in a PS Vita's shell
with its keys as the controls.

| Part | What it is |
| --- | --- |
| `src/app.rs` | The shell: a frame as `step` (the interface's commands, the game) and the guest's turn, then `draw`; the read of the pack beside the frames; the control words. |
| `src/city.rs`, `src/shaders/scene.wgsl` | The city: the PS Vita's programs (`vita/shaders/*.cg`) as WGSL, the pack's streams and the cells' records on the GPU, a frame's draws. |
| `src/sky.rs`, `src/post.rs`, `src/shadow.rs` | The sky dome, the scene's targets and the glow, the shadow window. |
| `src/pack.rs` | The pack through the kernel's `Source`: its file by ranges, or its pieces. |
| `src/blocks.rs` | BC1 and BC3 taken apart into RGBA, for a GPU that reads no blocks. |
| `src/web.rs`, `page/` | The tab: what the page calls, and the page itself. |
| `src/bin/shot.rs` | A frame on this machine's GPU, as a PNG, and the run's status. |
| `vendor/pocketjs/devices/web/pocket-web-wgpu` | PocketJS's browser kernel: the device and the screens, the overlay pass, the pack's reads, the page's title card and frame loop, the interface's realm, and **the Pocket3D player** (the bar, the PS Vita's shell, the dock with Download and Remix). |
| `tools/wgpu.ts` | build, serve, dist, shot, check. |

```
cargo run --release -p ny-cook          # the PS Vita's pack → .pocket-build/scene/vita60/city.pack
bun tools/wgpu.ts serve                 # build when there is no site, then http://127.0.0.1:8804/
bun tools/wgpu.ts shot --words "start=helicopter hour=0" --out a.png
bun tools/wgpu.ts check                 # Chrome: the title, each way through the city by day and by night, the menu
bun tools/wgpu.ts dist                  # the directory a static host serves
bun tools/wgpu.ts check --dist          # the same check of that directory
bun test tools/wgpu.test.ts             # the page's devices, the player's and the release's are one list
```

The build needs the `wasm32-unknown-unknown` target and `wasm-bindgen` 0.2.126 on the path (the version in
`Cargo.lock`; `build` refuses another), and `bun install` in `vendor/pocketjs` for the interface.

## The pack in a tab

**Every read is a range.** On the build machine the file is read; in a tab `serve` answers ranges of it, and `dist`
cuts it into pieces of at most 4 MiB named by their bytes (`pocket3d-packs`, cut where its sections begin), which a
host that limits a file's size serves whole and a browser keeps. Before the first frame the tab reads the tables, the
pack's own streams (the shapes, the mid and far levels), the pictures, the lamps' light, what moves and a window of
the heights: 138 MB of the 380. A cell's record of `NEAR` is read when the eye comes near, into one of 128 slots of a
storage buffer.

**A vertex is read by its index.** The pack's records are 20 or 16 bytes with attributes on any byte, which a WebGPU
vertex buffer does not take: the programs read them from storage buffers (the pack's streams, the cells' slots, this
frame's models) by `vertex_index` and `instance_index`, so no record is laid out again. A draw's uniforms (a slot of
256 bytes, by dynamic offset) name the buffer and the word its records start at.

## What differs from the PS Vita's picture

- **Sixty frames a second at the PS Vita's full reaches**, with no governor (the PS Vita: thirty, with a governor that
  draws the reaches in).
- **Four samples a pixel** (the PS Vita: two); the pictures are filtered with anisotropy 4.
- **The sun's map is 2 048²** over the same 320 m square (the PS Vita: 1 024²), read through a comparison sampler,
  which mixes four texels.
- **The depth runs from 1 at the near plane to 0 at infinity** in a 32-bit float buffer, where the PS Vita's runs
  from 0 to 1 at 30 km.
- **The glow is this frame's**, laid over it as the frame goes to the display; the PS Vita lays on the glow of the
  frame it is showing.
- **The world steps in the frame** (the PS Vita steps it on a thread of its own), and the highway's lanes in turn
  every fourth frame (the PS Vita: every eighth). **The shadow window is swept in the frame** that finds its rows in,
  about 1.3 ms.
- **The interface's settings** are kept in the browser's `localStorage` under `pocket-newyork.interface`.
