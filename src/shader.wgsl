struct Camera {
    view_projection: mat4x4<f32>,
    sky_colour: vec4<f32>,
    ambient_colour: vec4<f32>,
    sun_colour: vec4<f32>,
    sun_direction: vec4<f32>,
    // x: unlit flag, y: far plane, z: time of day, w: fog end distance.
    settings: vec4<f32>,
    camera_position: vec4<f32>,
};

struct Chunk {
    origin: vec3<f32>,
    padding: f32,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

@group(1) @binding(0)
var<uniform> chunk: Chunk;

@group(2) @binding(0)
var block_texture: texture_2d_array<f32>;

@group(2) @binding(1)
var block_sampler: sampler;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) layer: u32,
    @location(4) light_ao: u32,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) layer: u32,
    // x: sky, y: block, z: AO level.
    @location(3) light: vec3<f32>,
    @location(4) fog_distance: f32,
    @location(5) world_position: vec3<f32>,
};

@vertex
fn vs_main(input: VertexIn) -> VertexOut {
    var output: VertexOut;
    let world_position = chunk.origin + input.position;
    output.clip_position = camera.view_projection * vec4<f32>(world_position, 1.0);
    output.normal = input.normal;
    output.uv = input.uv;
    output.layer = input.layer;
    output.light = vec3<f32>(
        f32(input.light_ao & 0xFu) / 15.0,
        f32((input.light_ao >> 4u) & 0xFu) / 15.0,
        f32((input.light_ao >> 8u) & 0x3u),
    );
    output.fog_distance = length(world_position - camera.camera_position.xyz);
    output.world_position = world_position;
    return output;
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

    // v0.0.4 unlit debug fallback.
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
