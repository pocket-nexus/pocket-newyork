//! The shadows: for the hour's sun, the height below which each cell of a window of the city's height grid (`HMAP`)
//! is in shadow (`ny_frame::shadow`), as a texture every scene program reads. A thread of its own reads the
//! window around the eye from the pack and sweeps it whenever the eye has left the window's middle or the sun has
//! moved; the frame that finds a sweep finished starts showing it.

use pocket_vita_gxm::mem::Arena;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ny_frame::horizon::Coarse;
use ny_frame::shadow::{self as window, SIDE};
use ny_pack::Heights;
use ny_sim::math::V3;
use vita2d_sys as g;

const IDLE: u32 = 0;
const WORKING: u32 = 1;
const READY: u32 = 2;

struct Target(*mut u16);
unsafe impl Send for Target {}
unsafe impl Sync for Target {}

#[derive(Clone, Copy)]
struct Job {
    light: V3,
    /// The window's north-west cell.
    at: [i32; 2],
    target: usize,
}

struct Shared {
    head: Heights,
    /// Where the grid's values start in the pack.
    offset: u64,
    path: String,
    targets: [Target; 2],
    job: Mutex<Option<Job>>,
    state: AtomicU32,
    /// Hundredths of a millisecond the last read and sweep took.
    took: AtomicU32,
    /// The window's heights at 8 m, each the lowest of what stands on it: the street's occluders (`horizon.rs`).
    coarse: Mutex<Option<Arc<Coarse>>>,
}

impl Shared {
    /// Reads the window's heights (cells outside the grid stand at 0), sweeps them in ordinary memory, where the
    /// sweep's own reads are fast, then copies to the texture.
    unsafe fn sweep(&self, file: &mut File, heights: &mut [u16], scratch: &mut [u16], job: Job) {
        let t = Instant::now();
        let (w, h) = (self.head.w as i32, self.head.h as i32);
        let mut row = vec![0u8; SIDE * 2];
        for j in 0..SIDE {
            let dst = &mut heights[j * SIDE..(j + 1) * SIDE];
            dst.fill(0);
            let gz = job.at[1] + j as i32;
            if gz < 0 || gz >= h {
                continue;
            }
            let (from, to) = (job.at[0].max(0), (job.at[0] + SIDE as i32).min(w));
            if to <= from {
                continue;
            }
            let n = (to - from) as usize;
            if file.seek(SeekFrom::Start(self.offset + ((gz as u64) * w as u64 + from as u64) * 2)).is_err() || file.read_exact(&mut row[..n * 2]).is_err() {
                continue;
            }
            let at = (from - job.at[0]) as usize;
            for k in 0..n {
                dst[at + k] = u16::from_le_bytes([row[k * 2], row[k * 2 + 1]]);
            }
        }
        if self.coarse.lock().map(|c| c.as_ref().map(|c| (c.x0, c.z0))).ok().flatten() != Some(window::origin(&self.head, job.at)) {
            *self.coarse.lock().unwrap() = Some(Arc::new(window::coarse(heights, &self.head, job.at)));
        }
        window::sweep(heights, scratch, &self.head, job.light);
        core::ptr::copy_nonoverlapping(scratch.as_ptr(), self.targets[job.target].0, SIDE * SIDE);
        self.took.store((t.elapsed().as_secs_f32() * 100_000.0) as u32, Ordering::Relaxed);
    }
}

pub struct Shadows {
    textures: [g::SceGxmTexture; 2],
    /// The north-west cell of each texture's window.
    at: [[i32; 2]; 2],
    front: usize,
    shared: Arc<Shared>,
    /// Frames until the texture shown last may be written again: the GPU may still be reading it.
    cool: u32,
    asked: Job,
    pub sweeps: u32,
}

