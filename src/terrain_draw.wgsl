// Packed-quad terrain drawing (`M3.1`, §14.1).
//
// `mesher2` emits one `u64` per greedy quad. The client uploads those records
// unchanged, as two `u32` words per quad, and pulls six vertices per quad from
// a read-only storage buffer: there is no index buffer and no per-vertex data.
// `vertex_index / 6` selects the record, `vertex_index % 6` selects one corner
// of one of its two triangles.
//
// Record layout, least-significant field first:
//   x:5 y:5 z:5 | w1:5 h1:5 | normal:3 | ao:8 | sky:4 blk:4 | tile:16 | tint:4
//
// Face tangents match `mesher2`: `+X/-X` use `(u, v) = (z, y)`, `+Y/-Y` use
// `(x, z)`, `+Z/-Z` use `(x, y)`. Positive faces sit one block along their
// normal. UVs are in block units with a repeat sampler, so merged quads tile.
//
// The CPU mirror of the corner table and the AO packing lives in
// `renderer.rs` (`decode_quad`, `quad_positions`) and is covered by tests, so a
// layout or corner-order change must update both files.

struct Camera {
    view_projection: mat4x4<f32>,
    sky_colour: vec4<f32>,
    ambient_colour: vec4<f32>,
    sun_colour: vec4<f32>,
    sun_direction: vec4<f32>,
    // x: unlit flag, y: far plane, z: time of day, w: fog end distance.
    settings: vec4<f32>,
    // Camera is relative to the frame origin.
    camera_position: vec4<f32>,
    // The frame's absolute world-block render origin. Section origins are
    // absolute integers too; vertices subtract this in integer space.
    render_origin: vec4<i32>,
    // The frame origin's water-wave phase, wrapped to [0, 2*pi).
    wave_anchor: vec4<f32>,
};

struct Section {
    // Absolute world-block section origin in `xyz`; `w` is the world blocks
    // per cell (1 fine, 2 LOD1, 4 LOD2). Subtracting the frame origin in
    // integer space keeps every relative delta exact in f32.
    origin: vec4<i32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

@group(1) @binding(0)
var<uniform> section: Section;

@group(1) @binding(1)
var<storage, read> quads: array<u32>;

@group(2) @binding(0)
var block_texture: texture_2d_array<f32>;

@group(2) @binding(1)
var block_sampler: sampler;

// The decoded fields of one record.
struct Quad {
    origin: vec3<f32>,
    // Extents in blocks: `w1 + 1` along `u`, `h1 + 1` along `v`.
    width: f32,
    height: f32,
    normal: u32,
    ao: u32,
    sky: u32,
    block: u32,
    tile: u32,
    tint: u32,
};

fn load_quad(index: u32) -> Quad {
    let scale = f32(section.origin.w);
    let lo = quads[index * 2u];
    let hi = quads[index * 2u + 1u];
    var quad: Quad;
    quad.origin = vec3<f32>(
        f32(lo & 0x1Fu),
        f32((lo >> 5u) & 0x1Fu),
        f32((lo >> 10u) & 0x1Fu),
    ) * scale;
    quad.width = f32(((lo >> 15u) & 0x1Fu) + 1u) * scale;
    quad.height = f32(((lo >> 20u) & 0x1Fu) + 1u) * scale;
    quad.normal = (lo >> 25u) & 0x7u;
    // The AO byte straddles the two words: low four bits in `lo`, high in `hi`.
    quad.ao = ((lo >> 28u) & 0xFu) | ((hi & 0xFu) << 4u);
    quad.sky = (hi >> 4u) & 0xFu;
    quad.block = (hi >> 8u) & 0xFu;
    quad.tile = (hi >> 12u) & 0xFFFFu;
    quad.tint = (hi >> 28u) & 0xFu;
    return quad;
}

// Corner order of the two triangles, before the AO flip. Corner 0 is the
// quad's `(u, v)` origin, then `(0, h)`, `(w, h)`, `(w, 0)`.
fn corner_slot(vertex: u32, flip: bool) -> u32 {
    var order: array<u32, 6> = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    var slot = order[vertex];
    if (flip) {
        // 1,2,3 / 1,3,0: the other diagonal.
        slot = (slot + 1u) % 4u;
    }
    return slot;
}

// `(du, dv)` multipliers of the quad extents for one corner slot, in the
// face's tangent order. `+X/+Y/+Z` use the base order; `-X/+Z` mirror `u` and
// `-Y` mirrors `v`, which is what keeps every quad counter-clockwise from
// outside.
fn face_duv(normal: u32, slot: u32) -> vec2<f32> {
    var corners: array<vec2<f32>, 4> = array<vec2<f32>, 4>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(1.0, 0.0),
    );
    var duv = corners[slot];
    if (normal == 1u || normal == 4u) {
        duv.x = 1.0 - duv.x;
    } else if (normal == 3u) {
        duv.y = 1.0 - duv.y;
    }
    return duv;
}

