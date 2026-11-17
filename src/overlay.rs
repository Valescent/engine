//! Screen-space debug overlay text.

use bytemuck::{Pod, Zeroable};

/// One pre-transformed overlay vertex in normalised device coordinates.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct OverlayVertex {
    pub(crate) position: [f32; 2],
    pub(crate) color: [f32; 4],
}

const GLYPH_WIDTH: u32 = 5;
const GLYPH_HEIGHT: u32 = 7;
const SCALE: f32 = 2.0;
const MARGIN: f32 = 8.0;
const PADDING: f32 = 6.0;
const TEXT_COLOR: [f32; 4] = [0.95, 0.95, 0.88, 1.0];
const BACKGROUND_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 0.6];

/// Builds the background and pixel-font quads for one line of overlay text.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn build_overlay_mesh(text: &str, width: u32, height: u32) -> Vec<OverlayVertex> {
    if width == 0 || height == 0 || text.is_empty() {
        return Vec::new();
    }

    let advance = (GLYPH_WIDTH + 1) as f32 * SCALE;
    let text_width = text.chars().count() as f32 * advance - SCALE;
    let text_height = GLYPH_HEIGHT as f32 * SCALE;

    let mut vertices = Vec::new();
    let background_min = (MARGIN - PADDING, MARGIN - PADDING);
    let background_max = (
        MARGIN + text_width + PADDING,
        MARGIN + text_height + PADDING,
    );
    push_quad(
        &mut vertices,
        background_min,
        background_max,
        BACKGROUND_COLOR,
        width,
        height,
    );

    for (index, character) in text.chars().enumerate() {
        let Some(glyph) = glyph(character) else {
            continue;
        };
        let origin_x = MARGIN + index as f32 * advance;
        for (row, bits) in glyph.into_iter().enumerate() {
            for column in 0..GLYPH_WIDTH {
                if bits & (1 << (GLYPH_WIDTH - 1 - column)) == 0 {
                    continue;
                }
                let x = origin_x + column as f32 * SCALE;
                let y = MARGIN + row as f32 * SCALE;
                push_quad(
                    &mut vertices,
                    (x, y),
                    (x + SCALE, y + SCALE),
                    TEXT_COLOR,
                    width,
                    height,
                );
            }
        }
    }
    vertices
}

fn push_quad(
    vertices: &mut Vec<OverlayVertex>,
    min: (f32, f32),
    max: (f32, f32),
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    let corners = [min, (max.0, min.1), max, min, max, (min.0, max.1)];
    for (x, y) in corners {
        vertices.push(OverlayVertex {
            position: to_ndc(x, y, width, height),
            color,
        });
    }
}

#[allow(clippy::cast_precision_loss)]
fn to_ndc(x: f32, y: f32, width: u32, height: u32) -> [f32; 2] {
    [x / width as f32 * 2.0 - 1.0, 1.0 - y / height as f32 * 2.0]
}

fn glyph(character: char) -> Option<[u8; 7]> {
    let rows = match character {
        ' ' => [0, 0, 0, 0, 0, 0, 0],
        '.' => [0, 0, 0, 0, 0, 0b00110, 0b00110],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
        ],
        '6' => [
            0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        _ => return None,
    };
    Some(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyphs_cover_the_overlay_text() {
        for character in "FPS0123456789.M ".chars() {
            assert!(glyph(character).is_some(), "{character:?}");
        }
    }

    #[test]
    fn mesh_is_deterministic_and_inside_ndc() {
        let first = build_overlay_mesh("FPS 5711  0.18 MS", 1280, 720);
        let second = build_overlay_mesh("FPS 5711  0.18 MS", 1280, 720);

        assert_eq!(first, second);
        assert_ne!(first.len(), 0);
        assert!(first.iter().all(|vertex| {
            vertex.position[0] >= -1.0
                && vertex.position[0] <= 1.0
                && vertex.position[1] >= -1.0
                && vertex.position[1] <= 1.0
        }));
    }

    #[test]
    fn empty_or_zero_sized_meshes_produce_nothing() {
        assert_eq!(build_overlay_mesh("", 1280, 720).len(), 0);
        assert_eq!(build_overlay_mesh("FPS", 0, 720).len(), 0);
        assert_eq!(build_overlay_mesh("FPS", 1280, 0).len(), 0);
    }
}
