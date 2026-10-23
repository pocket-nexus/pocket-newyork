# Pocket New York

Manhattan on a PS Vita: on foot, in a yellow taxi, on a bicycle or in a sightseeing helicopter, from the Battery to 125th Street, at four hours of the day, with the traffic on its streets. The city is [Slop New York](https://github.com/mrdoob/slop-newyork) by mrdoob, a three.js simulation of New York built from NYC Open Data and OpenStreetMap; this repository compiles what it draws for a handheld and draws it there.

**Status: in progress.** It runs on a PS Vita in Pocket Devkit at 30 frames a second, at street level and over the title's aerial camera; [Where it stands](#where-it-stands) lists what is left. No package has been released. Compiled city data keeps the terms of its sources ([Attribution](#attribution)).

The city is modelled once, as the reference's three.js scene that runs in a browser, and a compiler lowers it to what a console draws:

- **`web/`** is the model: Slop New York by mrdoob (MIT), unmodified, with its builders, materials, controllers, cameras and traffic. `web/pocket/` adds the export pages and a viewer of compiled packs.
- **`crates/ny-cook`** is the scene compiler: SceneIR in, one pack out, for a device profile (`profiles/vita60.json`).
- **`crates/ny-pack`** is the pack: tables, vertex layouts and sections shared by the compiler and the runtimes.
- **`crates/ny-view`** is what a frame draws: the places of the pack at the level of detail their distance asks for.
- **`crates/ny-sim`** is what moves, the same on every device: the reference's four ways through the city, its cameras, its street names, its signals, its traffic, its pedestrians and its highway's cars, compared against the reference itself.
- **`ui/`** is the interface: one PocketJS app drawn over the city. **`crates/ny-interface`** is the renderer's side of it and the flow around play (the title, play, the menu).
- **`vita/`** draws a pack on the PS Vita through GXM.

PocketJS (pinned in `vendor/pocketjs`) supplies the device toolchains, the dev host, the GXM kernel and packaging, the Pocket3D title card at launch and **the app icon in the console's launcher** (`vendor/pocketjs/engine/pocket3d/icon/`). This repository holds no icon file.

## From the reference to a frame

```
bun install && bun tools/newyork.ts three   # three.js at the commit the reference was last pushed against
bun tools/newyork.ts export                 # the reference's scene → .pocket-build/scene/ir (SceneIR)
cargo run --release -p ny-cook              # SceneIR → .pocket-build/scene/vita60/city.pack
bun tools/ui.ts vita                        # the interface for the Vita
bun tools/vita.ts native                    # build, sync the pack and the interface, replace the binary Pocket Devkit runs
bun tools/vita.ts bench                     # frame timings over the title's aerial camera → .pocket-build/validation/bench/
bun tools/newyork.ts view                   # a pack in a browser tab, for looking at what the compiler made
bun tools/newyork.ts look                   # the reference itself, in Chrome over WebGPU, for comparison
```

The reference imports three.js from its dev branch on a CDN; `THREE_COMMIT` in `tools/web.ts` pins the commit current when the reference was last pushed, and `serve` answers the reference's imports from that copy. The export takes about two minutes.

## SceneIR: the export runs the reference

`web/pocket/export.html` loads the reference's own scripts in Chrome over WebGPU, with every material factory (`create…Material`) wrapped so the material it returns carries the factory's name and arguments, and builds Manhattan with the reference's own builders. It writes:

- **The scene** (`scene.js`): every drawn object, its geometry, its world matrix, its material and its instances. The reference streams some instances in around its camera (building details within 560 m, the trees' levels of detail); the export runs that streaming at the 468 points of a 400 m grid over the island and keeps the union: 771 196 instances of 712 objects.
- **Every vertex's colour** (`probe.js`): each object drawn again as points, one pixel a vertex, through its own material's colour graph, then through its emission graph under the night preset. A surface without a picture of its own takes these colours.
- **Facade pages** (`bake.js`): a box-local facade material lays a wall out in bays and storeys (`facadeCell`); each is drawn flat by the reference's own `createFacadeBoxes`, by day and by night: the upper storeys of a street front and a rear wall, the ground storey of a shop, an entrance and a rear wall, windowless walls, the corner piers of masonry. 80 pages of 10 materials.
- **Ground pictures**: the sidewalk's flags, the asphalt and its manholes, park paving and lawn, each from straight above over a square its pattern repeats in, folded at its edges so that it repeats without a seam.
- **The leaves**: the trees' leaf atlas through the tree material's colour graph.
- **What moves** (`models.js`): the reference's own stand-in cars (drawn there past 30 m), the bus, and the player's taxi, bicycle and helicopter, each vertex probed with white and with black paint so the compiler knows what of it is paint.
- **Signs and billboards** (`bake.js`): every material that shows a picture over its texture coordinates (Times Square's billboards, shop signs, street-name blades) drawn flat through its own colour graph.
- **The light** (`light.js`): per hour, a white diffuse sphere drawn under the reference's lights and environment, its linear radiance read back and fitted as a hemisphere and the sun; the reference's sky as a panorama, graded and tone-mapped as its post-processing does; the haze's colour over the horizon by angle to the sun; the sky a surface reflects; and the grade and ACES of a grey fitted as one rational curve (within 0.013 of the display's value).
- **The world** (`world.js`, `sim.js`, `map.js`): the hours read back from the reference's lighting, its places, cameras and street names, its colliders and road network, and its map of the island.

## The compiler

- **Facade boxes at the far level** are prisms, as Pocket Tokyo draws its simple levels: every box's footprint goes into one grid of 4 m cells, cells within 10 m of height form a terrace, and a terrace's outline, straightened, becomes its walls (only where it looks out over lower ground) and a flat top. 138 000 triangles for all of Manhattan, low-rise included, where a painted box per tall building took 346 000.
- **Facade boxes** become their walls, laid out as the reference's `facadeCell` lays them: a row for a partial storey at the bottom, the ground storey under its shop, entrance or rear page, the whole storeys above under the upper page, a solid band for a storey cut short at the top, and on masonry, columns for the corner piers. A wall quad is tested against the grid of heights 1.5 m out from its face: one its neighbours cover all along is left out, and at the mid and far levels one a lower neighbour covers in part starts at the neighbour's roof. That leaves out 40 % of the wall triangles.
- **Repeated things** (lamp posts, signals, cornices, water towers, awnings, the bridges' members, the trees) are one shape each and an instance table: a matrix and a tint. Instance colours (awnings) are the tint itself; any other per-instance difference rides as the instance's mean probed colour against the first's.
- **The levels of detail.** A cell is 128 m and holds the near level; its small repeated things (radius under 2.5 m) are its detail level. A block is 4 × 4 cells and holds the mid level; a region is 4 × 4 blocks and holds the far level. Island-wide meshes (the streets, the parks, the water) are cut by triangle. Each level simplifies what it holds (meshoptimizer, welded first), and what is merged clutter goes by a grid past the near level.
- **Order is the culling structure.** A mid batch has its triangles (an instanced one, its instances) ordered by the cell of its block, a far batch by the block of its region, with a record of where each starts (`SPAN`): a frame draws, of a block the eye is near, the runs of the cells it does not draw near.
- **Cells' records.** A cell's near and detail levels are one record of `NEAR`, read from the memory card when the eye comes near: 204 MB in all, 0.47 MB at most. The pack's own streams (the shapes, the mid and far levels) stay in memory.
- **Heights.** Everything near that stands up is drawn into a grid of 2 m cells (`HMAP`): a facade box by its footprint, anything else by its triangles.
- **Island-wide surfaces** (the water, the streets) are cut into pieces of at most 128, 256 and 512 m at the near, mid and far levels, so that what a program works out at the vertices does not stretch across a river; the reference's water past the grid is left out.
- **What a surface reflects** rides with it: roughness and metalness in a solid vertex's spare bytes, and per picture for walls, ground and signs.

## The frame on the Vita

- **The levels of detail** follow `ny_view::select`: regions past the far reach (7 km, where the hour's haze has the city) are not drawn, regions past the mid reach draw their far level, blocks past the near reach their mid level, cells within it their near level and, within the detail reach, their small things. A governor draws the reaches in when a frame draws more than its triangle budget or waits on the GPU.
- **Shadows** come from `HMAP`. A thread reads a 2 km window of it around the eye from the pack and sweeps it for the hour's sun (`ny_sim::shadow::sweep`, from Pocket Tokyo): each cell gets the height below which a point over it is in shadow. Every program compares its own height with the window's, read a little way out along its normal so that a wall does not read its own building's top.
- **The programs** light in linear light with the hour's read-back light and end with its tone curve, then lay the haze over: the reference's aerial fog, its colour by the draw's direction and its density by the draw's height. Surfaces reflect the sky by their roughness and metalness (Schlick's Fresnel as roughness damps it), and the city's dim light where a reflection meets buildings across the street. Walls read their facade page and, by night, the page of what their rooms emit. Ground reads its picture over world x and z. Geometry coloured by vertex takes its probed colour, and its emission by night. Instanced shapes take their instance's matrix and tint from a second stream; trees cut their cards where the leaves' picture is under half. The sky is a dome under the hour's panorama.
- **Shadows near the eye.** Besides the height grid's sweep, a 1 024² depth map of a 320 m square the sun looks down on is drawn before each frame from the near cells and what moves (trees, lamp posts, awnings, cars and people cast shadows, buildings' have edges); every program takes the darker of the two.
- **What the street hides is not drawn.** A horizon around the eye, from the height grid's 8 m cells at their lowest within 256 m, leaves out the mid and far blocks (and near cells) nothing of which rises over it: from street level most of the city behind the avenue's walls.
- **Night.** The lamps' pools are the reference's cobra-head spotlights (88 cd, 0.96 rad, 25 m); everything lit takes them, and the light they leave around (the pools over ~80 m); wet ground mirrors them by Fresnel, wetness and smoothness. The reference's bloom runs at every hour, and the sun has its disc.
- **What moves** is drawn from the game each frame: the traffic's cars (the reference's full cars within 40 m, its stand-ins past that) under pictures of themselves seen along each axis, those in view to 450 m; the sidewalks' people (the nearest 24 within 70 m, the near figure within 15 m) at their heights and in their coats; the highway's cars; and the player's vehicle. The traffic, the people and the highway step on a thread of their own alongside the frame.
- **The interface** is the PocketJS guest of `ui/`, drawn over the city.
- **The Pocket3D title card** plays first.

## Measured on the console

PS Vita, Pocket Devkit over USB, 30 frames a second, 2× multisampling, `bun tools/vita.ts bench` and the device's own timings (`profile`), 2026-10-23:

| | |
| --- | --- |
| Street level (5th Ave & E 37th St), 30 s | 900 frames, 8 late; a mean of 175 000 triangles |
| Title's aerial camera, 30 s | 900 frames, 5 late; a mean of 176 000 triangles |
| GPU at the street, by part (reaches at the governor's floor) | trees 5.5 ms, what moves 5.1, bloom 1.8, the sun's map 1.5 |
| CPU | the world's step off the frame (0.1 ms waited for it); the horizon 4.8 ms |
| First launch | about 75 s, most of it copying the 377 MB pack to the memory card |

## Where it stands

Beside the reference from the same eye (`bun tools/newyork.ts look --eye x,y,z --at x,y,z`), what is left:

- **Roofs** past the near level are flat and bluish: the reference draws rooftop plant (water towers, bulkheads, chillers) within 560 m, and here only the near level carries it.
- **Leaves** past the sun's map have no shadow, and near crowns cost the most of a street frame (their cards are cut, so the GPU shades every layer).
- **Not drawn yet:** the steam, the headlights' beams, the highway's lamp flares, the soft shadow under a car.
- **Not done:** the Pocket3D Demo row, LiveArea pictures, a package.

## The interface

Every 2D pixel comes from one PocketJS app, `ui/`: the title, the readouts over play (the place over a pair of green street-name signs, the island's map, the speed, the hour), the menu, the lists of ways, hours, cameras and places, the settings. Its names and its map are compiled from the export. `bun tools/ui.ts preview` draws every screen; `bun tools/ui.ts test` drives the flow against a mock renderer.

## Controls

| | Go | Look | Climb, descend (jump) | Boost | Hour | Camera | Menu |
| --- | --- | --- | --- | --- | --- | --- | --- |
| PS Vita | left stick | right stick | R, L | ✕ | d-pad left, right | SELECT | START |

## Attribution

Geographic data: NYC Open Data and © OpenStreetMap contributors (ODbL); a compiled pack is a derived database under the ODbL. The model in `web/` is [Slop New York](https://github.com/mrdoob/slop-newyork) by mrdoob, under the MIT License (`web/LICENSE`), built on [three.js](https://threejs.org/).

## License

The code is under the [MIT License](LICENSE); the code in `web/` keeps its own notice. The licence covers the code only: the city data keeps the terms of its sources listed above. Pocket3D's title card, app icon and device kernels come from PocketJS under their own licence.
