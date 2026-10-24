//! The shadows (`vita/src/shadow.rs`): for the hour's sun, the height below which each cell of a window of the city's
//! height grid (`HMAP`) is in shadow (`ny_frame::shadow`), as the texture every scene program reads. The window's
//! rows are read from the pack when the eye has left its middle; when they are in, the frame that finds them sweeps
//! them and shows the sweep. A tab has one thread: the sweep is the frame's (some milliseconds), which the PS Vita
//! leaves to a thread of its own.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use ny_frame::horizon::Coarse;
use ny_frame::shadow::{self as window, SIDE};
use ny_pack::Heights;
use ny_sim::math::V3;
use pocket_web_wgpu::gpu::Gpu;
use pocket_web_wgpu::{task, wgpu};

use crate::pack::Pack;

/// A window's rows of the grid, as they were read: its north-west cell, the first row read, and the bytes of the
/// rows from it (every column of the grid).
type Rows = ([i32; 2], i32, Result<Vec<u8>, String>);

pub struct Shadows {
    pack: Pack,
    head: Heights,
    /// Where the grid's values start in the pack.
    offset: u64,
    /// The window shown, the one asked for (being read), and the sun it was swept for.
    at: [i32; 2],
    asked: Option<[i32; 2]>,
    light: V3,
    /// The heights of the window shown, kept to sweep again when the sun moves.
    heights: Vec<u16>,
    swept: Vec<u16>,
    texels: Vec<f32>,
    coarse: Option<Arc<Coarse>>,
    arrived: Rc<RefCell<Option<Rows>>>,
    pub sweeps: u32,
    pub sweep_ms: f32,
}

/// The rows of the grid a window at `at` covers, clamped to the grid: the first, and how many.
fn rows(head: &Heights, at: [i32; 2]) -> (i32, i32) {
    let first = at[1].clamp(0, head.h as i32);
    let last = (at[1] + SIDE as i32).clamp(0, head.h as i32);
    (first, last - first)
}

impl Shadows {
    /// The window around `eye` read and swept for the sun towards `light` before the first frame.
    pub async fn new(gpu: &Gpu, texture: &wgpu::Texture, pack: &Pack, light: V3, eye: V3) -> Result<Shadows, String> {
        let (hmap, _) = pack.range(ny_pack::HMAP)?;
        let head: Heights = ny_pack::read(&pack.read_part(ny_pack::HMAP, 0, 32).await?, 0).ok_or("heights record")?;
        let mut s = Shadows {
            pack: pack.clone(),
            head,
            offset: hmap as u64 + 32,
            at: window::window_at(&head, eye),
            asked: None,
            light,
            heights: vec![0; SIDE * SIDE],
            swept: vec![0; SIDE * SIDE],
            texels: vec![0.0; SIDE * SIDE],
            coarse: None,
            arrived: Rc::new(RefCell::new(None)),
            sweeps: 0,
            sweep_ms: 0.0,
        };
        let (first, count) = rows(&head, s.at);
        let bytes = read(&s.pack, s.offset, &head, first, count).await;
        s.take(gpu, texture, (s.at, first, bytes));
        Ok(s)
    }

    /// The window at `at` from its rows, its occluders, and a sweep.
    fn take(&mut self, gpu: &Gpu, texture: &wgpu::Texture, (at, first, bytes): Rows) {
        let Ok(bytes) = bytes else { return };
        let (w, h) = (self.head.w as i32, self.head.h as i32);
        for j in 0..SIDE {
            let dst = &mut self.heights[j * SIDE..(j + 1) * SIDE];
            dst.fill(0);
            let gz = at[1] + j as i32;
            if gz < 0 || gz >= h {
                continue;
            }
            let (from, to) = (at[0].max(0), (at[0] + SIDE as i32).min(w));
            let row = (gz - first) as usize * w as usize * 2;
            for gx in from..to {
                let i = row + gx as usize * 2;
                if let Some(b) = bytes.get(i..i + 2) {
                    dst[(gx - at[0]) as usize] = u16::from_le_bytes([b[0], b[1]]);
                }
            }
        }
        self.at = at;
        self.coarse = Some(Arc::new(window::coarse(&self.heights, &self.head, at)));
        self.sweep(gpu, texture);
    }

    fn sweep(&mut self, gpu: &Gpu, texture: &wgpu::Texture) {
        let t = task::now();
        window::sweep(&self.heights, &mut self.swept, &self.head, self.light);
        for (o, &v) in self.texels.iter_mut().zip(&self.swept) {
            *o = v as f32 / 65535.0;
        }
        gpu.queue.write_texture(
            texture.as_image_copy(),
            bytemuck::cast_slice(&self.texels),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(SIDE as u32 * 4), rows_per_image: Some(SIDE as u32) },
            wgpu::Extent3d { width: SIDE as u32, height: SIDE as u32, depth_or_array_layers: 1 },
        );
        self.sweeps += 1;
        self.sweep_ms = (task::now() - t) as f32;
    }

    /// Once a frame: takes a window whose rows have come in, sweeps again for a sun that has moved, and asks for the
    /// rows of a new window when the eye has left the middle of this one.
    pub fn update(&mut self, gpu: &Gpu, texture: &wgpu::Texture, light: V3, eye: V3) {
        let came = self.arrived.borrow_mut().take();
        if let Some(came) = came {
            self.asked = None;
            self.light = light;
            self.take(gpu, texture, came);
            return;
        }
        if window::sun_moved(self.light, light) {
            self.light = light;
            self.sweep(gpu, texture);
        }
        if self.asked.is_none() && window::left(&self.head, self.at, eye) {
            let at = window::window_at(&self.head, eye);
            self.asked = Some(at);
            let (first, count) = rows(&self.head, at);
            let (pack, offset, head, arrived) = (self.pack.clone(), self.offset, self.head, self.arrived.clone());
            task::spawn(async move {
                let bytes = read(&pack, offset, &head, first, count).await;
                *arrived.borrow_mut() = Some((at, first, bytes));
            });
        }
    }

    /// Whether a window's rows are being read.
    pub fn reading(&self) -> bool {
        self.asked.is_some()
    }

    /// From world x and z to the texture's coordinates: scale, then x and z offsets.
    pub fn mapping(&self) -> [f32; 3] {
        window::mapping(&self.head, self.at)
    }

    /// Metres of height in one unit of the texture as a program reads it.
    pub fn unit(&self) -> f32 {
        window::unit(&self.head)
    }

    /// The street's occluders around the window shown.
    pub fn coarse(&self) -> Option<Arc<Coarse>> {
        self.coarse.clone()
    }
}

/// `count` rows of the grid from `first`, every column (the grid's values start at `offset` in the pack).
async fn read(pack: &Pack, offset: u64, head: &Heights, first: i32, count: i32) -> Result<Vec<u8>, String> {
    let row = head.w as u64 * 2;
    if count <= 0 {
        return Ok(Vec::new());
    }
    pack.source.range(offset + first as u64 * row, count as u64 * row).await
}
