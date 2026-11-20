//! Content pack: stable names, runtime ids, per-record hashes (`A5`).
//!
//! `A5.1`–`A5.4`. The parsed pack is the **sole authority** for block
//! properties (`A5.2`): nothing downstream may carry a second copy of a
//! property or attach behaviour to a number. Code resolves a block once at
//! startup by name (`pack.id("core:torch")`) or by tag, never by comparing
//! strings in a hot path.
//!
//! # Identity
//!
//! Stable names are what a save file, an edit record, and a description pin.
//! [`BlockId`]s are **derived at load** by sorting records by name (`A5.1`), so
//! one pack yields the same ids on every run, and two packs that differ only in
//! declaration order produce identical ids. Ids are never saved, hashed, or
//! reported; they are an in-memory index into [`ContentPack::records`].
//!
//! [`record_hash`] covers canonical parsed bytes (`A5.3`): struct field order,
//! little-endian fixed-width integers, length-prefixed strings, sorted map
//! entries, and `-0.0` normalised to `+0.0`. Whitespace, comments, and file
//! order never enter it. [`ContentPack::pack_hash`] hashes the name-sorted
//! `(kind, name, record_version, record_hash)` list, which is what a
//! description pins through its `CONTENT` reference.
//!
//! # Placeholder pack
//!
//! `OPEN(content ids)` the real pack does not exist yet. [`v1_default`] is a
//! minimal placeholder for the thirteen block ids the pipeline emits today
//! (`core:air` … `core:plant`) plus the five material ids `§2.6` and `§8.3`
//! need: `core:bedrock`, `core:clay`, `core:gravel`, `core:ice`, and
//! `core:snow`. It exists so lighting, meshing, the column pipeline, and the
//! surface rules have properties to read; it is not an authored pack, and its
//! `hardness` and `drops` values are invented. Because `A5.1` derives ids by
//! name, its numbering is **not** the legacy numbering that `pipeline`,
//! `light2`, and `mesher2` still hard-code; those constants are the pre-pack
//! placeholders and must be replaced by name lookups.
//!
//! `T1.5` makes the real pack additive only: a property change is a new record
//! version, never an edit in place, and removal is deprecation.

use std::collections::BTreeMap;

use crate::error::{ErrorKind, Stage, TerrainError};
use crate::hash::sha256;
use crate::section::BlockId;

/// First runtime id reserved for an unknown saved name (`T1.6`).
///
/// A saved name missing from the pack allocates one id at or above this base,
/// stays opaque, collidable, hardness-high, and drop-free, and keeps its
/// original name on disk. A pack may therefore never fill this range: runtime
/// ids below it are real content.
pub const UNKNOWN_BASE: BlockId = 0xF000;

/// Most records one pack may hold, so no real id reaches [`UNKNOWN_BASE`].
///
/// `MAX_RECORDS <= UNKNOWN_BASE` is asserted by a test, not by a cast, so the
/// constant stays usable in array bounds and comparisons.
pub const MAX_RECORDS: usize = 0xF000;

/// Longest legal stable name in bytes (`A5.1`).
pub const MAX_NAME_BYTES: usize = 64;

/// Highest light level; the range of `opacity`, `sky_atten`, and `emission`.
pub const LIGHT_LEVELS: u8 = 15;

/// Skylight lost per water block (`L2.4`).
pub const WATER_SKY_ATTEN: u8 = 1;

/// Light a lamp emits in the placeholder pack (`L2.1`).
pub const LAMP_EMISSION: u8 = 14;

/// Record kind tag for a block, the first element of the [`ContentPack::pack_hash`] tuple.
pub const KIND_BLOCK: u8 = 1;

/// Skylight attenuation reported for an id that the pack does not hold.
///
/// Fail safe, like `light2`'s opaque fallback: an unknown block can under-light
/// a scene, never leak light through geometry. Callers that care about the
/// difference use [`ContentPack::props`], which returns `None`.
pub const UNKNOWN_SKY_ATTEN: u8 = LIGHT_LEVELS;

