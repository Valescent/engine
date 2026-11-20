use far_heightfield_ring::{Report, build, preview_ppm};

const PREVIEW_PATH: &str = "preview.ppm";
const PREVIEW_SIZE: usize = 480;

fn main() {
    let camera = [0.0, 0.0];
    let clipmap = build(camera);
    let report = Report::new(&clipmap);
    print!("{report}");

    let ppm = preview_ppm(&clipmap, PREVIEW_SIZE);
    if let Err(error) = std::fs::write(PREVIEW_PATH, ppm) {
        eprintln!("failed to write {PREVIEW_PATH}: {error}");
        std::process::exit(1);
    }
    println!("preview={PREVIEW_PATH}");
}
