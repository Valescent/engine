// Vertex pulling for the ADR-0017 quad pool.
//
// One quad record per six vertices. The record layout, corner order, and light
// packing mirror `src/quad.rs`; the CPU tests pin that mirror.

struct Camera {
    view_projection: mat4x4<f32>,
    sky_colour: vec4<f32>,
    ambient_colour: vec4<f32>,
    sun_colour: vec4<f32>,
    sun_direction: vec4<f32>,
    settings: vec4<f32>,
    camera_position: vec4<f32>,
};

struct ChunkInfo {
    origin: vec4<f32>,
    water_origin: vec4<f32>,
    opaque_first: u32,
    opaque_quads: u32,
    water_first: u32,
    water_quads: u32,
};

struct Quad {
    w0: u32,
    w1: u32,
    w2: u32,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

@group(1) @binding(0)
var<storage, read> chunk_info: array<ChunkInfo>;

@group(1) @binding(1)
var<storage, read> quads: array<Quad>;

@group(2) @binding(0)
var block_texture: texture_2d_array<f32>;

@group(2) @binding(1)
var block_sampler: sampler;

// Corner order `(u, v)` bits, two bits per corner, packed per face.
const CORNER_UV: array<u32, 6> = array<u32, 6>(
    0x78u, 0x2Du, 0x78u, 0xD2u, 0x2Du, 0x78u,
);
const QUAD_CORNERS: array<u32, 6> = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) layer: u32,
    @location(3) light: vec3<f32>,
    @location(4) fog_distance: f32,
    @location(5) world_position: vec3<f32>,
};

fn face_normal(face: u32) -> vec3<f32> {
    switch face {
        case 0u: { return vec3<f32>(1.0, 0.0, 0.0); }
        case 1u: { return vec3<f32>(-1.0, 0.0, 0.0); }
        case 2u: { return vec3<f32>(0.0, 1.0, 0.0); }
        case 3u: { return vec3<f32>(0.0, -1.0, 0.0); }
        case 4u: { return vec3<f32>(0.0, 0.0, 1.0); }
        default: { return vec3<f32>(0.0, 0.0, -1.0); }
    }
}

fn face_axes(face: u32) -> array<vec3<f32>, 2> {
    switch face {
        case 0u, 1u: { return array<vec3<f32>, 2>(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, 0.0)); }
        case 2u, 3u: { return array<vec3<f32>, 2>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 0.0, 1.0)); }
        default: { return array<vec3<f32>, 2>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0)); }
    }
}

fn corner_light(quad: Quad, corner: u32) -> vec3<f32> {
    var sky: u32;
    var block: u32;
    var ao: u32;
    switch corner {
        case 0u: {
            sky = (quad.w1 >> 16u) & 0xFu;
            block = (quad.w1 >> 20u) & 0xFu;
            ao = (quad.w1 >> 8u) & 0x3u;
        }
        case 1u: {
            sky = (quad.w1 >> 24u) & 0xFu;
            block = (quad.w1 >> 28u) & 0xFu;
            ao = (quad.w1 >> 10u) & 0x3u;
        }
        case 2u: {
            sky = quad.w2 & 0xFu;
            block = (quad.w2 >> 4u) & 0xFu;
            ao = (quad.w1 >> 12u) & 0x3u;
        }
        default: {
            sky = (quad.w2 >> 8u) & 0xFu;
            block = (quad.w2 >> 12u) & 0xFu;
            ao = (quad.w1 >> 14u) & 0x3u;
        }
    }
    return vec3<f32>(f32(sky) / 15.0, f32(block) / 15.0, f32(ao));
}

fn shade(quad: Quad, origin: vec4<f32>, corner: u32) -> VertexOut {
    let w0 = quad.w0;
    let face = (w0 >> 25u) & 0x7u;
    let width = f32(((w0 >> 15u) & 0x1Fu) + 1u);
    let height = f32(((w0 >> 20u) & 0x1Fu) + 1u);
    let cell = vec3<f32>(
        f32(w0 & 0x1Fu),
        f32((w0 >> 5u) & 0x1Fu),
        f32((w0 >> 10u) & 0x1Fu),
    );
    let normal = face_normal(face);
    let axes = face_axes(face);
    let pair = (CORNER_UV[face] >> (corner * 2u)) & 0x3u;
    let u_bit = f32(pair & 0x1u);
    let v_bit = f32((pair >> 1u) & 0x1u);
    let local = cell + max(normal, vec3<f32>(0.0))
        + axes[0] * u_bit * width
        + axes[1] * v_bit * height;
    let world_position = origin.xyz + local * origin.w;

    let us = width * origin.w;
    let vs = height * origin.w;
    var uv = vec2<f32>(0.0);
    if (corner == 1u || corner == 2u) {
        uv.y = vs;
    }
    if (corner == 2u || corner == 3u) {
        uv.x = us;
    }

    var output: VertexOut;
    output.clip_position = camera.view_projection * vec4<f32>(world_position, 1.0);
    output.normal = normal;
    output.uv = uv;
    output.layer = quad.w1 & 0xFFu;
    output.light = corner_light(quad, corner);
    output.fog_distance = length(world_position - camera.camera_position.xyz);
    output.world_position = world_position;
    return output;
}

