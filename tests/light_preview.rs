//! A text view of the column lighting (`src/light2.rs`).
//!
//! Not a rendering: it prints ASCII maps of sky, block, and water-attenuated
//! light. Run with `cargo test --test light_preview -- --nocapture` to see them.

use engine::terrain::light2::{ColumnBlocks, compute_column, default_table};
use engine::terrain::section::Section;

const AIR: u16 = 0;
const STONE: u16 = 1;
const GRASS: u16 = 3;
const LAMP: u16 = 6;
const WATER: u16 = 7;

const WIDTH: i32 = 16;
const SURFACE: i32 = 100;

fn blocks() -> ColumnBlocks {
    let mut sections: [Section; 12] = std::array::from_fn(|_| Section::uniform(AIR));
    let set = |sections: &mut [Section; 12], x: i32, y: i32, z: i32, id: u16| {
        sections[usize::try_from(y / 32).unwrap()].set(
            u32::try_from(x).unwrap(),
            u32::try_from(y % 32).unwrap(),
            u32::try_from(z).unwrap(),
            id,
        );
    };

    for z in 0..WIDTH {
        for x in 0..WIDTH {
            for y in 0..SURFACE {
                set(&mut sections, x, y, z, STONE);
            }
            set(&mut sections, x, SURFACE, z, GRASS);
        }
    }

    // A water channel along the north edge, four blocks deep.
    for z in 0..WIDTH {
        for y in (SURFACE + 1)..=(SURFACE + 4) {
            set(&mut sections, 8, y, z, WATER);
        }
    }

    // A one-block shaft straight down to the lamp chamber, open to the sky.
    for y in 20..=(SURFACE + 1) {
        set(&mut sections, 3, y, 3, AIR);
    }
    for y in 20..=22 {
        for z in 2..=4 {
            for x in 2..=4 {
                set(&mut sections, x, y, z, AIR);
            }
        }
    }
    set(&mut sections, 4, 21, 4, LAMP);

    ColumnBlocks::new(sections, &[None; 8])
}

fn glyph(level: u8) -> char {
    char::from_digit(u32::from(level), 16).unwrap_or('?')
}

/// 24-bit BMP, bottom-up, rows padded to four bytes. No dependency, opens in any
/// viewer on Windows.
fn write_bmp(path: &str, width: usize, height: usize, pixels: &[[u8; 3]]) {
    let row_bytes = (width * 3).div_ceil(4) * 4;
    let image_bytes = row_bytes * height;
    let mut out = Vec::with_capacity(54 + image_bytes);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(
        &u32::try_from(54 + image_bytes)
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&54_u32.to_le_bytes());
    out.extend_from_slice(&40_u32.to_le_bytes());
    out.extend_from_slice(&i32::try_from(width).unwrap_or(0).to_le_bytes());
    out.extend_from_slice(&i32::try_from(height).unwrap_or(0).to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&24_u16.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&u32::try_from(image_bytes).unwrap_or(0).to_le_bytes());
    out.extend_from_slice(&2_835_u32.to_le_bytes());
    out.extend_from_slice(&2_835_u32.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes());
    for row in (0..height).rev() {
        let start = out.len();
        for column in 0..width {
            let [r, g, b] = pixels[row * width + column];
            out.extend_from_slice(&[b, g, r]);
        }
        while out.len() - start < row_bytes {
            out.push(0);
        }
    }
    std::fs::write(path, out).expect("the preview path is writable");
}

#[test]
fn lighting_maps() {
    let light = compute_column(&blocks(), &default_table());

    println!("\nsky light, XZ slice at y = 60 (shaft at x=3,z=3, lamp chamber x,z 2..4)");
    println!(
        "    {}",
        (0..WIDTH).map(|x| (x % 10).to_string()).collect::<String>()
    );
    for z in 0..WIDTH {
        let row: String = (0..WIDTH).map(|x| glyph(light.sky(x, 60, z))).collect();
        println!("{z:3} {row}");
    }

    println!("\nblock light, XZ slice at y = 21 (the lamp sits at 4,21,4)");
    for z in 0..WIDTH {
        let row: String = (0..WIDTH).map(|x| glyph(light.block(x, 21, z))).collect();
        println!("{z:3} {row}");
    }

    println!("\nsky light, XY slice at z = 8 (water channel at x = 8, y 101..104)");
    for y in (60..=108).rev() {
        let row: String = (0..WIDTH).map(|x| glyph(light.sky(x, y, 8))).collect();
        println!("{y:3} {row}");
    }

    assert_eq!(light.sky(0, 120, 0), 15, "open sky above the surface");
    assert_eq!(light.sky(0, 99, 0), 0, "no sky under the roof");
    assert_eq!(
        light.sky(3, 30, 3),
        15,
        "the shaft carries sky to its floor"
    );
    assert_eq!(light.sky(6, 30, 6), 0, "solid rock stays dark");
    assert!(light.block(4, 21, 4) >= 13, "the lamp lights its own cell");
    assert_eq!(light.block(0, 120, 0), 0, "no block light in open air");
}

/// Writes `target/light-preview.bmp`: the XY slice at `z = 8`, `y` 60..108,
/// scaled 10x. Blue is sky light, orange is block light, black is unlit.
#[test]
fn write_light_image() {
    let light = compute_column(&blocks(), &default_table());
    let scale = 10_usize;
    let rows = 49_usize;
    let columns = usize::try_from(WIDTH).unwrap();
    let mut pixels = vec![[0_u8; 3]; rows * scale * columns * scale];
    for row in 0..rows {
        let y = i32::try_from(59 + rows - row).unwrap();
        for column in 0..columns {
            let x = i32::try_from(column).unwrap();
            let sky = light.sky(x, y, 8);
            let block = light.block(x, y, 8);
            let pixel = [
                (u16::from(block) * 18).min(255) as u8,
                (u16::from(sky) * 9 + u16::from(block) * 9).min(255) as u8,
                (u16::from(sky) * 17).min(255) as u8,
            ];
            for dy in 0..scale {
                for dx in 0..scale {
                    pixels[(row * scale + dy) * columns * scale + column * scale + dx] = pixel;
                }
            }
        }
    }
    write_bmp(
        "target/light-preview.bmp",
        columns * scale,
        rows * scale,
        &pixels,
    );
    println!("wrote target/light-preview.bmp");
}

#[test]
fn water_attenuates_per_block() {
    let light = compute_column(&blocks(), &default_table());
    assert_eq!(light.sky(8, 104, 0), 14);
    assert_eq!(light.sky(8, 103, 0), 13);
    assert_eq!(light.sky(8, 102, 0), 12);
    assert_eq!(light.sky(8, 101, 0), 11);
}
