struct Camera {
    view_projection: mat4x4<f32>,
};

struct Chunk {
    origin: vec3<f32>,
    padding: f32,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

@group(1) @binding(0)
var<uniform> chunk: Chunk;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
};

@vertex
fn vs_main(input: VertexIn) -> VertexOut {
    var output: VertexOut;
    let world_position = chunk.origin + input.position;
    output.clip_position = camera.view_projection * vec4<f32>(world_position, 1.0);
    output.normal = input.normal;
    return output;
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let light_direction = normalize(vec3<f32>(0.35, 0.85, 0.45));
    let shade = 0.45 + 0.55 * max(dot(normalize(input.normal), light_direction), 0.0);
    let colour = vec3<f32>(0.32, 0.62, 0.38);
    return vec4<f32>(colour * shade, 1.0);
}
