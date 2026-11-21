//! Canonical description bytes and the 32-byte world identity.

use crate::error::{Stage, TerrainError};
use crate::hash::sha256;

use super::schema::{Axis, Description, reject};

impl Description {
    /// Canonical bytes: the input to the description hash.
    ///
    /// Field order is structure order. Strings are length-prefixed with a
    /// `u32` byte count. Floats are bit patterns with `-0.0` normalised.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, TerrainError> {
        self.validate()?;
        let mut out = Vec::with_capacity(256);
        out.extend_from_slice(&self.schema_version.to_le_bytes());
        out.extend_from_slice(&self.world_height.to_le_bytes());
        for value in [
            self.sea,
            self.max_terrain_y,
            self.deep_y,
            self.macro_cell,
            self.tile_blocks,
            self.hydro_tile,
            self.hydro_border,
            self.world_half_extent,
            self.max_lake_y,
            self.cave_min_roof,
            self.detail_down,
            self.detail_up,
            self.strata_thickness,
            self.fault_cell,
            self.ore_region,
            self.tree_cell,
            self.z_equator,
            self.z_half_span,
            self.edge_band,
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&self.river_min_cells.to_le_bytes());
        out.extend_from_slice(&self.lake_min_cells.to_le_bytes());
        out.extend_from_slice(&self.blend_blocks.to_le_bytes());
        for (key, value) in [
            ("LAKE_MIN_DEPTH", self.lake_min_depth),
            ("W_SCALE", self.w_scale),
            ("RIVER_RAMP_CELLS", self.river_ramp_cells),
            ("CAVE_T", self.cave_t),
            ("ENTRANCE_T", self.entrance_t),
            ("LAPSE", self.lapse),
            ("T_SNOW", self.t_snow),
            ("STRATA_WARP_AMP", self.strata_warp_amp),
        ] {
            out.extend_from_slice(&canonical_f32(key, value)?.to_le_bytes());
        }
        out.push(self.water_sky_atten);
        out.push(match self.wind_dir {
            Axis::X => 0,
            Axis::Z => 1,
        });
        for embedded in [&self.graph, &self.content] {
            let len = u32::try_from(embedded.name.len())
                .map_err(|_| reject(Stage::Description, "GRAPH", "name length"))?;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(embedded.name.as_bytes());
            out.extend_from_slice(&embedded.record_version.to_le_bytes());
            out.extend_from_slice(&embedded.hash);
        }
        Ok(out)
    }

    /// The 32-byte world identity (`T1.2`).
    pub fn description_hash(&self) -> Result<[u8; 32], TerrainError> {
        Ok(sha256(&self.canonical_bytes()?))
    }
}

fn canonical_f32(key: &'static str, value: f32) -> Result<u32, TerrainError> {
    if !value.is_finite() {
        return Err(reject(Stage::Description, key, "finite"));
    }
    Ok(if value == 0.0 { 0 } else { value.to_bits() })
}
