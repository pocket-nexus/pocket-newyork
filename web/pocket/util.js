// What the export's modules share: the log on the page, the sink that receives files, and what a material is.

const params = new URLSearchParams(location.search);
const SINK = params.get('sink');

const logEl = document.getElementById('log');
export const log = (...a) => {
  const line = a.join(' ');
  console.log(line);
  logEl.textContent = line + '\n' + logEl.textContent.slice(0, 4000);
};
export const put = async (name, body) => {
  if (!SINK) return;
  const res = await fetch(`${SINK}/${name}`, { method: 'PUT', body });
  if (!res.ok) throw new Error(`${name}: HTTP ${res.status}`);
};

/** Each material of the scene: what made it and what the compiler can read of it. */
export function describeMaterial(m) {
  const nodes = ['colorNode', 'emissiveNode', 'normalNode', 'positionNode', 'opacityNode', 'maskNode', 'roughnessNode', 'metalnessNode', 'aoNode', 'outputNode', 'fragmentNode', 'vertexNode', 'castShadowNode']
    .filter((k) => m[k]);
  return {
    type: m.type,
    name: m.name,
    factory: m.userData.pocket?.factory ?? null,
    args: m.userData.pocket?.args ?? null,
    color: m.color ? m.color.getHex() : null,
    emissive: m.emissive ? m.emissive.getHex() : null,
    emissiveIntensity: m.emissiveIntensity ?? null,
    roughness: m.roughness ?? null,
    metalness: m.metalness ?? null,
    opacity: m.opacity,
    transparent: m.transparent,
    blending: m.blending,
    side: m.side,
    alphaTest: m.alphaTest,
    depthWrite: m.depthWrite,
    vertexColors: m.vertexColors,
    fog: m.fog,
    map: m.map ? (m.map.name || m.map.uuid) : null,
    nodes
  };
}


/** The scene's materials in the order scene.js numbers them (`materials[id]` of scene.json): the drawn objects in
 *  traversal order, each object's materials in turn. Every export run builds the same scene, so the numbers agree. */
export function materialOrder(scene) {
  const out = [];
  const seen = new Set();
  scene.traverse((o) => {
    if (!o.isMesh && !o.isLine && !o.isPoints && !o.isSprite) return;
    for (const m of Array.isArray(o.material) ? o.material : [o.material]) {
      if (!seen.has(m)) {
        seen.add(m);
        out.push(m);
      }
    }
  });
  return out;
}
