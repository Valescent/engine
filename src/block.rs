//! Block types and their texture-array layers.

/// Number of layers in the block texture array.
pub(crate) const TEXTURE_LAYERS: u32 = 10;

/// Edge length of one block texture tile in pixels.
pub(crate) const TEXTURE_SIZE: u32 = 16;

/// A block type. The numeric values are the stable on-disk IDs.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum Block {
    Air = 0,
    Stone = 1,
    Dirt = 2,
    Grass = 3,
    Wood = 4,
    Leaves = 5,
    Lamp = 6,
    Water = 7,
}

/// Texture-array layers used by one block's top, side, and bottom faces.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FaceLayers {
    pub(crate) top: u32,
    pub(crate) side: u32,
    pub(crate) bottom: u32,
}

impl Block {
    /// Returns whether the block blocks movement and culls faces.
    pub(crate) fn is_solid(self) -> bool {
        !matches!(self, Self::Air | Self::Water)
    }

    /// Returns the face layers for a solid block, or `None` for air.
    pub(crate) fn layers(self) -> Option<FaceLayers> {
        let uniform = |layer| FaceLayers {
            top: layer,
            side: layer,
            bottom: layer,
        };
        match self {
            Self::Air => None,
            Self::Stone => Some(uniform(0)),
            Self::Dirt => Some(uniform(1)),
            Self::Grass => Some(FaceLayers {
                top: 2,
                side: 3,
                bottom: 4,
            }),
            Self::Wood => Some(FaceLayers {
                top: 5,
                side: 6,
                bottom: 5,
            }),
            Self::Leaves => Some(uniform(7)),
            Self::Lamp => Some(uniform(8)),
            Self::Water => Some(uniform(9)),
        }
    }

    /// Converts a stable on-disk block ID.
    pub(crate) fn from_id(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::Air),
            1 => Some(Self::Stone),
            2 => Some(Self::Dirt),
            3 => Some(Self::Grass),
            4 => Some(Self::Wood),
            5 => Some(Self::Leaves),
            6 => Some(Self::Lamp),
            7 => Some(Self::Water),
            _ => None,
        }
    }

    /// Returns the stable on-disk block ID.
    pub(crate) fn id(self) -> u8 {
        self as u8
    }

    /// Returns the block selected by a `1`-based hotbar slot.
    pub(crate) fn from_hotbar(slot: usize) -> Option<Self> {
        match slot {
            1 => Some(Self::Stone),
            2 => Some(Self::Dirt),
            3 => Some(Self::Grass),
            4 => Some(Self::Wood),
            5 => Some(Self::Leaves),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_ids_are_stable() {
        let blocks = [
            Block::Air,
            Block::Stone,
            Block::Dirt,
            Block::Grass,
            Block::Wood,
            Block::Leaves,
            Block::Lamp,
            Block::Water,
        ];

        for (id, block) in blocks.into_iter().enumerate() {
            assert_eq!(block.id(), u8::try_from(id).unwrap());
            assert_eq!(Block::from_id(block.id()), Some(block));
        }
        assert_eq!(Block::from_id(8), None);
    }

    #[test]
    fn only_air_and_water_are_not_solid() {
        assert!(!Block::Air.is_solid());
        assert!(!Block::Water.is_solid());
        for block in [
            Block::Stone,
            Block::Dirt,
            Block::Grass,
            Block::Wood,
            Block::Leaves,
            Block::Lamp,
        ] {
            assert!(block.is_solid());
        }
    }

    #[test]
    fn air_has_no_layers() {
        assert_eq!(Block::Air.layers(), None);
    }

    #[test]
    fn face_layers_match_the_spec_table() {
        let expected = [
            (Block::Stone, 0, 0, 0),
            (Block::Dirt, 1, 1, 1),
            (Block::Grass, 2, 3, 4),
            (Block::Wood, 5, 6, 5),
            (Block::Leaves, 7, 7, 7),
            (Block::Lamp, 8, 8, 8),
            (Block::Water, 9, 9, 9),
        ];

        for (block, top, side, bottom) in expected {
            assert_eq!(
                block.layers(),
                Some(FaceLayers { top, side, bottom }),
                "{block:?}"
            );
        }
    }

    #[test]
    fn hotbar_maps_every_selectable_block() {
        assert_eq!(Block::from_hotbar(0), None);
        assert_eq!(Block::from_hotbar(1), Some(Block::Stone));
        assert_eq!(Block::from_hotbar(5), Some(Block::Leaves));
        assert_eq!(Block::from_hotbar(6), None);
        assert!((1..=5).all(|slot| Block::from_hotbar(slot).is_some_and(Block::is_solid)));
    }
}
