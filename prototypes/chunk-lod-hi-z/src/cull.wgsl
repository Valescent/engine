// Chunk AABB culling against a camera frustum and a Hi-Z depth pyramid.
//
// Depth convention: `reverse_depth = 1 - view_depth / far`. Larger means
// nearer, matching a reverse-Z depth buffer. The pyramid stores the minimum
// reverse depth of each 2x2 block, so a texel holds the *farthest* occluder
// in that region. A chunk is occluded when even its nearest slab is farther
// than the farthest occluder covering its screen-space AABB.
//
// The prototype only produces indirect draw records. No draw is issued here.

struct Chunk {
    min: vec4<f32>,
    max: vec4<f32>,
};

struct Params {
    // xyz = inward-facing plane normal, w = plane offset.
    planes: array<vec4<f32>, 6>,
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    camera_forward: vec4<f32>,
    // width, height, max mip level, chunk count.
    hiz: vec4<u32>,
    // far plane, occlusion bias (in reverse-depth units), unused, unused.
    depth: vec4<f32>,
};

struct DrawIndexedIndirectArgs {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
};

struct ScreenRect {
    min_uv: vec2<f32>,
    max_uv: vec2<f32>,
    valid: bool,
};

@group(0) @binding(0) var<storage, read> chunks: array<Chunk>;
@group(0) @binding(1) var<storage, read> hiz: array<f32>;
@group(0) @binding(2) var<storage, read_write> visible_count: atomic<u32>;
@group(0) @binding(3) var<storage, read_write> draws: array<DrawIndexedIndirectArgs>;
@group(0) @binding(4) var<storage, read_write> visible_ids: array<u32>;
@group(0) @binding(5) var<storage, read> params: Params;

const BIG: f32 = 1.0e30;

// Conservative AABB / plane test: only reject when every corner is outside.
fn aabb_outside_frustum(lo: vec3<f32>, hi: vec3<f32>) -> bool {
    for (var i = 0u; i < 6u; i = i + 1u) {
        let plane = params.planes[i];
        let corner = select(hi, lo, plane.xyz < vec3<f32>(0.0));
        if (dot(plane.xyz, corner) + plane.w < 0.0) {
            return true;
        }
    }
    return false;
}

// Project the eight AABB corners and return their screen-space UV bounds.
// Corners behind the camera make the rect invalid; those chunks are left
// visible so Hi-Z can never introduce a false negative.
fn project_aabb(lo: vec3<f32>, hi: vec3<f32>) -> ScreenRect {
    var rect = ScreenRect(vec2<f32>(BIG, BIG), vec2<f32>(-BIG, -BIG), true);
    for (var i = 0u; i < 8u; i = i + 1u) {
        let corner = select(
            lo,
            hi,
            vec3<bool>((i & 1u) != 0u, (i & 2u) != 0u, (i & 4u) != 0u),
        );
        let clip = params.view_proj * vec4<f32>(corner, 1.0);
        if (clip.w <= 0.0) {
            rect.valid = false;
            return rect;
        }
        let ndc = clip.xyz / clip.w;
        let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        rect.min_uv = min(rect.min_uv, uv);
        rect.max_uv = max(rect.max_uv, uv);
    }
    return rect;
}

// Offset of a mip in the flat pyramid. Levels are stored largest first:
// offset(L) = (W*H - w(L)*h(L)) * 4/3 for a full quadtree.
fn level_offset(width: u32, height: u32, mip_width: u32, mip_height: u32) -> u32 {
    return (width * height - mip_width * mip_height) * 4u / 3u;
}

// Pick a mip whose texels cover the screen rect in at most 2x2 samples, then
// take the minimum over those samples. Min-of-min is conservative.
fn pyramid_min(uv_min: vec2<f32>, uv_max: vec2<f32>) -> f32 {
    let width = params.hiz.x;
    let height = params.hiz.y;
    let clamped_min = clamp(uv_min, vec2<f32>(0.0), vec2<f32>(1.0));
    let clamped_max = clamp(uv_max, vec2<f32>(0.0), vec2<f32>(1.0));

    let span_px = max((clamped_max - clamped_min) * vec2<f32>(f32(width), f32(height)), vec2<f32>(1.0));
    let span = max(span_px.x, span_px.y);
    let level = u32(clamp(ceil(log2(span)), 0.0, f32(params.hiz.z)));

    let mip_width = max(1u, width >> level);
    let mip_height = max(1u, height >> level);
    let base = level_offset(width, height, mip_width, mip_height);

    let x0 = clamp(u32(clamped_min.x * f32(mip_width)), 0u, mip_width - 1u);
    let x1 = clamp(u32(clamped_max.x * f32(mip_width)), 0u, mip_width - 1u);
    let y0 = clamp(u32(clamped_min.y * f32(mip_height)), 0u, mip_height - 1u);
    let y1 = clamp(u32(clamped_max.y * f32(mip_height)), 0u, mip_height - 1u);

    var result = min(hiz[base + y0 * mip_width + x0], hiz[base + y0 * mip_width + x1]);
    result = min(result, hiz[base + y1 * mip_width + x0]);
    result = min(result, hiz[base + y1 * mip_width + x1]);
    return result;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let chunk_id = gid.x;
    if (chunk_id >= params.hiz.w) {
        return;
    }

    let chunk = chunks[chunk_id];
    let lo = chunk.min.xyz;
    let hi = chunk.max.xyz;

    if (aabb_outside_frustum(lo, hi)) {
        return;
    }

    // Minimum view-space depth of the AABB, converted to reverse depth.
    let center = (lo + hi) * 0.5;
    let half_extent = (hi - lo) * 0.5;
    let forward = params.camera_forward.xyz;
    let nearest_depth = dot(forward, center - params.camera_pos.xyz)
        - dot(abs(forward), half_extent);
    let reverse_depth = clamp(1.0 - nearest_depth / params.depth.x, 0.0, 1.0);

    let rect = project_aabb(lo, hi);
    if (rect.valid) {
        let occluder_depth = pyramid_min(rect.min_uv, rect.max_uv);
        if (reverse_depth < occluder_depth - params.depth.y) {
            return;
        }
    }

    // Append one indirect draw record and the matching chunk id.
    let slot = atomicAdd(&visible_count, 1u);
    if (slot < params.hiz.w) {
        draws[slot] = DrawIndexedIndirectArgs(6u, 1u, 0u, 0, chunk_id);
        visible_ids[slot] = chunk_id;
    }
}
