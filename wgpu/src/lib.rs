//! Pocket New York drawn with wgpu: in a browser tab over WebGPU, and on the machine that builds it (Metal), where
//! frames go to files (`src/bin/shot.rs`). See `wgpu/README.md`.

pub mod app;
mod blocks;
pub mod city;
pub mod pack;
mod post;
mod shadow;
mod sky;
#[cfg(target_arch = "wasm32")]
pub mod web;