struct Corner {
    position: vec3<f32>,
    uv: vec2<f32>,
};

// UV corner order shared by all faces and cross quads, in block units. A
// merged quad repeats the tile once per block with the repeat sampler.
fn corner_uv(slot: u32, width: f32, height: f32) -> vec2<f32> {
    var uvs: array<vec2<f32>, 4> = array<vec2<f32>, 4>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(1.0, 0.0),
    );
    let uv = uvs[slot];
    return vec2<f32>(uv.x * width, uv.y * height);
}

fn corner_geometry(quad: Quad, slot: u32) -> Corner {
    var corner: Corner;
    let scale = f32(section.origin.w);
    let x = quad.origin.x;
    let y = quad.origin.y;
    let z = quad.origin.z;

    if (quad.normal >= 6u) {
        // Cutout cross quad (`M2.4`): a vertical diagonal through the cell.
        // Slot 0..3 runs bottom `p0`, top `p0`, top `p1`, bottom `p1`.
        var p0 = vec2<f32>(x, z);
        var p1 = vec2<f32>(x + scale, z + scale);
        if (quad.normal == 7u) {
            p0 = vec2<f32>(x + scale, z);
            p1 = vec2<f32>(x, z + scale);
        }
        var position = vec3<f32>(p0.x, y, p0.y);
        if (slot == 1u) {
            position = vec3<f32>(p0.x, y + scale, p0.y);
        } else if (slot == 2u) {
            position = vec3<f32>(p1.x, y + scale, p1.y);
        } else if (slot == 3u) {
            position = vec3<f32>(p1.x, y, p1.y);
        }
        corner.position = position;
        corner.uv = corner_uv(slot, scale, scale);
        return corner;
    }

    let duv = face_duv(quad.normal, slot);
    // Greedy merging creates T-junctions: a long quad's edge can pass through a
    // vertex of two shorter neighbours, and the differing edge equations leave a
    // one-pixel gap that reads as a bright speck (sky through a wall). Expanding
    // every quad by a thousandth of a block closes the gap; coplanar overlap is
    // benign because the depth test keeps the first surface.
    let overlap = 0.001;
    let u = duv.x * (quad.width + 2.0 * overlap) - overlap;
    let v = duv.y * (quad.height + 2.0 * overlap) - overlap;
    var position = vec3<f32>(x + u, y + v, z);
    if (quad.normal == 0u) {
        position = vec3<f32>(x + scale, y + v, z + u);
    } else if (quad.normal == 1u) {
        position = vec3<f32>(x, y + v, z + u);
    } else if (quad.normal == 2u) {
        position = vec3<f32>(x + u, y + scale, z + v);
    } else if (quad.normal == 3u) {
        position = vec3<f32>(x + u, y, z + v);
    } else if (quad.normal == 4u) {
        position = vec3<f32>(x + u, y + v, z + scale);
    }
    corner.position = position;
    corner.uv = corner_uv(slot, quad.width, quad.height);
    return corner;
}

fn face_normal(normal: u32) -> vec3<f32> {
    if (normal == 0u) {
        return vec3<f32>(1.0, 0.0, 0.0);
    } else if (normal == 1u) {
        return vec3<f32>(-1.0, 0.0, 0.0);
    } else if (normal == 2u) {
        return vec3<f32>(0.0, 1.0, 0.0);
    } else if (normal == 3u) {
        return vec3<f32>(0.0, -1.0, 0.0);
    } else if (normal == 4u) {
        return vec3<f32>(0.0, 0.0, 1.0);
    } else if (normal == 5u) {
        return vec3<f32>(0.0, 0.0, -1.0);
    }
    // Cross quads shade like a ground-standing billboard.
    return vec3<f32>(0.0, 1.0, 0.0);
}

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) layer: u32,
    @location(3) @interpolate(flat) sky: u32,
    @location(4) @interpolate(flat) block_light: u32,
    @location(5) ao: f32,
    @location(6) @interpolate(flat) tint: u32,
    @location(7) fog_distance: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOut {
    let quad = load_quad(vertex_index / 6u);
    let vertex = vertex_index % 6u;

    // Packed AO corners (`M3.1`): corner 0 is `(-u, -v)`, then `(+u, -v)`,
    // `(+u, +v)`, `(-u, +v)`.
    let ao0 = quad.ao & 0x3u;
    let ao1 = (quad.ao >> 2u) & 0x3u;
    let ao2 = (quad.ao >> 4u) & 0x3u;
    let ao3 = (quad.ao >> 6u) & 0x3u;

    // Anisotropy test: the `0-2` pair against the `1-3` pair picks the
    // diagonal. Water and cross quads carry no AO, so they never flip.
    var flip = false;
    if (quad.normal < 6u) {
        flip = (ao0 + ao2) > (ao1 + ao3);
    }
    let slot = corner_slot(vertex, flip);

    // Winding slot to packed AO corner: 0->0, 1->3, 2->2, 3->1.
    var ao = 0u;
    if (slot == 0u) {
        ao = ao0;
    } else if (slot == 1u) {
        ao = ao3;
    } else if (slot == 2u) {
        ao = ao2;
    } else {
        ao = ao1;
    }

    let corner = corner_geometry(quad, slot);
    // Integer subtraction first: the difference is at most a few hundred
    // blocks, so the f32 cast is exact even when the world coordinate is not.
    let relative_origin = vec3<f32>(section.origin.xyz - camera.render_origin.xyz);
    let world_position = relative_origin + corner.position;

    var output: VertexOut;
    output.clip_position = camera.view_projection * vec4<f32>(world_position, 1.0);
    output.normal = face_normal(quad.normal);
    output.uv = corner.uv;
    output.layer = quad.tile;
    output.sky = quad.sky;
    output.block_light = quad.block;
    output.ao = f32(ao);
    output.tint = quad.tint;
    output.fog_distance = length(world_position - camera.camera_position.xyz);
    return output;
}

