struct Camera {
    view_projection: mat4x4<f32>,
    sky_colour: vec4<f32>,
    ambient_colour: vec4<f32>,
    sun_colour: vec4<f32>,
    sun_direction: vec4<f32>,
    settings: vec4<f32>,
    // `camera_position.w` carries break progress for the highlight colour.
    camera_position: vec4<f32>,
    render_origin: vec4<i32>,
    wave_anchor: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

struct VertexIn {
    @location(0) position: vec3<f32>,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
};

@vertex
fn vs_main(input: VertexIn) -> VertexOut {
    var output: VertexOut;
    output.clip_position = camera.view_projection * vec4<f32>(input.position, 1.0);
    return output;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    let progress = clamp(camera.camera_position.w, 0.0, 1.0);
    let idle = vec3<f32>(1.0, 0.92, 0.45);
    let breaking = vec3<f32>(1.0, 0.30, 0.08);
    let pulse = 0.85 + 0.15 * sin(progress * 30.0);
    return vec4<f32>(mix(idle, breaking, progress) * pulse, 1.0);
}
