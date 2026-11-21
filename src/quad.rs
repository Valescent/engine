//! The 12-byte quad record from ADR-0015.
//!
//! Three `u32` words per merged quad, laid out so the vertex shader can pull
//! one record per six vertices and reconstruct the corners. The CPU mesher
//! emits one record per merged quad in [`crate::mesh::MeshData::quads`], and
//! the quad-pool renderer uploads those records and draws them indirectly
//! through [`ChunkInfo`] ranges.
//!
//! Layout:
//!
//! | Word | Bits |
//! |---|---|
//! | W0 | `x:5 y:5 z:5 width-1:5 height-1:5 face:3 flags:4` |
//! | W1 | `layer:8 ao0:2 ao1:2 ao2:2 ao3:2 c0 sky:4 block:4 c1 sky:4 block:4` |
//! | W2 | `c2 sky:4 block:4 c3 sky:4 block:4` and 16 spare bits |

#![allow(dead_code)] // Layout helpers are pinned by tests; the renderer uses the packed words.

use bytemuck::{Pod, Zeroable};
use glam::IVec3;

/// Water quad flag.
pub(crate) const FLAG_WATER: u32 = 1 << 0;

/// Sub-voxel (4 cm) quad flag.
pub(crate) const FLAG_SUBVOXEL: u32 = 1 << 1;

const FACE_NORMALS: [IVec3; 6] = [
    IVec3::X,
    IVec3::NEG_X,
    IVec3::Y,
    IVec3::NEG_Y,
    IVec3::Z,
    IVec3::NEG_Z,
];

/// In-plane axes for a face, matching the mesher's `(u, v)` order.
fn face_axes(face: u8) -> (IVec3, IVec3) {
    match face {
        0 | 1 => (IVec3::Z, IVec3::Y),
        2 | 3 => (IVec3::X, IVec3::Z),
        _ => (IVec3::X, IVec3::Y),
    }
}

/// Corner order in `(u, v)` bits, wound counter-clockwise around the normal.
fn corner_uv(face: u8, corner: usize) -> (u32, u32) {
    match face {
        0 | 2 | 5 => [(0, 0), (0, 1), (1, 1), (1, 0)][corner],
        1 | 4 => [(1, 0), (1, 1), (0, 1), (0, 0)][corner],
        _ => [(0, 1), (0, 0), (1, 0), (1, 1)][corner],
    }
}

/// One merged quad in the layout fixed by ADR-0015.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Pod, Zeroable)]
pub(crate) struct QuadRecord {
    words: [u32; 3],
}

impl QuadRecord {
    /// Packs one quad. Positions are chunk-local cells; extents are 1..=32.
    ///
    /// `corners` are `pack_light_ao` values in the mesher's corner order.
    pub(crate) fn new(
        position: IVec3,
        width: u32,
        height: u32,
        face: u8,
        flags: u32,
        layer: u8,
        corners: [u32; 4],
    ) -> Self {
        debug_assert!((0..32).contains(&position.x));
        debug_assert!((0..32).contains(&position.y));
        debug_assert!((0..32).contains(&position.z));
        debug_assert!((1..=32).contains(&width));
        debug_assert!((1..=32).contains(&height));
        debug_assert!(flags <= 0xF);

        let x = u32::try_from(position.x).expect("position fits");
        let y = u32::try_from(position.y).expect("position fits");
        let z = u32::try_from(position.z).expect("position fits");
        let face = u32::from(face);
        let flags = flags & 0xF;
        let layer = u32::from(layer);

        let w0 = x
            | (y << 5)
            | (z << 10)
            | ((width - 1) << 15)
            | ((height - 1) << 20)
            | (face << 25)
            | (flags << 28);

        let corner = |index: usize| Corner::unpack(corners[index]);
        let c0 = corner(0);
        let c1 = corner(1);
        let c2 = corner(2);
        let c3 = corner(3);

        let w1 = layer
            | (u32::from(c0.ao) << 8)
            | (u32::from(c1.ao) << 10)
            | (u32::from(c2.ao) << 12)
            | (u32::from(c3.ao) << 14)
            | (u32::from(c0.sky) << 16)
            | (u32::from(c0.block) << 20)
            | (u32::from(c1.sky) << 24)
            | (u32::from(c1.block) << 28);

        let w2 = u32::from(c2.sky)
            | (u32::from(c2.block) << 4)
            | (u32::from(c3.sky) << 8)
            | (u32::from(c3.block) << 12);

        Self {
            words: [w0, w1, w2],
        }
    }

