//! Frames on this machine's GPU (Metal), with no browser and no interface: the pack is read, the words are
//! applied, and the frame after `--frames` more is written as a PNG; the run's status is printed as JSON.
//!
//!   ny-shot --pack .pocket-build/scene/vita60/city.pack --out a.png [--words "mode=play player=helicopter hour=0"]
//!           [--frames 60]

use ny_wgpu::app::{App, Held, HEIGHT, WIDTH};
use pocket_web_wgpu::gpu::{Gpu, Screen};
use pocket_web_wgpu::task;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let pack = arg("--pack").unwrap_or_else(|| ".pocket-build/scene/vita60/city.pack".into());
    let out = arg("--out").unwrap_or_else(|| "shot.png".into());
    let words = arg("--words").unwrap_or_default();
    let frames: u32 = arg("--frames").and_then(|v| v.parse().ok()).unwrap_or(30);

    let gpu = task::wait(Gpu::headless_wanting(pocket_web_wgpu::wgpu::Features::TEXTURE_COMPRESSION_BC)).expect("a GPU");
    let screen = Screen::texture(&gpu, WIDTH, HEIGHT, 1);
    let mut app = App::open(gpu.clone(), screen);
    // (a read here runs to its end before `load` returns)
    app.load(&pack);
    let mut now = 0.0f64;
    let mut frame = |app: &mut App| {
        now += 1000.0 / 60.0;
        app.step(now, &Held::default());
        app.draw().expect("a frame");
    };
    frame(&mut app);
    if !app.entered() {
        eprintln!("{}", app.status());
        std::process::exit(1);
    }
    app.control(&words);
    for _ in 0..frames {
        frame(&mut app);
    }
    let pixels = task::wait(app.screen.read(&gpu)).expect("the frame read back");
    let file = std::fs::File::create(&out).expect("the picture's file");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), WIDTH, HEIGHT);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().and_then(|mut w| w.write_image_data(&pixels)).expect("the PNG");
    println!("{}", app.status());
}