@vertex
fn vs_quad(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOut {
    let info = chunk_info[instance_index];
    let quad = quads[info.opaque_first + vertex_index / 6u];
    return shade(quad, info.origin, QUAD_CORNERS[vertex_index % 6u]);
}

@vertex
fn vs_quad_indexed(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOut {
    let info = chunk_info[instance_index];
    let quad = quads[vertex_index / 4u];
    return shade(quad, info.origin, vertex_index % 4u);
}

@vertex
fn vs_water_quad(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOut {
    let info = chunk_info[instance_index];
    let quad = quads[info.water_first + vertex_index / 6u];
    return shade(quad, info.water_origin, QUAD_CORNERS[vertex_index % 6u]);
}

@vertex
fn vs_water_quad_indexed(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOut {
    let info = chunk_info[instance_index];
    let quad = quads[vertex_index / 4u];
    return shade(quad, info.water_origin, vertex_index % 4u);
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let albedo = textureSample(
        block_texture,
        block_sampler,
        input.uv,
        i32(input.layer),
    );
    let normal = normalize(input.normal);

    if (camera.settings.x > 0.5) {
        let light_direction = normalize(vec3<f32>(0.35, 0.85, 0.45));
        let shade = 0.45 + 0.55 * max(dot(normal, light_direction), 0.0);
        return vec4<f32>(albedo.rgb * shade, 1.0);
    }

    let sky = input.light.x;
    let block = input.light.y;
    let ao = 0.55 + 0.15 * input.light.z;
    let sun_direction = camera.sun_direction.xyz;

    let ambient = camera.ambient_colour.rgb * sky;
    let sun = camera.sun_colour.rgb
        * max(dot(normal, sun_direction), 0.0)
        * max(sun_direction.y, 0.0)
        * sky;
    let moon = vec3<f32>(0.50, 0.60, 0.80)
        * max(dot(normal, -sun_direction), 0.0)
        * max(-sun_direction.y, 0.0)
        * sky;
    let lamp = vec3<f32>(1.00, 0.75, 0.45) * block;
    let lit = albedo.rgb * ao * (ambient + sun + moon + lamp);

    let fog_end = camera.settings.w;
    let fog = clamp((input.fog_distance - 0.7 * fog_end) / (0.3 * fog_end), 0.0, 1.0);
    let fogged = mix(lit, camera.sky_colour.rgb, fog);
    return vec4<f32>(clamp(fogged, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}

@fragment
fn fs_water(input: VertexOut) -> @location(0) vec4<f32> {
    let albedo = textureSample(
        block_texture,
        block_sampler,
        input.uv,
        i32(input.layer),
    );

    let phase = 40.0 * 3.141592653589793 * camera.settings.z;
    let world = input.world_position;
    let wave = vec3<f32>(
        sin(phase + world.x * 0.7 + world.z * 0.5),
        0.0,
        cos(phase + world.x * 0.6 - world.z * 0.4),
    );
    let normal = normalize(input.normal + 0.15 * wave);

    let sky = input.light.x;
    let block = input.light.y;
    let ao = 0.55 + 0.15 * input.light.z;
    let sun_direction = camera.sun_direction.xyz;
    let ambient = camera.ambient_colour.rgb * sky;
    let sun = camera.sun_colour.rgb
        * max(dot(normal, sun_direction), 0.0)
        * max(sun_direction.y, 0.0)
        * sky;
    let moon = vec3<f32>(0.50, 0.60, 0.80)
        * max(dot(normal, -sun_direction), 0.0)
        * max(-sun_direction.y, 0.0)
        * sky;
    let lamp = vec3<f32>(1.00, 0.75, 0.45) * block;
    let lit = albedo.rgb * ao * (ambient + sun + moon + lamp);

    let fog_end = camera.settings.w;
    let fog = clamp((input.fog_distance - 0.7 * fog_end) / (0.3 * fog_end), 0.0, 1.0);
    let fogged = mix(lit, camera.sky_colour.rgb, fog);
    return vec4<f32>(clamp(fogged, vec3<f32>(0.0), vec3<f32>(1.0)), 0.65);
}
