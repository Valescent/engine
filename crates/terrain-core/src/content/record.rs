use std::collections::BTreeMap;

use crate::error::{ErrorKind, Stage, TerrainError};
use crate::section::BlockId;

use super::pack::ContentPack;

/// First runtime id reserved for an unknown saved name (`T1.6`).
///
/// A saved name missing from the pack allocates one id at or above this base,
/// stays opaque, collidable, hardness-high, and drop-free, and keeps its
/// original name on disk. A pack may therefore never fill this range: runtime
/// ids below it are real content.
pub(crate) const UNKNOWN_BASE: BlockId = 0xF000;

/// Most records one pack may hold, so no real id reaches `UNKNOWN_BASE`.
///
/// `MAX_RECORDS <= UNKNOWN_BASE` is asserted by a test, not by a cast, so the
/// constant stays usable in array bounds and comparisons.
pub(crate) const MAX_RECORDS: usize = 0xF000;

/// Longest legal stable name in bytes (`A5.1`).
pub(crate) const MAX_NAME_BYTES: usize = 64;

/// Highest light level; the range of `opacity`, `sky_atten`, and `emission`.
pub const LIGHT_LEVELS: u8 = 15;

/// Skylight lost per water block (`L2.4`).
pub(crate) const WATER_SKY_ATTEN: u8 = 1;

/// Light a lamp emits in the placeholder pack (`L2.1`).
pub(crate) const LAMP_EMISSION: u8 = 14;

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

    /// Stable canonical tag; part of `record_hash`.
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

    /// Stable canonical tag; part of `record_hash`.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::Opaque => 0,
            Self::Cutout => 1,
            Self::Water => 2,
        }
    }
}

/// Per-face texture references (ADR-0015).
///
/// Presentation only: a record's textures are **not** part of `record_hash`
/// or `ContentPack::pack_hash`, so re-texturing a block never invalidates a
/// saved world or a pinned generation description. Empty strings are invalid;
/// every record names all three faces even when they are the same texture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaceTextures {
    /// Texture seen from above (`+Y`).
    pub top: String,
    /// Texture seen from the four sides (`±X`, `±Z`) and cross quads.
    pub side: String,
    /// Texture seen from below (`-Y`).
    pub bottom: String,
}

impl FaceTextures {
    /// One texture on every face.
    #[must_use]
    pub fn uniform(name: &str) -> Self {
        Self {
            top: name.to_owned(),
            side: name.to_owned(),
            bottom: name.to_owned(),
        }
    }

    /// Distinct top, side, and bottom textures.
    #[must_use]
    pub fn faces(top: &str, side: &str, bottom: &str) -> Self {
        Self {
            top: top.to_owned(),
            side: side.to_owned(),
            bottom: bottom.to_owned(),
        }
    }
}

/// One block definition from the content pack (`A5.2`).
///
/// Field order **is** the canonical hash order; see `record_hash`. Every
/// field is required: there are no defaults, because a default that differs
/// from the author's intent is a silent world change.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockRecord {
    /// Stable `namespace:name`, lowercase `[a-z0-9_]`, at most
    /// `MAX_NAME_BYTES` bytes, immutable once shipped (`A5.1`).
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
    /// Per-face texture references. Presentation only; excluded from
    /// `record_hash` by ADR-0015.
    pub textures: FaceTextures,
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
        self_dropping(with_textures(
            opaque("core:grass", 0.6),
            FaceTextures::faces("core:grass_top", "core:grass_side", "core:dirt"),
        )),
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
        self_dropping(with_textures(
            opaque("core:wood", 2.0),
            FaceTextures::faces("core:wood_top", "core:wood_side", "core:wood_top"),
        )),
    ];
    ContentPack::load(records).expect("the built-in v1 placeholder pack is valid")
}

/// Placeholder record version for every [`v1_default`] entry.
const RECORD_VERSION: u32 = 1;

/// The default shape of a solid, opaque cube.
pub(super) fn opaque(name: &str, hardness: f32) -> BlockRecord {
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
        textures: FaceTextures::uniform(name),
    }
}

/// Replaces a record's per-face textures (presentation only, ADR-0015).
fn with_textures(mut record: BlockRecord, textures: FaceTextures) -> BlockRecord {
    record.textures = textures;
    record
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
pub(super) fn content_error(rule: &'static str) -> TerrainError {
    TerrainError::new(
        Stage::Description,
        ErrorKind::DescriptionInvalid {
            key: "CONTENT",
            rule,
        },
    )
}

/// Checks one record's ranges and name (`A5.1`, `A5.2`).
pub(super) fn validate(record: &BlockRecord) -> Result<(), TerrainError> {
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
    validate_name(&record.textures.top)?;
    validate_name(&record.textures.side)?;
    validate_name(&record.textures.bottom)?;
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::content::ContentPack;
    use crate::error::{ErrorKind, TerrainError};

    fn error_parts(error: &TerrainError) -> (&'static str, &'static str) {
        match &error.kind {
            ErrorKind::DescriptionInvalid { key, rule } => (key, rule),
            other => panic!("expected DescriptionInvalid, found {other:?}"),
        }
    }

    #[test]
    fn invalid_texture_references_are_rejected() {
        let mut records = v1_default().records().to_vec();
        records[0].textures.bottom = String::new();
        let error = ContentPack::load(records).unwrap_err();
        assert_eq!(error_parts(&error), ("CONTENT", "missing name"));

        let mut records = v1_default().records().to_vec();
        records[0].textures.side = "Not A Name".to_owned();
        let error = ContentPack::load(records).unwrap_err();
        assert_eq!(error_parts(&error), ("CONTENT", "name charset"));
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
