//! The reference's cameras (`CameraSystem.update` in cameras.js): three that follow the player (chase, action,
//! pov) and the scenic views, fixed or scripted by the clock. The follow cameras are eased in the followed
//! entity's frame, the scenic ones in the world, as the reference eases them.

use crate::math::*;
use crate::player::{Body, Players};
use crate::world::World;
use crate::Mode;
use libm::{cosf, expf, fabsf, sinf};

/// The reference's preset ids, in `CAMERA_PRESETS` order (the pack's `CAMS` holds the same, with fields of view).
pub const PRESETS: [&str; 11] = ["chase", "action", "pov", "centralPark", "fifthAve", "timesSquare", "chryslerGCT", "observatory", "bryantPark", "waterfront", "drone"];
pub const CHASE: u8 = 0;

#[derive(Clone, Copy, Debug)]
pub struct Cameras {
    pub preset: u8,
    pub fov: f32,
    pub pos: V3,
    pub look: V3,
    initialized: bool,
    pub orbit_yaw: f32,
    pub orbit_pitch: f32,
    pub zoom: f32,
    dragging: bool,
    user_orbit_timer: f32,
    smoothed_speed: f32,
    follow: Option<Mode>,
    follow_offset: V3,
    follow_look_offset: V3,
    follow_base_y: f32,
}

impl Default for Cameras {
    fn default() -> Cameras {
        Cameras {
            preset: CHASE,
            fov: 56.0,
            pos: v3(0.0, 10.0, 380.0),
            look: v3(0.0, 8.0, 440.0),
            initialized: false,
            orbit_yaw: 0.0,
            orbit_pitch: 0.09,
            zoom: 1.0,
            dragging: false,
            user_orbit_timer: 0.0,
            smoothed_speed: 0.0,
            follow: None,
            follow_offset: V3::ZERO,
            follow_look_offset: V3::ZERO,
            follow_base_y: 0.0,
        }
    }
}

impl Cameras {
    pub fn is_follow(&self) -> bool {
        self.preset < 3
    }

    /// `setCameraPreset`: `snap` starts it where it should be instead of easing over.
    pub fn set_preset(&mut self, world: &World, preset: u8, snap: bool) {
        if preset as usize >= PRESETS.len() {
            return;
        }
        self.preset = preset;
        self.fov = world.cameras.get(preset as usize).map(|c| c.fov).unwrap_or(56.0);
        if snap {
            self.initialized = false;
        }
    }

    /// `resetForTeleport`: a fresh chase view.
    pub fn reset(&mut self, world: &World) {
        self.dragging = false;
        self.orbit_yaw = 0.0;
        self.orbit_pitch = 0.09;
        self.zoom = 1.0;
        self.user_orbit_timer = 0.0;
        self.smoothed_speed = 0.0;
        self.follow = None;
        self.follow_offset = V3::ZERO;
        self.follow_look_offset = V3::ZERO;
        self.follow_base_y = 0.0;
        self.set_preset(world, CHASE, true);
    }

    /// A drag about the player: radians a second of yaw and pitch (`orbit[0]` > 0 turns the view to the right, as a
    /// drag to the right does). While it lasts the camera is held; four seconds after it, it eases back behind.
    pub fn orbit(&mut self, orbit: [f32; 2], dt: f32) {
        let on = orbit[0] != 0.0 || orbit[1] != 0.0;
        self.dragging = on;
        if on {
            self.orbit_yaw -= orbit[0] * dt;
            self.orbit_pitch = (self.orbit_pitch + orbit[1] * dt).min(0.68).max(-0.32);
            self.user_orbit_timer = 4.0;
        }
    }