/// How the mesher draws a block (`H4.2`).
///
/// Closed on purpose: an unknown manifest string is a typed error, never a
/// silent `Cube`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum RenderShape {
    /// A full cube; greedy-mergeable and opaque or translucent.
    Cube,
    /// Two crossed quads: no collision, no occlusion, no skylight block.
    CrossQuad,
    /// A partial-height snow layer on top of the block below.
    SnowLayer,
}

impl RenderShape {
    /// Parses the manifest spelling.
    ///
    /// # Errors
    ///
    /// `DescriptionInvalid { key: "CONTENT", rule: "render_shape" }` for any
    /// string outside `cube`, `cross_quad`, `snow_layer`.
    pub fn from_name(name: &str) -> Result<Self, TerrainError> {
        match name {
            "cube" => Ok(Self::Cube),
            "cross_quad" => Ok(Self::CrossQuad),
            "snow_layer" => Ok(Self::SnowLayer),
            _ => Err(content_error("render_shape")),
        }
    }

    /// The canonical manifest spelling.
    #[must_use]
    pub const fn as_name(self) -> &'static str {
        match self {
            Self::Cube => "cube",
            Self::CrossQuad => "cross_quad",
            Self::SnowLayer => "snow_layer",
        }
    }

    /// Stable canonical tag; part of [`record_hash`].
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::Cube => 0,
            Self::CrossQuad => 1,
            Self::SnowLayer => 2,
        }
    }
}

/// Which render pass a block belongs to (`M2.1`).
///
/// Closed on purpose: an unknown manifest string is a typed error, never a
/// silent `Opaque`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum RenderLayer {
    /// The opaque pass.
    Opaque,
    /// The alpha-tested cutout pass.
    Cutout,
    /// The blended water pass, drawn after opaque and cutout.
    Water,
}

impl RenderLayer {
    /// Parses the manifest spelling.
    ///
    /// # Errors
    ///
    /// `DescriptionInvalid { key: "CONTENT", rule: "render_layer" }` for any
    /// string outside `opaque`, `cutout`, `water`.
    pub fn from_name(name: &str) -> Result<Self, TerrainError> {
        match name {
            "opaque" => Ok(Self::Opaque),
            "cutout" => Ok(Self::Cutout),
            "water" => Ok(Self::Water),
            _ => Err(content_error("render_layer")),
        }
    }

    /// The canonical manifest spelling.
    #[must_use]
    pub const fn as_name(self) -> &'static str {
        match self {
            Self::Opaque => "opaque",
            Self::Cutout => "cutout",
            Self::Water => "water",
        }
    }

    /// Stable canonical tag; part of [`record_hash`].
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::Opaque => 0,
            Self::Cutout => 1,
            Self::Water => 2,
        }
    }
}

/// One block definition from the content pack (`A5.2`).
///
/// Field order **is** the canonical hash order; see [`record_hash`]. Every
/// field is required: there are no defaults, because a default that differs
/// from the author's intent is a silent world change.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockRecord {
    /// Stable `namespace:name`, lowercase `[a-z0-9_]`, at most
    /// [`MAX_NAME_BYTES`] bytes, immutable once shipped (`A5.1`).
    pub name: String,
    /// Version of this record's property set; `0` is invalid (`T1.5`).
    pub record_version: u32,
    /// Light lost per step through the block, `0..=`[`LIGHT_LEVELS`].
    pub opacity: u8,
    /// Extra skylight lost per step; `0` except for water (`L2.4`).
    pub sky_atten: u8,
    /// Light emitted, `0..=`[`LIGHT_LEVELS`] (`L2.1`).
    pub emission: u8,
    /// Mesh shape (`H4.2`).
    pub render_shape: RenderShape,
    /// Render pass (`M2.1`).
    pub render_layer: RenderLayer,
    /// Whether the block is solid for collision and span scans.
    pub collision: bool,
    /// Whether a placement may replace it in one edit (`H4.2`).
    pub replaceable: bool,
    /// Whether the block is a liquid, for flow and water surfaces (`D7.1`).
    pub liquid: bool,
    /// Mining resistance in seconds; finite and non-negative.
    pub hardness: f32,
    /// Drop name → count, sorted by name by construction. Empty means nothing
    /// drops, as for air and water.
    pub drops: BTreeMap<String, u8>,
}