// 16-entry light curve, indexed by a nibble. No `pow`, no normalisation by
// hand in the fragment shader.
fn light_curve(level: u32) -> f32 {
    var curve: array<f32, 16> = array<f32, 16>(
        0.03, 0.06, 0.10, 0.15, 0.21, 0.28, 0.36, 0.45,
        0.55, 0.65, 0.75, 0.84, 0.90, 0.95, 0.98, 1.00,
    );
    return curve[level];
}

// Identity biome tint table: the index is carried through but recolours
// nothing yet.
fn tint_colour(index: u32) -> vec3<f32> {
    var tints: array<vec3<f32>, 16> = array<vec3<f32>, 16>(
        vec3<f32>(1.0, 1.0, 1.0), vec3<f32>(1.0, 1.0, 1.0),
        vec3<f32>(1.0, 1.0, 1.0), vec3<f32>(1.0, 1.0, 1.0),
        vec3<f32>(1.0, 1.0, 1.0), vec3<f32>(1.0, 1.0, 1.0),
        vec3<f32>(1.0, 1.0, 1.0), vec3<f32>(1.0, 1.0, 1.0),
        vec3<f32>(1.0, 1.0, 1.0), vec3<f32>(1.0, 1.0, 1.0),
        vec3<f32>(1.0, 1.0, 1.0), vec3<f32>(1.0, 1.0, 1.0),
        vec3<f32>(1.0, 1.0, 1.0), vec3<f32>(1.0, 1.0, 1.0),
        vec3<f32>(1.0, 1.0, 1.0), vec3<f32>(1.0, 1.0, 1.0),
    );
    return tints[index];
}

fn shade(input: VertexOut, albedo: vec3<f32>) -> vec4<f32> {
    let normal = normalize(input.normal);
    let tint = tint_colour(input.tint);
    let sky = light_curve(input.sky);
    let block = light_curve(input.block_light);
    let ao = 0.55 + 0.15 * input.ao;

    let sun_direction = camera.sun_direction.xyz;
    let ambient = camera.ambient_colour.rgb * sky;
    let sun = camera.sun_colour.rgb
        * max(dot(normal, sun_direction), 0.0)
        * max(sun_direction.y, 0.0)
        * sky;
    let lamp = vec3<f32>(1.00, 0.75, 0.45) * block;
    var lit = albedo * tint * ao * (ambient + sun + lamp);

    // v0.0.4 unlit debug fallback, matching `shader.wgsl`.
    if (camera.settings.x > 0.5) {
        let light_direction = normalize(vec3<f32>(0.35, 0.85, 0.45));
        let flat = 0.45 + 0.55 * max(dot(normal, light_direction), 0.0);
        lit = albedo * tint * flat;
    }

    let fog_end = camera.settings.w;
    let fog = clamp((input.fog_distance - 0.7 * fog_end) / (0.3 * fog_end), 0.0, 1.0);
    let fogged = mix(lit, camera.sky_colour.rgb, fog);
    return vec4<f32>(clamp(fogged, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let albedo = textureSample(
        block_texture,
        block_sampler,
        input.uv,
        i32(input.layer),
    );
    // Alpha test: cutout records share this pass, opaque records always pass.
    if (albedo.a < 0.5) {
        discard;
    }
    return shade(input, albedo.rgb);
}

@fragment
fn fs_water(input: VertexOut) -> @location(0) vec4<f32> {
    let albedo = textureSample(
        block_texture,
        block_sampler,
        input.uv,
        i32(input.layer),
    );
    let colour = shade(input, albedo.rgb);
    return vec4<f32>(colour.rgb, 0.65);
}