    pub fn update(&mut self, world: &World, players: &Players, dt: f32, elapsed: f32) {
        let mode = players.active;
        let e: Body = *players.active_body();
        let entity = mode;
        if self.user_orbit_timer > 0.0 && !self.dragging {
            self.user_orbit_timer = (self.user_orbit_timer - dt).max(0.0);
        }
        let tau = core::f32::consts::PI * 2.0;
        if !self.dragging && self.user_orbit_timer <= 0.0 && fabsf(e.speed) > 0.5 {
            self.orbit_yaw -= roundf_js(self.orbit_yaw / tau) * tau;
            self.orbit_yaw += (0.0 - self.orbit_yaw) * (1.0 - expf(-dt * 2.2));
        }
        // The follow cameras pull back with speed, eased
        self.smoothed_speed += (fabsf(e.speed) - self.smoothed_speed) * (1.0 - expf(-dt * 3.0));
        let speed_lag = self.smoothed_speed * 0.1;
        let total_yaw = e.heading + self.orbit_yaw;
        let p = e.pos;
        let (sh, ch) = (sinf(e.heading), cosf(e.heading));
        let (desired, look) = match PRESETS[self.preset as usize] {
            "chase" => {
                let dist = e.chase_distance * self.zoom * (1.0 + (self.smoothed_speed * 0.01).min(0.25));
                let height = e.chase_height * self.zoom + sinf(self.orbit_pitch) * dist * 0.85;
                let horiz = cosf(self.orbit_pitch) * dist + speed_lag;
                let pos = v3(p.x - sinf(total_yaw) * horiz, (p.y + height).max(0.65), p.z - cosf(total_yaw) * horiz);
                let up_bias = match entity {
                    Mode::Fly => 0.2,
                    Mode::Drive => 0.52,
                    _ => 1.55,
                };
                (pos, v3(p.x + sh * 5.2, p.y + e.target_y + up_bias, p.z + ch * 5.2))
            }
            "action" => {
                let side = match entity {
                    Mode::Drive => 2.65,
                    Mode::Fly => 4.2,
                    _ => 0.95,
                };
                let back = (match entity {
                    Mode::Drive => 3.6,
                    Mode::Fly => 8.5,
                    _ => 2.8,
                }) + speed_lag;
                let low = if entity == Mode::Fly { p.y + 1.2 } else { p.y + 0.92 };
                let pos = v3(p.x - sh * back + ch * side, low.max(0.55), p.z - ch * back - sh * side);
                (pos, v3(p.x + sh * 3.2, p.y + 1.05, p.z + ch * 3.2))
            }
            "pov" => {
                let (fwd, eye_y) = match entity {
                    Mode::Drive => (0.55, 1.52),
                    Mode::Cycle => (0.35, 1.55),
                    Mode::Fly => (1.65, 1.65),
                    Mode::Walk => (0.35, 1.68),
                };
                let pos = v3(p.x + sh * fwd, p.y + eye_y, p.z + ch * fwd);
                (pos, v3(p.x + sinf(total_yaw) * 40.0, p.y + eye_y + sinf(-self.orbit_pitch) * 18.0 + 1.5, p.z + cosf(total_yaw) * 40.0))
            }
            "centralPark" => {
                let sway = sinf(elapsed * 0.22) * 3.5;
                (v3(-70.0 + sway, 14.0, -1425.0), v3(-250.0, 6.5, -1690.0))
            }
            "fifthAve" => {
                let sway = sinf(elapsed * 0.25) * 1.8;
                (v3(sway, 6.5, -20.0), v3(-60.0, 150.0, 686.0))
            }
            "timesSquare" => {
                let sway = sinf(elapsed * 0.3) * 3.0;
                (v3(-603.0 + sway, 5.5, -290.0), v3(-566.0, 40.0, -40.0))
            }
            "chryslerGCT" => {
                let sway = cosf(elapsed * 0.22) * 2.5;
                (v3(300.0 + sway, 9.5, 95.0), v3(470.0, 120.0, -50.0))
            }
            "observatory" => {
                let pan = sinf(elapsed * 0.18) * 80.0;
                (v3(-58.0, 322.0, 660.0), v3(420.0 + pan, 150.0, -200.0))
            }
            "bryantPark" => {
                let sway = sinf(elapsed * 0.25) * 1.5;
                let (l, top) = if world.info.lawn_top.is_nan() { ([-260.0, -150.0, 40.0, 120.0], 1.3) } else { (world.info.lawn, world.info.lawn_top) };
                let lz = (l[2] + l[3]) * 0.5;
                (v3(l[0] + 18.0, top + 1.7, lz + sway), v3(l[1] + 260.0, 70.0, lz - 60.0))
            }
            "waterfront" => {
                let drift = sinf(elapsed * 0.15) * 40.0;
                (v3(1640.0, 60.0, 60.0 + drift), v3(150.0, 150.0, -150.0))
            }
            _ => {
                let angle = elapsed * 0.07 + 0.65;
                let radius = 720.0 + sinf(elapsed * 0.23) * 140.0;
                let alt = 330.0 + cosf(elapsed * 0.19) * 90.0;
                (v3(cosf(angle) * radius + 60.0, alt, sinf(angle) * radius + 250.0), v3(60.0, 160.0, 250.0))
            }
        };
        let k = 1.0 - expf(-dt * 8.5);
        let pov = self.preset == 2;
        if !self.initialized || pov {
            self.pos = desired;
            self.look = look;
            self.initialized = true;
            self.follow = None;
        } else if self.preset < 2 {
            // Eased in the followed entity's frame, its height in the world's
            if self.follow != Some(mode) {
                self.follow = Some(mode);
                self.follow_base_y = p.y;
                self.follow_offset = self.pos - p;
                self.follow_look_offset = self.look - p;
            }
            self.follow_base_y += (p.y - self.follow_base_y) * k;
            self.follow_offset = self.follow_offset.lerp(desired - p, k);
            self.follow_look_offset = self.follow_look_offset.lerp(look - p, k);
            let base = v3(p.x, self.follow_base_y, p.z);
            self.pos = base + self.follow_offset;
            self.look = base + self.follow_look_offset;
            self.pos.y = self.pos.y.max(0.55);
        } else {
            self.follow = None;
            self.pos = self.pos.lerp(desired, k);
            self.look = self.look.lerp(look, k);
        }
    }
}

/// `Math.round`: halves go up.
fn roundf_js(x: f32) -> f32 {
    libm::floorf(x + 0.5)
}
