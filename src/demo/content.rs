//! The demo's content tables: the mesher material table and the collision
//! table, both derived from the built-in v1 content pack.
//!
//! The v1 pack carries no texture references yet, so [`demo_texture_layer`]
//! maps stable block names onto the renderer's procedural texture array as a
//! placeholder.

use crate::content;
use crate::mesher2;

/// `solid[id]` from the pack's collision flag (`A5.2`).
pub(crate) fn solid_table(pack: &content::ContentPack) -> Vec<bool> {
    let mut solid = vec![false; pack.len()];
    for record in pack.records() {
        if let Some(id) = pack.id(&record.name) {
            solid[id as usize] = record.collision;
        }
    }
    solid
}

/// The mesher material table for the built-in v1 content pack.
///
/// Ids come from [`content::v1_default`], the same pack [`pipeline::Terrain::new`]
/// resolves, so sections and this table always agree. The pack has no texture
/// references yet, so [`demo_texture_layer`] maps names onto the renderer's
/// procedural array as a placeholder.
pub(crate) fn demo_materials() -> mesher2::MaterialProps {
    let pack = content::v1_default();
    let mut props = mesher2::MaterialProps::new();
    for record in pack.records() {
        let id = pack.id(&record.name).expect("every pack record has an id");
        props.set(
            id,
            mesher2::Material {
                solid: record.collision,
                opaque: record.opacity >= content::LIGHT_LEVELS,
                water: record.liquid,
                cross_quad: record.render_shape == content::RenderShape::CrossQuad,
                texture_top: demo_texture_layer(&record.textures.top),
                texture_side: demo_texture_layer(&record.textures.side),
                texture_bottom: demo_texture_layer(&record.textures.bottom),
                tint: 0,
                emissive: record.emission > 0,
            },
        );
    }
    props
}

/// Placeholder texture-array layer for a stable block name.
///
/// The v1 pack carries no texture refs (W22 note), so this is the only place the
/// demo maps names onto `renderer.rs`'s eleven procedural layers.
/// Maps a stable texture reference onto the renderer's procedural array.
///
/// The array is still placeholder art (`ADR-0007`, `ADR-0015`): a reference is
/// resolved by name, and an unknown name falls back to stone. Grass and wood
/// are the only blocks with three distinct faces today.
pub(crate) fn demo_texture_layer(name: &str) -> u16 {
    match name {
        "core:dirt" | "core:clay" | "core:grass_bottom" => 1,
        "core:grass_top" => 2,
        "core:grass_side" => 3,
        "core:wood_top" => 5,
        "core:wood_side" => 6,
        "core:leaves" | "core:plant" => 7,
        "core:lamp" => 8,
        "core:water" | "core:ice" => 9,
        "core:sand" | "core:snow" => 10,
        // Stone, bedrock, boulder, gravel, ores, and anything unknown.
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use crate::content;

    use super::*;

    #[test]
    fn demo_materials_map_each_face_from_the_pack() {
        let pack = content::v1_default();
        let props = demo_materials();
        let tiles = |name: &str| {
            let material = props.material(pack.id(name).expect("the pack holds the block"));
            (
                material.texture_top,
                material.texture_side,
                material.texture_bottom,
            )
        };

        assert_eq!(tiles("core:grass"), (2, 3, 1));
        assert_eq!(tiles("core:wood"), (5, 6, 5));
        assert_eq!(tiles("core:stone"), (0, 0, 0));
        assert_eq!(tiles("core:sand"), (10, 10, 10));
    }
}