/// A parsed content pack, indexed by stable name and by runtime id.
///
/// `records` is kept in ascending name order, so `id` is also the index of the
/// record and every accessor is one array read (`A5.2`). Construct it through
/// [`ContentPack::load`] (or [`v1_default`]) so that invariant holds.
#[derive(Clone, Debug, PartialEq)]
pub struct ContentPack {
    records: Vec<BlockRecord>,
    ids: BTreeMap<String, BlockId>,
}

impl ContentPack {
    /// Validates, sorts, and indexes a pack (`A5.1`).
    ///
    /// Records are sorted by name and ids are assigned `0, 1, 2, …` in that
    /// order, so ids are a pure function of the name set.
    ///
    /// # Errors
    ///
    /// `DescriptionInvalid { key: "CONTENT" }`, with the `rule` naming the first
    /// offender, when:
    ///
    /// - the pack holds more than [`MAX_RECORDS`] records;
    /// - a name is empty, longer than [`MAX_NAME_BYTES`], outside the charset,
    ///   or not `namespace:name`;
    /// - a name is declared twice (which would also duplicate its id);
    /// - `record_version` is `0`;
    /// - `opacity`, `sky_atten`, or `emission` exceeds [`LIGHT_LEVELS`];
    /// - `hardness` is negative or not finite;
    /// - a drop name is invalid or its count is `0`.
    ///
    /// Records that pass validation are never repaired or defaulted.
    pub fn load(mut records: Vec<BlockRecord>) -> Result<Self, TerrainError> {
        if records.len() > MAX_RECORDS {
            return Err(content_error("record count"));
        }
        for record in &records {
            validate(record)?;
        }

        records.sort_by(|left, right| left.name.cmp(&right.name));
        for pair in records.windows(2) {
            if pair[0].name == pair[1].name {
                return Err(content_error("duplicate name"));
            }
        }

        let mut ids = BTreeMap::new();
        for (index, record) in records.iter().enumerate() {
            let id = u16::try_from(index).map_err(|_| content_error("record count"))?;
            // Ids derive from names, so this can only fire if the two are ever
            // decoupled; keeping the check makes the invariant explicit.
            if ids.insert(record.name.clone(), id).is_some() {
                return Err(content_error("duplicate id"));
            }
        }

        Ok(Self { records, ids })
    }

    /// The records, in ascending name order.
    #[must_use]
    pub fn records(&self) -> &[BlockRecord] {
        &self.records
    }

    /// Number of records in the pack.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether the pack holds no records.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Resolves a stable name to its runtime id (`A5.2`).
    #[must_use]
    pub fn id(&self, name: &str) -> Option<BlockId> {
        self.ids.get(name).copied()
    }

    /// The stable name for a runtime id, or `None` when the id is not in the pack.
    #[must_use]
    pub fn name(&self, id: BlockId) -> Option<&str> {
        self.records
            .get(usize::from(id))
            .map(|record| record.name.as_str())
    }

    /// The properties of one runtime id (`A5.2`).
    ///
    /// `None` for an id outside the pack, including the [`UNKNOWN_BASE`]
    /// placeholder range.
    #[must_use]
    pub fn props(&self, id: BlockId) -> Option<&BlockRecord> {
        self.records.get(usize::from(id))
    }

    /// Skylight attenuation for one id; [`UNKNOWN_SKY_ATTEN`] outside the pack.
    #[must_use]
    pub fn sky_atten(&self, id: BlockId) -> u8 {
        self.props(id)
            .map_or(UNKNOWN_SKY_ATTEN, |record| record.sky_atten)
    }

    /// Light emission for one id; `0` outside the pack.
    #[must_use]
    pub fn emission(&self, id: BlockId) -> u8 {
        self.props(id).map_or(0, |record| record.emission)
    }

