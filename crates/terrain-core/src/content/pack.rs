use std::collections::BTreeMap;

use crate::error::TerrainError;

use super::hash::record_hash;
use super::id::BlockId;
use super::record::{BlockRecord, LIGHT_LEVELS, MAX_RECORDS, content_error, validate};

/// Skylight attenuation reported for an id that the pack does not hold.
///
/// Fail safe, like `light2`'s opaque fallback: an unknown block can under-light
/// a scene, never leak light through geometry. Callers that care about the
/// difference use `ContentPack::props`, which returns `None`.
pub const UNKNOWN_SKY_ATTEN: u8 = LIGHT_LEVELS;

/// A parsed content pack, indexed by stable name and by runtime id.
///
/// `records` is kept in ascending name order, so `id` is also the index of the
/// record and every accessor is one array read (`A5.2`). Construct it through
/// [`ContentPack::load`] (or [`v1_default`](super::record::v1_default)) so that invariant holds.
#[derive(Clone, Debug, PartialEq)]
pub struct ContentPack {
    pub(super) records: Vec<BlockRecord>,
    pub(super) ids: BTreeMap<String, BlockId>,
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
    /// - the pack holds more than `MAX_RECORDS` records;
    /// - a name is empty, longer than `MAX_NAME_BYTES`, outside the charset,
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
    /// `None` for an id outside the pack, including the `UNKNOWN_BASE`
    /// placeholder range.
    #[must_use]
    pub(crate) fn props(&self, id: BlockId) -> Option<&BlockRecord> {
        self.records.get(usize::from(id))
    }

    /// Skylight attenuation for one id; [`UNKNOWN_SKY_ATTEN`] outside the pack.
    #[must_use]
    pub(crate) fn sky_atten(&self, id: BlockId) -> u8 {
        self.props(id)
            .map_or(UNKNOWN_SKY_ATTEN, |record| record.sky_atten)
    }

    /// Light emission for one id; `0` outside the pack.
    #[must_use]
    pub(crate) fn emission(&self, id: BlockId) -> u8 {
        self.props(id).map_or(0, |record| record.emission)
    }

    /// Refuses generation against a pack that does not match `pinned` exactly (`A5.4`).
    ///
    /// Every pinned record must be present with the same `record_version` and
    /// the same `record_hash`. Extra records in `self` are ignored: a
    /// superset is fine, a subset is not. Missing, changed, or re-versioned
    /// records are all `DescriptionInvalid { key: "CONTENT" }`, with the rule
    /// naming which case fired.
    ///
    /// This is the *generation* check only. Opening saved terrain is allowed
    /// with missing names, which degrade to `UNKNOWN_BASE` placeholders
    /// (`T1.6`), and never regenerates against a subset.
    ///
    /// # Errors
    ///
    /// `DescriptionInvalid { key: "CONTENT" }` for the first pinned record that
    /// is missing, changed, or has a different `record_version`.
    pub(crate) fn check_compatible(&self, pinned: &ContentPack) -> Result<(), TerrainError> {
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::content::v1_default;
    use crate::error::{ErrorKind, TerrainError};

    use super::super::id::BlockId;

    use super::super::record::{LAMP_EMISSION, UNKNOWN_BASE, WATER_SKY_ATTEN, opaque};

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
}
