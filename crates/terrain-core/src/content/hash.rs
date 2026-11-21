use crate::hash::sha256;

use super::pack::ContentPack;
use super::record::BlockRecord;

/// Record kind tag for a block, the first element of the `ContentPack::pack_hash` tuple.
pub(crate) const KIND_BLOCK: u8 = 1;

impl ContentPack {
    /// Hashes the name-sorted `(kind, name, record_version, record_hash)` list (`A5.3`).
    ///
    /// Declaration order, whitespace, and comments cannot affect the result.
    /// The digest is the pin a description stores in its `CONTENT` reference.
    #[must_use]
    pub(crate) fn pack_hash(&self) -> [u8; 32] {
        let mut order: Vec<usize> = (0..self.records.len()).collect();
        order.sort_unstable_by_key(|&index| self.records[index].name.as_str());

        let mut out = Vec::with_capacity(order.len() * 96);
        for index in order {
            let record = &self.records[index];
            out.push(KIND_BLOCK);
            push_name(&mut out, &record.name);
            out.extend_from_slice(&record.record_version.to_le_bytes());
            out.extend_from_slice(&record_hash(record));
        }
        sha256(&out)
    }
}

/// Hashes one record over its canonical parsed bytes (`A5.3`).
///
/// Layout, in struct field order:
///
/// 1. `name`: `u32` byte length, then UTF-8 bytes.
/// 2. `record_version`: `u32`.
/// 3. `opacity`, `sky_atten`, `emission`: one byte each.
/// 4. `render_shape`, `render_layer`: one tag byte each.
/// 5. `collision`, `replaceable`, `liquid`: `0` or `1`.
/// 6. `hardness`: `f32` bit pattern, `-0.0` normalised to `+0.0`.
/// 7. `drops`: `u32` entry count, then each `(name, count)` in name order.
///
/// Texture references are **not** hashed (ADR-0015): they are presentation
/// state, so a re-texture cannot change [`Self`], `ContentPack::pack_hash`,
/// or a saved section's baseline.
///
/// The function is total: a record that `load` would reject still hashes its
/// bytes (a non-finite `hardness` hashes its bit pattern) rather than panicking.
/// Ids are deliberately absent; they are runtime state.
#[must_use]
pub(crate) fn record_hash(record: &BlockRecord) -> [u8; 32] {
    let mut out = Vec::with_capacity(96);
    push_name(&mut out, &record.name);
    out.extend_from_slice(&record.record_version.to_le_bytes());
    out.push(record.opacity);
    out.push(record.sky_atten);
    out.push(record.emission);
    out.push(record.render_shape.tag());
    out.push(record.render_layer.tag());
    out.push(u8::from(record.collision));
    out.push(u8::from(record.replaceable));
    out.push(u8::from(record.liquid));
    out.extend_from_slice(&canonical_f32(record.hardness).to_le_bytes());
    let count = u32::try_from(record.drops.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&count.to_le_bytes());
    for (name, dropped) in &record.drops {
        push_name(&mut out, name);
        out.push(*dropped);
    }
    sha256(&out)
}

/// Hashes a record's presentation state, which `record_hash` deliberately
/// excludes (`ADR-0015`).
///
/// Layout: each of `textures.top`, `textures.side`, `textures.bottom` as a
/// `u32` byte length followed by UTF-8 bytes. The result changes when a block
/// is re-textured and is stable across everything `record_hash` covers, so
/// visual baselines can track appearance without forking world identity.
#[must_use]
pub(crate) fn render_hash(record: &BlockRecord) -> [u8; 32] {
    let mut out = Vec::with_capacity(96);
    push_name(&mut out, &record.textures.top);
    push_name(&mut out, &record.textures.side);
    push_name(&mut out, &record.textures.bottom);
    sha256(&out)
}