    /// Hashes the name-sorted `(kind, name, record_version, record_hash)` list (`A5.3`).
    ///
    /// Declaration order, whitespace, and comments cannot affect the result.
    /// The digest is the pin a description stores in its `CONTENT` reference.
    #[must_use]
    pub fn pack_hash(&self) -> [u8; 32] {
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

    /// Refuses generation against a pack that does not match `pinned` exactly (`A5.4`).
    ///
    /// Every pinned record must be present with the same `record_version` and
    /// the same [`record_hash`]. Extra records in `self` are ignored: a
    /// superset is fine, a subset is not. Missing, changed, or re-versioned
    /// records are all `DescriptionInvalid { key: "CONTENT" }`, with the rule
    /// naming which case fired.
    ///
    /// This is the *generation* check only. Opening saved terrain is allowed
    /// with missing names, which degrade to [`UNKNOWN_BASE`] placeholders
    /// (`T1.6`), and never regenerates against a subset.
    ///
    /// # Errors
    ///
    /// `DescriptionInvalid { key: "CONTENT" }` for the first pinned record that
    /// is missing, changed, or has a different `record_version`.
    pub fn check_compatible(&self, pinned: &ContentPack) -> Result<(), TerrainError> {
        for expected in &pinned.records {
            let Some(id) = self.id(&expected.name) else {
                return Err(content_error("missing"));
            };
            let Some(actual) = self.props(id) else {
                return Err(content_error("missing"));
            };
            if actual.record_version != expected.record_version {
                return Err(content_error("record_version"));
            }
            if record_hash(actual) != record_hash(expected) {
                return Err(content_error("changed"));
            }
        }
        Ok(())
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
/// The function is total: a record that `load` would reject still hashes its
/// bytes (a non-finite `hardness` hashes its bit pattern) rather than panicking.
/// Ids are deliberately absent; they are runtime state.
#[must_use]
pub fn record_hash(record: &BlockRecord) -> [u8; 32] {
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

/// The placeholder pack for the ids the pipeline emits (`OPEN(content ids)`).
///
/// Eighteen records: the thirteen blocks the column pipeline, mesher, and
/// lighting currently know about (air, boulder, dirt, grass, lamp, leaves,
/// copper ore, iron ore, plant, sand, stone, water, wood) plus the five frozen
/// materials the answered spec names (`§2.6`, `§8.3`): bedrock, clay, gravel,
/// ice, and snow. They are **not** an authored pack: `hardness` and `drops` are
/// plausible placeholders, every `record_version` is `1`, and the numbering
/// follows `A5.1` (sorted by name), not the legacy constants in
/// `pipeline`/`light2`/`mesher2`.
///
/// `core:bedrock` is opaque, collidable, drops nothing, and carries a hardness
/// high enough that normal play cannot break it. The four other new materials
/// are ordinary opaque, collidable solids that drop themselves.
///
/// # Panics
///
/// Only if this hard-coded table stops being a valid pack, which is a bug in
/// this module, not bad input. A test builds it on every run.
#[must_use]
pub fn v1_default() -> ContentPack {
    // Already in name order; `load` re-sorts so the order here is documentation.
    let records = vec![
        air("core:air"),
        opaque("core:bedrock", 100.0),
        self_dropping(opaque("core:boulder", 2.5)),
        self_dropping(opaque("core:clay", 0.6)),
        self_dropping(opaque("core:dirt", 0.5)),
        self_dropping(opaque("core:grass", 0.6)),
        self_dropping(opaque("core:gravel", 0.6)),
        self_dropping(opaque("core:ice", 0.5)),
        lamp("core:lamp"),
        self_dropping(leaves("core:leaves")),
        self_dropping(opaque("core:ore_copper", 3.0)),
        self_dropping(opaque("core:ore_iron", 3.0)),
        self_dropping(plant("core:plant")),
        self_dropping(opaque("core:sand", 0.5)),
        self_dropping(opaque("core:snow", 0.2)),
        self_dropping(opaque("core:stone", 1.5)),
        water("core:water"),
        self_dropping(opaque("core:wood", 2.0)),
    ];
    ContentPack::load(records).expect("the built-in v1 placeholder pack is valid")
}

/// Placeholder record version for every [`v1_default`] entry.
const RECORD_VERSION: u32 = 1;

/// The default shape of a solid, opaque cube.
fn opaque(name: &str, hardness: f32) -> BlockRecord {
    BlockRecord {
        name: name.to_owned(),
        record_version: RECORD_VERSION,
        opacity: LIGHT_LEVELS,
        sky_atten: LIGHT_LEVELS,
        emission: 0,
        render_shape: RenderShape::Cube,
        render_layer: RenderLayer::Opaque,
        collision: true,
        replaceable: false,
        liquid: false,
        hardness,
        drops: BTreeMap::new(),
    }
}

/// Air: clear, walk-through, replaceable.
fn air(name: &str) -> BlockRecord {
    BlockRecord {
        opacity: 0,
        sky_atten: 0,
        collision: false,
        replaceable: true,
        ..opaque(name, 0.0)
    }
}

/// Leaves: cutout, dims block light slightly, still collidable.
fn leaves(name: &str) -> BlockRecord {
    BlockRecord {
        opacity: 1,
        sky_atten: 0,
        render_layer: RenderLayer::Cutout,
        ..opaque(name, 0.2)
    }
}

/// Lamp: opaque to skylight, emits [`LAMP_EMISSION`] (`L2.1`).
fn lamp(name: &str) -> BlockRecord {
    BlockRecord {
        emission: LAMP_EMISSION,
        render_layer: RenderLayer::Cutout,
        ..opaque(name, 0.3)
    }
}

/// Water: skylight-attenuating liquid with no collision (`L2.4`, `D7.1`).
fn water(name: &str) -> BlockRecord {
    BlockRecord {
        opacity: 0,
        sky_atten: WATER_SKY_ATTEN,
        render_layer: RenderLayer::Water,
        collision: false,
        liquid: true,
        ..opaque(name, 0.0)
    }
}

/// Scatter: a cross-quad with no collision and no occlusion (`H4.2`).
fn plant(name: &str) -> BlockRecord {
    BlockRecord {
        opacity: 0,
        sky_atten: 0,
        render_shape: RenderShape::CrossQuad,
        render_layer: RenderLayer::Cutout,
        collision: false,
        replaceable: true,
        ..opaque(name, 0.0)
    }
}

/// Makes a record drop one of itself, the placeholder drop rule.
fn self_dropping(mut record: BlockRecord) -> BlockRecord {
    record.drops.insert(record.name.clone(), 1);
    record
}

/// Builds the typed error for a content rule (`A5.4` adds no error variant).
fn content_error(rule: &'static str) -> TerrainError {
    TerrainError::new(
        Stage::Description,
        ErrorKind::DescriptionInvalid {
            key: "CONTENT",
            rule,
        },
    )
}

/// Checks one record's ranges and name (`A5.1`, `A5.2`).
fn validate(record: &BlockRecord) -> Result<(), TerrainError> {
    validate_name(&record.name)?;
    if record.record_version == 0 {
        return Err(content_error("record_version"));
    }
    if record.opacity > LIGHT_LEVELS {
        return Err(content_error("opacity"));
    }
    if record.sky_atten > LIGHT_LEVELS {
        return Err(content_error("sky_atten"));
    }
    if record.emission > LIGHT_LEVELS {
        return Err(content_error("emission"));
    }
    if !record.hardness.is_finite() || record.hardness < 0.0 {
        return Err(content_error("hardness"));
    }
    for (name, count) in &record.drops {
        validate_name(name)?;
        if *count == 0 {
            return Err(content_error("drop count"));
        }
    }
    Ok(())
}

/// Checks the `A5.1` stable-name grammar: `namespace:name`, lowercase.
fn validate_name(name: &str) -> Result<(), TerrainError> {
    if name.is_empty() {
        return Err(content_error("missing name"));
    }
    if name.len() > MAX_NAME_BYTES {
        return Err(content_error("name length"));
    }
    if !name.bytes().all(is_name_byte) {
        return Err(content_error("name charset"));
    }
    let mut parts = name.split(':');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(namespace), Some(local), None) if !namespace.is_empty() && !local.is_empty() => {
            Ok(())
        }
        _ => Err(content_error("name namespace:name")),
    }
}

/// Bytes the stable-name grammar allows (`A5.1`): lowercase, digits, `_`, `:`.
const fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b':'
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
    use std::collections::BTreeSet;

    use super::*;

    /// Every name and id in the placeholder pack (`A5.1`), in id order.
    ///
    /// Ids are derived from the name sort, so adding the five `§2.6`/`§8.3`
    /// materials renumbers the records that sort after them. Only the stable
    /// names and properties are contract; the numeric ids are runtime state.
    const V1_IDS: [(&str, BlockId); 18] = [
        ("core:air", 0),
        ("core:bedrock", 1),
        ("core:boulder", 2),
        ("core:clay", 3),
        ("core:dirt", 4),
        ("core:grass", 5),
        ("core:gravel", 6),
        ("core:ice", 7),
        ("core:lamp", 8),
        ("core:leaves", 9),
        ("core:ore_copper", 10),
        ("core:ore_iron", 11),
        ("core:plant", 12),
        ("core:sand", 13),
        ("core:snow", 14),
        ("core:stone", 15),
        ("core:water", 16),
        ("core:wood", 17),
    ];

    fn error_parts(error: &TerrainError) -> (&'static str, &'static str) {
        match &error.kind {
            ErrorKind::DescriptionInvalid { key, rule } => (key, rule),
            other => panic!("expected DescriptionInvalid, found {other:?}"),
        }
    }

    #[test]
    fn names_sort_into_stable_ids() {
        let pack = v1_default();

        // Declaration order is irrelevant: the same records reversed give the
        // same pack, ids included.
        let mut declarations = pack.records().to_vec();
        declarations.reverse();
        let reversed = ContentPack::load(declarations).unwrap();
        assert_eq!(reversed, pack);
        assert_eq!(reversed.pack_hash(), pack.pack_hash());

        // Ids are dense and follow the name sort, so the id is the index.
        let mut previous: Option<&str> = None;
        for (index, record) in pack.records().iter().enumerate() {
            assert_eq!(
                pack.id(&record.name),
                Some(u16::try_from(index).unwrap()),
                "{}",
                record.name
            );
            if let Some(previous) = previous {
                assert!(previous < record.name.as_str(), "name order");
            }
            previous = Some(record.name.as_str());
        }

        // A second build agrees.
        assert_eq!(v1_default(), pack);
        assert_eq!(v1_default().ids, pack.ids);
    }

    #[test]
    fn duplicate_name_and_id_are_rejected() {
        let mut records = v1_default().records().to_vec();
        records.push(records[0].clone());
        let error = ContentPack::load(records).unwrap_err();
        assert_eq!(error_parts(&error), ("CONTENT", "duplicate name"));

        // Ids derive from names, so a well-formed pack can never hold a
        // duplicated id; assert the derived set is exact.
        let pack = v1_default();
        let ids: BTreeSet<BlockId> = pack
            .records()
            .iter()
            .filter_map(|record| pack.id(&record.name))
            .collect();
        assert_eq!(ids.len(), pack.len());
        assert_eq!(
            ids.into_iter().max(),
            Some(u16::try_from(pack.len() - 1).unwrap())
        );
    }

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
    fn generation_requires_an_exact_match() {
        let pinned = v1_default();
        assert!(pinned.check_compatible(&pinned).is_ok());

        let mut changed_records = pinned.records().to_vec();
        changed_records[0].emission = 1;
        let changed = ContentPack::load(changed_records).unwrap();
        let error = changed.check_compatible(&pinned).unwrap_err();
        assert_eq!(error_parts(&error), ("CONTENT", "changed"));

        let mut bumped_records = pinned.records().to_vec();
        bumped_records[0].record_version = 2;
        let bumped = ContentPack::load(bumped_records).unwrap();
        let error = bumped.check_compatible(&pinned).unwrap_err();
        assert_eq!(error_parts(&error), ("CONTENT", "record_version"));

        let mut subset_records = pinned.records().to_vec();
        subset_records.remove(0);
        let subset = ContentPack::load(subset_records).unwrap();
        let error = subset.check_compatible(&pinned).unwrap_err();
        assert_eq!(error_parts(&error), ("CONTENT", "missing"));

        // The check is directional: a subset accepts the full pack.
        assert!(pinned.check_compatible(&subset).is_ok());
    }

    #[test]
    fn extra_records_are_allowed() {
        let pinned = v1_default();
        let mut records = pinned.records().to_vec();
        records.push(opaque("extra:test", 1.0));
        let superset = ContentPack::load(records).unwrap();

        assert!(superset.check_compatible(&pinned).is_ok());
        assert_eq!(superset.len(), pinned.len() + 1);
        assert!(superset.id("extra:test").is_some());
        assert_ne!(superset.pack_hash(), pinned.pack_hash());
    }

    #[test]
    fn out_of_range_attenuation_is_rejected() {
        let mut sky = v1_default().records().to_vec();
        sky[0].sky_atten = LIGHT_LEVELS + 1;
        let error = ContentPack::load(sky).unwrap_err();
        assert_eq!(error_parts(&error), ("CONTENT", "sky_atten"));

        let mut emission = v1_default().records().to_vec();
        emission[0].emission = LIGHT_LEVELS + 1;
        let error = ContentPack::load(emission).unwrap_err();
        assert_eq!(error_parts(&error), ("CONTENT", "emission"));

        let mut opacity = v1_default().records().to_vec();
        opacity[0].opacity = LIGHT_LEVELS + 1;
        let error = ContentPack::load(opacity).unwrap_err();
        assert_eq!(error_parts(&error), ("CONTENT", "opacity"));
    }

    #[test]
    fn v1_default_ids_are_stable() {
        let pack = v1_default();
        assert_eq!(pack.len(), V1_IDS.len());
        assert!(!pack.is_empty());

        for (name, id) in V1_IDS {
            assert_eq!(pack.id(name), Some(id), "{name}");
            assert_eq!(
                pack.props(id).map(|record| record.name.as_str()),
                Some(name),
                "id {id}"
            );
        }

        // `A5.2` accessors read the packed record; ids outside the pack are
        // total and fail safe.
        let water = pack.id("core:water").unwrap();
        assert_eq!(pack.sky_atten(water), WATER_SKY_ATTEN);
        assert_eq!(pack.emission(pack.id("core:lamp").unwrap()), LAMP_EMISSION);
        assert_eq!(pack.sky_atten(UNKNOWN_BASE), UNKNOWN_SKY_ATTEN);
        assert_eq!(pack.emission(UNKNOWN_BASE), 0);
        assert!(pack.props(UNKNOWN_BASE).is_none());
    }

    #[test]
    fn answered_material_records_are_present_and_solid() {
        let pack = v1_default();

        // `§2.6`/`§8.3`: the five materials the answered spec names are ordinary
        // opaque, collidable solids and none is water. Bedrock is unbreakable by
        // normal play, which the pack expresses as a high hardness and no drops.
        let materials = [
            "core:bedrock",
            "core:clay",
            "core:gravel",
            "core:ice",
            "core:snow",
        ];
        for name in materials {
            let id = pack.id(name).unwrap_or_else(|| panic!("{name} is missing"));
            let record = pack.props(id).expect("a resolved id has a record");
            assert_eq!(record.record_version, RECORD_VERSION, "{name}");
            assert_eq!(record.opacity, LIGHT_LEVELS, "{name} must be opaque");
            assert_eq!(record.sky_atten, LIGHT_LEVELS, "{name} must block sky");
            assert!(record.collision, "{name} must collide");
            assert!(!record.liquid, "{name} must not be water");
            assert!(!record.replaceable, "{name} must not be replaceable");
        }

        let bedrock = pack
            .props(pack.id("core:bedrock").expect("bedrock is in the pack"))
            .expect("the id resolves");
        assert!(bedrock.hardness >= 100.0, "bedrock must be unbreakable");
        assert!(bedrock.drops.is_empty(), "bedrock must drop nothing");

        for name in ["core:clay", "core:gravel", "core:ice", "core:snow"] {
            let record = pack
                .props(pack.id(name).expect("the material is in the pack"))
                .expect("the id resolves");
            assert_eq!(
                record.drops.keys().map(String::as_str).collect::<Vec<_>>(),
                vec![name],
                "{name} must drop itself"
            );
        }
    }

    #[test]
    fn unknown_shape_and_layer_names_are_rejected() {
        let error = RenderShape::from_name("sphere").unwrap_err();
        assert_eq!(error_parts(&error), ("CONTENT", "render_shape"));
        let error = RenderLayer::from_name("transparent").unwrap_err();
        assert_eq!(error_parts(&error), ("CONTENT", "render_layer"));

        for shape in [
            RenderShape::Cube,
            RenderShape::CrossQuad,
            RenderShape::SnowLayer,
        ] {
            assert_eq!(RenderShape::from_name(shape.as_name()), Ok(shape));
        }
        for layer in [RenderLayer::Opaque, RenderLayer::Cutout, RenderLayer::Water] {
            assert_eq!(RenderLayer::from_name(layer.as_name()), Ok(layer));
        }
        assert_eq!(RenderShape::Cube.tag(), 0);
        assert_eq!(RenderLayer::Water.tag(), 2);
    }

    #[test]
    fn invalid_records_are_rejected() {
        let pack = v1_default();
        let record = pack
            .props(pack.id("core:stone").expect("core:stone is in the pack"))
            .expect("the id resolves")
            .clone();

        let mut empty_name = record.clone();
        empty_name.name = String::new();
        assert_eq!(
            error_parts(&ContentPack::load(vec![empty_name]).unwrap_err()),
            ("CONTENT", "missing name")
        );

        let mut long_name = record.clone();
        long_name.name = "core:".to_owned() + &"a".repeat(MAX_NAME_BYTES);
        assert_eq!(
            error_parts(&ContentPack::load(vec![long_name]).unwrap_err()),
            ("CONTENT", "name length")
        );

        let mut uppercase = record.clone();
        uppercase.name = "Core:Stone".to_owned();
        assert_eq!(
            error_parts(&ContentPack::load(vec![uppercase]).unwrap_err()),
            ("CONTENT", "name charset")
        );

        let mut no_namespace = record.clone();
        no_namespace.name = "stone".to_owned();
        assert_eq!(
            error_parts(&ContentPack::load(vec![no_namespace]).unwrap_err()),
            ("CONTENT", "name namespace:name")
        );

        let mut version_zero = record.clone();
        version_zero.record_version = 0;
        assert_eq!(
            error_parts(&ContentPack::load(vec![version_zero]).unwrap_err()),
            ("CONTENT", "record_version")
        );

        let mut negative_hardness = record.clone();
        negative_hardness.hardness = -1.0;
        assert_eq!(
            error_parts(&ContentPack::load(vec![negative_hardness]).unwrap_err()),
            ("CONTENT", "hardness")
        );

        let mut not_finite = record.clone();
        not_finite.hardness = f32::NAN;
        assert_eq!(
            error_parts(&ContentPack::load(vec![not_finite]).unwrap_err()),
            ("CONTENT", "hardness")
        );

        let mut zero_drop = record;
        zero_drop.drops = BTreeMap::from([("core:stone".to_owned(), 0)]);
        assert_eq!(
            error_parts(&ContentPack::load(vec![zero_drop]).unwrap_err()),
            ("CONTENT", "drop count")
        );
    }

    #[test]
    fn reserved_base_matches_the_record_cap() {
        assert_eq!(usize::from(UNKNOWN_BASE), MAX_RECORDS);
        assert!(MAX_RECORDS <= usize::from(BlockId::MAX) + 1);
    }
}
