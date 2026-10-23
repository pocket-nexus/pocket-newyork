//! SceneIR as the export page wrote it (`web/pocket/scene.js`, `probe.js`): the tables of `scene.json`, the arrays
//! of `buffers.bin`, and the colour of every probed vertex.

use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Clone, Copy, Debug)]
pub struct Array {
    pub off: usize,
    pub len: usize,
    #[serde(rename = "type")]
    pub kind: Kind,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    F32,
    U32,
    U16,
    I32,
    I16,
    U8,
    I8,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Attribute {
    #[serde(rename = "itemSize")]
    pub item_size: usize,
    #[serde(default)]
    pub normalized: bool,
    #[serde(flatten)]
    pub array: Array,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Group {
    pub start: usize,
    pub count: usize,
    #[serde(rename = "materialIndex")]
    pub material_index: usize,
}

#[derive(Deserialize, Clone, Debug)]
pub struct DrawRange {
    pub start: usize,
    pub count: i64,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Geometry {
    pub id: usize,
    #[serde(rename = "vertexCount")]
    pub vertex_count: usize,
    pub attributes: HashMap<String, Attribute>,
    pub index: Option<Array>,
    #[serde(default)]
    pub groups: Vec<Group>,
    #[serde(rename = "drawRange")]
    pub draw_range: DrawRange,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Material {
    pub id: usize,
    #[serde(default)]
    pub uuid: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub name: String,
    pub factory: Option<String>,
    pub args: Option<Value>,
    pub color: Option<u32>,
    pub transparent: bool,
    #[serde(default)]
    pub side: u32,
    #[serde(default)]
    pub nodes: Vec<String>,
    pub map: Option<String>,
    #[serde(default)]
    pub blending: u32,
    pub roughness: Option<f32>,
    pub metalness: Option<f32>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Instances {
    pub count: usize,
    pub matrix: Array,
    pub color: Option<Array>,
    #[serde(default)]
    pub attrs: HashMap<String, Attribute>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum MaterialRef {
    One(usize),
    Many(Vec<usize>),
}

#[derive(Deserialize, Clone, Debug)]
pub struct Object {
    pub id: usize,
    #[serde(default)]
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub geometry: usize,
    pub material: MaterialRef,
    #[serde(rename = "matrixWorld")]
    pub matrix_world: [f32; 16],
    pub shown: bool,
    #[serde(rename = "castShadow")]
    pub cast_shadow: bool,
    pub instances: Option<Instances>,
}

impl Object {
    pub fn material(&self, slot: usize) -> usize {
        match &self.material {
            MaterialRef::One(m) => *m,
            MaterialRef::Many(v) => v[slot.min(v.len() - 1)],
        }
    }
    pub fn instance_count(&self) -> usize {
        self.instances.as_ref().map(|i| i.count).unwrap_or(1)
    }
}

#[derive(Deserialize)]
struct SceneJson {
    geometries: Vec<Geometry>,
    materials: Vec<Material>,
    objects: Vec<Object>,
}

#[derive(Deserialize, Clone, Copy, Debug)]
pub struct ProbeEntry {
    pub object: usize,
    pub verts: usize,
    pub instances: usize,
}

pub struct Ir {
    pub dir: PathBuf,
    pub geometries: Vec<Geometry>,
    pub materials: Vec<Material>,
    pub objects: Vec<Object>,
    bytes: Vec<u8>,
    pub probes: HashMap<usize, ProbeEntry>,
}

/// The probed colours of one object: per point (instance × vertices + vertex) its albedo, then its emission.
pub struct Probe {
    pub verts: usize,
    pub instances: usize,
    bytes: Vec<u8>,
}

impl Probe {
    fn point(&self, instance: usize, vertex: usize) -> usize {
        (instance % self.instances) * self.verts + vertex
    }
    /// Albedo, sRGB; alpha is the opacity.
    pub fn albedo(&self, instance: usize, vertex: usize) -> [u8; 4] {
        let at = self.point(instance, vertex) * 4;
        [self.bytes[at], self.bytes[at + 1], self.bytes[at + 2], self.bytes[at + 3]]
    }
    /// Emission, linear.
    pub fn emission(&self, instance: usize, vertex: usize) -> [f32; 3] {
        let at = (self.verts * self.instances + self.point(instance, vertex)) * 4;
        let m = self.bytes[at + 3] as f32 / 255.0 * 16.0;
        [self.bytes[at] as f32 / 255.0 * m, self.bytes[at + 1] as f32 / 255.0 * m, self.bytes[at + 2] as f32 / 255.0 * m]
    }
}

impl Ir {
    pub fn load(dir: &Path) -> Result<Ir, String> {
        let text = std::fs::read_to_string(dir.join("scene.json")).map_err(|e| format!("scene.json: {e}"))?;
        let scene: SceneJson = serde_json::from_str(&text).map_err(|e| format!("scene.json: {e}"))?;
        let bytes = std::fs::read(dir.join("buffers.bin")).map_err(|e| format!("buffers.bin: {e}"))?;
        let probes = match std::fs::read_to_string(dir.join("probe.json")) {
            Ok(t) => serde_json::from_str::<Vec<ProbeEntry>>(&t).map_err(|e| format!("probe.json: {e}"))?.into_iter().map(|p| (p.object, p)).collect(),
            Err(_) => HashMap::new(),
        };
        Ok(Ir { dir: dir.to_path_buf(), geometries: scene.geometries, materials: scene.materials, objects: scene.objects, bytes, probes })
    }

    /// The facade style key of a material (`facade::key`), when a factory made it.
    pub fn material_key(&self, id: usize) -> Option<String> {
        let m = self.materials.get(id)?;
        Some(crate::facade::key(m.factory.as_deref()?, m.args.as_ref()?))
    }

    pub fn probe(&self, object: usize) -> Option<Probe> {
        let e = self.probes.get(&object)?;
        let bytes = std::fs::read(self.dir.join(format!("probe/{object}.bin"))).ok()?;
        if bytes.len() != e.verts * e.instances * 8 {
            return None;
        }
        Some(Probe { verts: e.verts, instances: e.instances, bytes })
    }

    /// An array as floats, whatever its stored type (normalized integers are scaled to -1..1 or 0..1).
    pub fn floats(&self, a: &Array, normalized: bool) -> Vec<f32> {
        let b = &self.bytes[a.off..];
        let n = a.len;
        let rd = |i: usize, w: usize| &b[i * w..i * w + w];
        match a.kind {
            Kind::F32 => (0..n).map(|i| f32::from_le_bytes(rd(i, 4).try_into().unwrap())).collect(),
            Kind::U32 => (0..n).map(|i| u32::from_le_bytes(rd(i, 4).try_into().unwrap()) as f32).collect(),
            Kind::I32 => (0..n).map(|i| i32::from_le_bytes(rd(i, 4).try_into().unwrap()) as f32).collect(),
            Kind::U16 => (0..n).map(|i| u16::from_le_bytes(rd(i, 2).try_into().unwrap()) as f32 / if normalized { 65535.0 } else { 1.0 }).collect(),
            Kind::I16 => (0..n).map(|i| (i16::from_le_bytes(rd(i, 2).try_into().unwrap()) as f32 / if normalized { 32767.0 } else { 1.0 }).max(-1.0)).collect(),
            Kind::U8 => (0..n).map(|i| b[i] as f32 / if normalized { 255.0 } else { 1.0 }).collect(),
            Kind::I8 => (0..n).map(|i| (b[i] as i8 as f32 / if normalized { 127.0 } else { 1.0 }).max(-1.0)).collect(),
        }
    }

    pub fn indices(&self, a: &Array) -> Vec<u32> {
        let b = &self.bytes[a.off..];
        match a.kind {
            Kind::U16 => (0..a.len).map(|i| u16::from_le_bytes([b[i * 2], b[i * 2 + 1]]) as u32).collect(),
            Kind::U32 => (0..a.len).map(|i| u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap())).collect(),
            _ => panic!("index array of type {:?}", a.kind),
        }
    }

    pub fn attribute(&self, g: &Geometry, name: &str) -> Option<(Vec<f32>, usize)> {
        let a = g.attributes.get(name)?;
        Some((self.floats(&a.array, a.normalized), a.item_size))
    }

    /// The triangles a geometry draws, as vertex indices, by material slot.
    pub fn triangles(&self, g: &Geometry) -> Vec<(usize, Vec<u32>)> {
        let all: Vec<u32> = match &g.index {
            Some(a) => self.indices(a),
            None => (0..g.vertex_count as u32).collect(),
        };
        let start = g.draw_range.start.min(all.len());
        let end = if g.draw_range.count < 0 { all.len() } else { (start + g.draw_range.count as usize).min(all.len()) };
        if g.groups.is_empty() {
            let mut t = all[start..end].to_vec();
            t.truncate(t.len() / 3 * 3);
            return vec![(0, t)];
        }
        g.groups
            .iter()
            .map(|gr| {
                let s = gr.start.max(start).min(end);
                let e = (gr.start + gr.count).min(end).max(s);
                let mut t = all[s..e].to_vec();
                t.truncate(t.len() / 3 * 3);
                (gr.material_index, t)
            })
            .collect()
    }
}