impl Shadows {
    /// # Safety
    /// GXM is initialized; `vram` outlives the shadows.
    pub unsafe fn new(vram: &mut Arena, path: &str, head: Heights, offset: u64, light: V3, eye: V3) -> Result<Shadows, String> {
        let mut textures: [g::SceGxmTexture; 2] = core::mem::zeroed();
        let mut targets = [core::ptr::null_mut::<u16>(); 2];
        for k in 0..2 {
            targets[k] = vram.alloc(SIDE * SIDE * 2, 512)?.cast();
            core::ptr::write_bytes(targets[k], 0, SIDE * SIDE);
            let r = g::sceGxmTextureInitLinear(&mut textures[k], targets[k].cast(), g::SceGxmTextureFormat_SCE_GXM_TEXTURE_FORMAT_U16_RRRR, SIDE as u32, SIDE as u32, 1);
            if r < 0 {
                return Err(format!("shadow texture: sceGxmTextureInitLinear 0x{:08x}", r as u32));
            }
            g::sceGxmTextureSetMinFilter(&mut textures[k], g::SceGxmTextureFilter_SCE_GXM_TEXTURE_FILTER_LINEAR);
            g::sceGxmTextureSetMagFilter(&mut textures[k], g::SceGxmTextureFilter_SCE_GXM_TEXTURE_FILTER_LINEAR);
            g::sceGxmTextureSetUAddrMode(&mut textures[k], g::SceGxmTextureAddrMode_SCE_GXM_TEXTURE_ADDR_CLAMP);
            g::sceGxmTextureSetVAddrMode(&mut textures[k], g::SceGxmTextureAddrMode_SCE_GXM_TEXTURE_ADDR_CLAMP);
        }
        let shared = Arc::new(Shared { head, offset, path: path.to_string(), targets: [Target(targets[0]), Target(targets[1])], job: Mutex::new(None), state: AtomicU32::new(IDLE), took: AtomicU32::new(0), coarse: Mutex::new(None) });
        let first = Job { light, at: window::window_at(&head, eye), target: 0 };
        // The first sweep before the first frame.
        let mut file = File::open(path).map_err(|e| format!("{path}: {e}"))?;
        let (mut heights, mut scratch) = (vec![0u16; SIDE * SIDE], vec![0u16; SIDE * SIDE]);
        shared.sweep(&mut file, &mut heights, &mut scratch, first);
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("newyork-shadows".into())
            .stack_size(128 * 1024)
            .spawn(move || {
                let mut file = match File::open(&worker.path) {
                    Ok(f) => f,
                    Err(_) => return,
                };
                loop {
                    let job = worker.job.lock().unwrap().take();
                    match job {
                        Some(job) => {
                            unsafe { worker.sweep(&mut file, &mut heights, &mut scratch, job) };
                            worker.state.store(READY, Ordering::Release);
                        }
                        None => std::thread::sleep(Duration::from_millis(8)),
                    }
                }
            })
            .map_err(|e| format!("shadow thread: {e}"))?;
        Ok(Shadows { textures, at: [first.at; 2], front: 0, shared, cool: 0, asked: first, sweeps: 1 })
    }

    /// Once a frame: shows a finished sweep, and asks for a new one when the sun has moved or the eye has left the
    /// middle of the window.
    pub fn update(&mut self, light: V3, eye: V3) {
        match self.shared.state.load(Ordering::Acquire) {
            READY => {
                self.front = 1 - self.front;
                self.at[self.front] = self.asked.at;
                self.cool = 3;
                self.sweeps += 1;
                self.shared.state.store(IDLE, Ordering::Release);
            }
            IDLE if self.cool == 0 => {
                let h = &self.shared.head;
                let moved = window::left(h, self.asked.at, eye);
                if moved || window::sun_moved(self.asked.light, light) {
                    self.asked = Job { light, at: if moved { window::window_at(h, eye) } else { self.asked.at }, target: 1 - self.front };
                    *self.shared.job.lock().unwrap() = Some(self.asked);
                    self.shared.state.store(WORKING, Ordering::Release);
                }
            }
            _ => {}
        }
        self.cool = self.cool.saturating_sub(1);
    }

    /// The texture to draw with this frame.
    pub fn texture(&self) -> &g::SceGxmTexture {
        &self.textures[self.front]
    }

    /// From world x and z to the texture's coordinates: scale, then x and z offsets.
    pub fn mapping(&self) -> [f32; 3] {
        window::mapping(&self.shared.head, self.at[self.front])
    }

    /// Metres of height in one unit of the texture as a program reads it (0..1).
    pub fn unit(&self) -> f32 {
        window::unit(&self.shared.head)
    }

    /// The street's occluders around the window the sweep last read.
    pub fn coarse(&self) -> Option<Arc<Coarse>> {
        self.shared.coarse.lock().ok().and_then(|c| c.clone())
    }

    /// Milliseconds the last read and sweep took.
    pub fn sweep_ms(&self) -> f32 {
        self.shared.took.load(Ordering::Relaxed) as f32 / 100.0
    }
}