    /// Reconstructs the four cell corners in the mesher's order.
    ///
    /// This mirrors what the vertex shader will do: the stored position is the
    /// underlying voxel cell, positive faces add one along their normal, and the
    /// corners span the extents along the face's `(u, v)` axes.
    pub(crate) fn corners(&self) -> [IVec3; 4] {
        let face = self.face();
        let normal = FACE_NORMALS[usize::from(face)];
        let plane = IVec3::new(normal.x.max(0), normal.y.max(0), normal.z.max(0));
        let origin = self.position() + plane;
        let (u_axis, v_axis) = face_axes(face);
        let width = i32::try_from(self.width()).expect("extent fits");
        let height = i32::try_from(self.height()).expect("extent fits");
        let mut corners = [IVec3::ZERO; 4];
        for (index, corner) in corners.iter_mut().enumerate() {
            let (u, v) = corner_uv(face, index);
            let u = i32::try_from(u).expect("bit fits");
            let v = i32::try_from(v).expect("bit fits");
            *corner = origin + u_axis * (u * width) + v_axis * (v * height);
        }
        corners
    }

    pub(crate) fn words(&self) -> [u32; 3] {
        self.words
    }

    pub(crate) fn position(&self) -> IVec3 {
        let w0 = self.words[0];
        IVec3::new(
            i32::try_from(w0 & 0x1F).expect("field fits"),
            i32::try_from((w0 >> 5) & 0x1F).expect("field fits"),
            i32::try_from((w0 >> 10) & 0x1F).expect("field fits"),
        )
    }

    pub(crate) fn width(&self) -> u32 {
        ((self.words[0] >> 15) & 0x1F) + 1
    }

    pub(crate) fn height(&self) -> u32 {
        ((self.words[0] >> 20) & 0x1F) + 1
    }

    pub(crate) fn face(&self) -> u8 {
        u8::try_from((self.words[0] >> 25) & 0x7).expect("field fits")
    }

    pub(crate) fn flags(&self) -> u32 {
        (self.words[0] >> 28) & 0xF
    }

    pub(crate) fn has_flag(&self, flag: u32) -> bool {
        self.flags() & flag != 0
    }

    pub(crate) fn layer(&self) -> u8 {
        u8::try_from(self.words[1] & 0xFF).expect("field fits")
    }

    /// Returns `(sky, block, ao)` for one corner.
    pub(crate) fn corner(&self, index: usize) -> (u8, u8, u8) {
        let (w1, w2) = (self.words[1], self.words[2]);
        let field = |word: u32, shift: u32, mask: u32| {
            u8::try_from((word >> shift) & mask).expect("field fits")
        };
        match index {
            0 => (field(w1, 16, 0xF), field(w1, 20, 0xF), field(w1, 8, 0x3)),
            1 => (field(w1, 24, 0xF), field(w1, 28, 0xF), field(w1, 10, 0x3)),
            2 => (field(w2, 0, 0xF), field(w2, 4, 0xF), field(w1, 12, 0x3)),
            3 => (field(w2, 8, 0xF), field(w2, 12, 0xF), field(w1, 14, 0x3)),
            _ => panic!("quad has four corners"),
        }
    }
}

/// Per-chunk entry in the pool's chunk table (48 bytes).
///
/// `origin` is the chunk's minimum corner in metres plus the LOD scale in `w`.
/// `water_origin` is the same corner for the water batch. `*_first` indexes the
/// flat quad array; the draw count comes from `*_quads`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub(crate) struct ChunkInfo {
    pub(crate) origin: [f32; 4],
    pub(crate) water_origin: [f32; 4],
    pub(crate) opaque_first: u32,
    pub(crate) opaque_quads: u32,
    pub(crate) water_first: u32,
    pub(crate) water_quads: u32,
}

#[derive(Clone, Copy)]
struct Corner {
    sky: u8,
    block: u8,
    ao: u8,
}

