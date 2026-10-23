// What the city offers besides its surfaces, as the reference states it: its hours (read back from its lighting
// after each preset is applied, with how light.js reads it back), its places (the teleports of index.html with their names, and the landmarks of
// city-config.js) and its cameras (cameras.js). Writes `world.json`.

import { readLight } from './light.js';
import { log, put } from './util.js';

/** The hours: what LightingSystem.applyPreset leaves on the lights, the fog and the city's uniforms. */
function hours(app) {
  const { lighting } = app;
  const u = window.NYC.cityUniforms;
  const out = [];
  const lin = (c) => [c.r, c.g, c.b];
  for (const id of ['golden', 'midday', 'twilight', 'midnight']) {
    lighting.applyPreset(id, true);
    const sun = lighting.sunLight;
    const hemi = lighting.hemiLight;
    out.push({
      id,
      sunDir: sun.position.clone().normalize().toArray(),
      sun: lin(sun.color).map((c) => c * sun.intensity),
      hemiSky: lin(hemi.color).map((c) => c * hemi.intensity),
      hemiGround: lin(hemi.groundColor).map((c) => c * hemi.intensity),
      skyTop: lin(u.uSkyTopColor.value),
      skyHorizon: lin(u.uSkyHorizonColor.value),
      fog: lin(app.scene.fog.color),
      fogDensity: app.scene.fog.density,
      interiorGlow: u.uInteriorGlow.value,
      roadWetness: u.uRoadWetness.value,
      canyonBounce: u.uCanyonBounce.value,
      night: lighting.uNightFactor.value,
      skyExposure: lighting.uSkyExposure.value,
      envIntensity: app.scene.environmentIntensity
    });
  }
  lighting.applyPreset('golden', true);
  return out;
}

/** The teleports, named as index.html's buttons name them. */
async function places() {
  const html = await (await fetch('../index.html')).text();
  const doc = new DOMParser().parseFromString(html, 'text/html');
  const at = window.NYC.TELEPORT_LOCATIONS;
  return [...doc.querySelectorAll('[data-teleport]')].map((b) => {
    const id = b.getAttribute('data-teleport');
    const loc = at[id];
    return { id, name: b.querySelector('.pop-name')?.textContent.trim() ?? id, sub: b.querySelector('.pop-sub')?.textContent.trim() ?? '', x: loc.x, z: loc.z, heading: loc.heading };
  });
}

export async function exportWorld(app) {
  const NYC = window.NYC;
  const world = {
    hours: hours(app),
    places: await places(),
    landmarks: NYC.NYC_LANDMARKS.map((l) => ({ id: l.id, name: l.name, short: l.short, x: l.x, z: l.z, district: l.district })),
    cameras: NYC.CAMERA_PRESETS.map((c) => ({ ...c })),
    island: NYC.ISLAND_BOUNDS,
    centralPark: NYC.CENTRAL_PARK_BOUNDS
  };
  const light = await readLight(app);
  for (const h of world.hours) Object.assign(h, light[h.id]);
  await put('world.json', JSON.stringify(world, null, 1));
  log(`world: ${world.hours.length} hours, ${world.places.length} places, ${world.landmarks.length} landmarks, ${world.cameras.length} cameras`);
}