/// Appends a `u32`-length-prefixed string to a canonical buffer.
fn push_name(out: &mut Vec<u8>, name: &str) {
    let length = u32::try_from(name.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(name.as_bytes());
}

/// Canonical float bits: `-0.0` becomes `+0.0`, everything else keeps its bits.
fn canonical_f32(value: f32) -> u32 {
    if value == 0.0 { 0 } else { value.to_bits() }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::content::{
        BlockRecord, ContentPack, FaceTextures, RenderLayer, RenderShape, v1_default,
    };

    #[test]
    fn pack_hash_ignores_declaration_order() {
        let base = v1_default();

        let mut reversed = base.records().to_vec();
        reversed.reverse();
        assert_eq!(
            ContentPack::load(reversed).unwrap().pack_hash(),
            base.pack_hash()
        );

        // Rotating one record to the front is the same pack too.
        let mut rotated = base.records().to_vec();
        let first = rotated.remove(0);
        rotated.push(first);
        assert_eq!(
            ContentPack::load(rotated).unwrap().pack_hash(),
            base.pack_hash()
        );

        // The hash is stable across calls and not the record hash.
        assert_eq!(base.pack_hash(), v1_default().pack_hash());
        assert_ne!(base.pack_hash(), record_hash(&base.records()[0]));
    }

    #[test]
    fn record_hash_changes_with_any_property() {
        let pack = v1_default();
        let base = pack
            .props(pack.id("core:stone").expect("core:stone is in the pack"))
            .expect("the id resolves")
            .clone();
        assert_eq!(base.name, "core:stone");
        let baseline = record_hash(&base);

        let variants = [
            BlockRecord {
                name: "core:stone_2".to_owned(),
                ..base.clone()
            },
            BlockRecord {
                record_version: 2,
                ..base.clone()
            },
            BlockRecord {
                opacity: 14,
                ..base.clone()
            },
            BlockRecord {
                sky_atten: 14,
                ..base.clone()
            },
            BlockRecord {
                emission: 1,
                ..base.clone()
            },
            BlockRecord {
                render_shape: RenderShape::CrossQuad,
                ..base.clone()
            },
            BlockRecord {
                render_layer: RenderLayer::Cutout,
                ..base.clone()
            },
            BlockRecord {
                collision: false,
                ..base.clone()
            },
            BlockRecord {
                replaceable: true,
                ..base.clone()
            },
            BlockRecord {
                liquid: true,
                ..base.clone()
            },
            BlockRecord {
                hardness: 1.6,
                ..base.clone()
            },
            BlockRecord {
                drops: BTreeMap::from([("core:dirt".to_owned(), 1)]),
                ..base.clone()
            },
        ];
        for variant in variants {
            assert_ne!(
                record_hash(&variant),
                baseline,
                "a change to {:?} must change the record hash",
                variant.name
            );
        }

        let positive_zero = BlockRecord {
            hardness: 0.0,
            ..base.clone()
        };
        let negative_zero = BlockRecord {
            hardness: -0.0,
            ..base
        };
        assert_eq!(
            record_hash(&positive_zero),
            record_hash(&negative_zero),
            "-0.0 must not be a distinct record"
        );

        // `record_hash` is total: a record that `load` would reject still
        // hashes its bytes instead of panicking.
        let pathological = BlockRecord {
            hardness: f32::NAN,
            ..positive_zero
        };
        assert_ne!(record_hash(&pathological), baseline);
    }

    #[test]
    fn texture_references_are_presentation_only() {
        let pack = v1_default();
        let base = pack
            .props(pack.id("core:grass").expect("core:grass is in the pack"))
            .expect("the id resolves")
            .clone();
        assert_eq!(base.textures.top, "core:grass_top");
        assert_eq!(base.textures.side, "core:grass_side");
        assert_eq!(base.textures.bottom, "core:dirt");

        let retextured = BlockRecord {
            textures: FaceTextures::uniform("test:retextured"),
            ..base.clone()
        };
        assert_eq!(
            record_hash(&retextured),
            record_hash(&base),
            "ADR-0015: textures must not enter the record hash"
        );
        assert_ne!(
            render_hash(&retextured),
            render_hash(&base),
            "the render hash tracks appearance"
        );

        let mut records = pack.records().to_vec();
        for record in &mut records {
            record.textures = FaceTextures::uniform("test:retextured");
        }
        let changed = ContentPack::load(records).unwrap();
        assert_eq!(
            changed.pack_hash(),
            pack.pack_hash(),
            "ADR-0015: a re-texture must not change the pack hash"
        );
        assert!(changed.check_compatible(&pack).is_ok());
    }

    #[test]
    fn v1_default_pack_hash_is_pinned() {
        let expected: [u8; 32] = [
            0x67, 0xce, 0x4c, 0x90, 0x00, 0x2b, 0xc0, 0x5f, 0x49, 0xf3, 0x1e, 0xc5, 0xde, 0xd7,
            0x4b, 0x52, 0x92, 0xac, 0xa1, 0x85, 0xcf, 0xfc, 0xb2, 0xf6, 0x24, 0xe8, 0x6e, 0x4b,
            0x01, 0xe6, 0xdd, 0x51,
        ];
        assert_eq!(v1_default().pack_hash(), expected);
    }
}