impl Corner {
    fn unpack(packed: u32) -> Self {
        Self {
            sky: u8::try_from(packed & 0xF).expect("field fits"),
            block: u8::try_from((packed >> 4) & 0xF).expect("field fits"),
            ao: u8::try_from((packed >> 8) & 0x3).expect("field fits"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::pack_light_ao;

    #[test]
    fn unit_quad_packs_to_zero() {
        let quad = QuadRecord::new(IVec3::ZERO, 1, 1, 0, 0, 0, [0; 4]);
        assert_eq!(quad.words(), [0, 0, 0]);
        assert_eq!(quad.position(), IVec3::ZERO);
        assert_eq!((quad.width(), quad.height()), (1, 1));
        assert_eq!((quad.face(), quad.flags(), quad.layer()), (0, 0, 0));
    }

    #[test]
    fn round_trips_every_field_at_its_extremes() {
        let corners = [
            pack_light_ao(15, 0, 0),
            pack_light_ao(0, 15, 1),
            pack_light_ao(7, 8, 2),
            pack_light_ao(3, 12, 3),
        ];
        let quad = QuadRecord::new(IVec3::new(31, 30, 29), 32, 31, 5, 0b1111, 255, corners);

        assert_eq!(quad.position(), IVec3::new(31, 30, 29));
        assert_eq!((quad.width(), quad.height()), (32, 31));
        assert_eq!((quad.face(), quad.flags(), quad.layer()), (5, 0b1111, 255));
        assert_eq!(quad.corner(0), (15, 0, 0));
        assert_eq!(quad.corner(1), (0, 15, 1));
        assert_eq!(quad.corner(2), (7, 8, 2));
        assert_eq!(quad.corner(3), (3, 12, 3));
    }

    #[test]
    fn corners_are_independent() {
        let corners = [
            pack_light_ao(1, 2, 0),
            pack_light_ao(3, 4, 1),
            pack_light_ao(5, 6, 2),
            pack_light_ao(7, 8, 3),
        ];
        let quad = QuadRecord::new(IVec3::ZERO, 2, 2, 0, 0, 0, corners);
        for (index, expected) in [
            (0, (1, 2, 0)),
            (1, (3, 4, 1)),
            (2, (5, 6, 2)),
            (3, (7, 8, 3)),
        ] {
            assert_eq!(quad.corner(index), expected, "corner {index}");
        }
    }

    #[test]
    fn words_match_the_documented_bit_layout() {
        let corners = [
            pack_light_ao(1, 2, 0),
            pack_light_ao(3, 4, 1),
            pack_light_ao(5, 6, 2),
            pack_light_ao(7, 8, 3),
        ];
        let quad = QuadRecord::new(IVec3::new(1, 2, 3), 4, 5, 2, 0b01, 9, corners);
        let [w0, w1, w2] = quad.words();

        assert_eq!(
            w0,
            1 | (2 << 5) | (3 << 10) | (3 << 15) | (4 << 20) | (2 << 25) | (1 << 28)
        );
        assert_eq!(
            w1,
            9 | (1 << 10) | (2 << 12) | (3 << 14) | (1 << 16) | (2 << 20) | (3 << 24) | (4 << 28)
        );
        assert_eq!(w2, 5 | (6 << 4) | (7 << 8) | (8 << 12));
        assert!(quad.has_flag(FLAG_WATER));
        assert!(!quad.has_flag(FLAG_SUBVOXEL));
    }

    #[test]
    fn chunk_info_is_forty_eight_bytes() {
        assert_eq!(std::mem::size_of::<ChunkInfo>(), 48);
        let info = ChunkInfo {
            origin: [1.0, 2.0, 3.0, 2.0],
            water_origin: [1.0, 2.0, 3.0, 1.0],
            opaque_first: 4,
            opaque_quads: 5,
            water_first: 6,
            water_quads: 7,
        };
        let bytes: &[u8] = bytemuck::bytes_of(&info);
        assert_eq!(bytes.len(), 48);
        assert_eq!(u32::from_le_bytes(bytes[32..36].try_into().unwrap()), 4);
        assert_eq!(u32::from_le_bytes(bytes[44..48].try_into().unwrap()), 7);
    }

    #[test]
    fn corners_match_the_face_table() {
        let expected: [[IVec3; 4]; 6] = [
            [
                IVec3::new(3, 3, 4),
                IVec3::new(3, 9, 4),
                IVec3::new(3, 9, 9),
                IVec3::new(3, 3, 9),
            ],
            [
                IVec3::new(2, 3, 9),
                IVec3::new(2, 9, 9),
                IVec3::new(2, 9, 4),
                IVec3::new(2, 3, 4),
            ],
            [
                IVec3::new(2, 4, 4),
                IVec3::new(2, 4, 10),
                IVec3::new(7, 4, 10),
                IVec3::new(7, 4, 4),
            ],
            [
                IVec3::new(2, 3, 10),
                IVec3::new(2, 3, 4),
                IVec3::new(7, 3, 4),
                IVec3::new(7, 3, 10),
            ],
            [
                IVec3::new(7, 3, 5),
                IVec3::new(7, 9, 5),
                IVec3::new(2, 9, 5),
                IVec3::new(2, 3, 5),
            ],
            [
                IVec3::new(2, 3, 4),
                IVec3::new(2, 9, 4),
                IVec3::new(7, 9, 4),
                IVec3::new(7, 3, 4),
            ],
        ];
        for face in 0..6 {
            let quad = QuadRecord::new(IVec3::new(2, 3, 4), 5, 6, face, 0, 0, [0; 4]);
            assert_eq!(quad.corners(), expected[usize::from(face)], "face {face}");
        }
    }

    #[test]
    fn light_ao_round_trips_through_the_packer() {
        for sky in [0, 1, 7, 15] {
            for block in [0, 2, 9, 15] {
                for ao in [0, 1, 2, 3] {
                    let packed = pack_light_ao(sky, block, ao);
                    let quad = QuadRecord::new(IVec3::ZERO, 1, 1, 0, 0, 0, [packed; 4]);
                    assert_eq!(quad.corner(0), (sky, block, ao));
                }
            }
        }
    }
}
