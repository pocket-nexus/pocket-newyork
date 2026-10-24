//! What a frame of Pocket New York works out on the processor before it draws, the same on every device: the
//! hour's light as the scene programs take it (`light`), what moves as instances of the pack's models (`movers`),
//! what the street hides (`horizon`), which cells' records to read and where they go (`near`), the window of the
//! height grid the shadows are swept over (`shadow`), the sun's depth map around the eye (`sun`), the lamps' light
//! around a point (`lamps`), the trees nearest the eye at finer levels (`trees`) and the matrices (`mat`). A device
//! owns its GPU, its threads and its reads; the PS Vita (`vita/`) and the browser (`wgpu/`) both build on this.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod horizon;
pub mod lamps;
pub mod light;
pub mod mat;
pub mod movers;
pub mod near;
pub mod shadow;
pub mod sun;
pub mod trees;

/// `x` modulo `m` in `0..m` (`f32::rem_euclid`, which `core` lacks).
fn wrap(x: f32, m: f32) -> f32 {
    let r = x % m;
    if r < 0.0 {
        r + libm::fabsf(m)
    } else {
        r
    }
}
