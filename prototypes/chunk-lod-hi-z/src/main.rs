//! Headless prototype for chunk LOD-style GPU culling: frustum test + Hi-Z
//! depth-pyramid test + indirect draw record emission. Compute-only; see README.

#![deny(unsafe_code)]

use std::process::ExitCode;
use std::time::Duration;

use bytemuck::{Pod, Zeroable};
use glam::camera::rh;
use glam::{Mat4, Vec3, Vec4};

const CULL_WGSL: &str = include_str!("cull.wgsl");
const GRID: u32 = 64;
const CHUNK_SIZE: f32 = 32.0;
const CHUNK_COUNT: u32 = GRID * GRID;
const WORKGROUP_SIZE: u32 = 64;
const WORKGROUP_COUNT: u32 = CHUNK_COUNT.div_ceil(WORKGROUP_SIZE);
const HIZ_SIZE: u32 = 256;
const HIZ_MAX_LEVEL: u32 = 8;
const NEAR_PLANE: f32 = 1.0;
const FAR_PLANE: f32 = 2200.0;
const FOV_Y_DEGREES: f32 = 60.0;
const ASPECT: f32 = 16.0 / 9.0;
/// Synthetic occluder depth, in view-space metres.
const WALL_DEPTH: f32 = 1200.0;
/// Reverse-depth epsilon, roughly 5.5 m at this far plane.
const OCCLUSION_BIAS: f32 = 0.0025;
/// Only assert occlusion for chunks comfortably behind the synthetic wall.
const GROUND_TRUTH_MARGIN: f32 = 32.0;
const DRAW_ARGS_BYTES: usize = std::mem::size_of::<DrawIndexedIndirectArgs>();
const DRAW_ARGS_SIZE: u64 = DRAW_ARGS_BYTES as u64;

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("chunk-lod-hi-z: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Fallible<()> {
    if let Some(arg) = std::env::args().nth(1) {
        return Err(format!("unsupported argument `{arg}`; compute-only (no --render)").into());
    }

    let chunks = build_chunks();
    let (eye, forward, view_proj) = camera();
    let planes = frustum_planes(view_proj);
    let frustum_ids = chunks_in_frustum(&chunks, &planes);
    let expected_occluded = expected_occluded_ids(&chunks, &planes, eye, forward);
    ensure(!frustum_ids.is_empty(), "empty frustum: camera setup broken")?;
    ensure(!expected_occluded.is_empty(), "wall occludes nothing: setup broken")?;

    let params = Params {
        planes,
        view_proj: view_proj.to_cols_array_2d(),
        camera_pos: [eye.x, eye.y, eye.z, 1.0],
        camera_forward: [forward.x, forward.y, forward.z, 0.0],
        hiz: [HIZ_SIZE, HIZ_SIZE, HIZ_MAX_LEVEL, CHUNK_COUNT],
        depth: [FAR_PLANE, OCCLUSION_BIAS, 0.0, 0.0],
    };

    let (device, queue, info) = init_gpu()?;
    let hiz_empty = build_hiz(false);
    let hiz_wall = build_hiz(true);
    let resources = create_resources(&device, &queue, &params, &chunks, hiz_empty.len())?;
    let empty = run_scene(&device, &queue, &resources, &hiz_empty)?;
    let wall = run_scene(&device, &queue, &resources, &hiz_wall)?;
    verify(&empty, &wall, &frustum_ids, &expected_occluded, &info)
}

fn ensure(condition: bool, message: impl Into<String>) -> Fallible<()> {
    if condition { Ok(()) } else { Err(message.into().into()) }
}

// ---------------------------------------------------------------------------
// Synthetic world
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Chunk {
    min: [f32; 4],
    max: [f32; 4],
}

impl Chunk {
    fn aabb(&self) -> (Vec3, Vec3) {
        (
            Vec3::new(self.min[0], self.min[1], self.min[2]),
            Vec3::new(self.max[0], self.max[1], self.max[2]),
        )
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    planes: [[f32; 4]; 6],
    view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    camera_forward: [f32; 4],
    hiz: [u32; 4],
    depth: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Pod, Zeroable)]
struct DrawIndexedIndirectArgs {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
}

fn build_chunks() -> Vec<Chunk> {
    let mut chunks = Vec::with_capacity(CHUNK_COUNT as usize);
    for cz in 0..GRID {
        for cx in 0..GRID {
            let (x0, z0) = (cx as f32 * CHUNK_SIZE, cz as f32 * CHUNK_SIZE);
            chunks.push(Chunk {
                min: [x0, 0.0, z0, 0.0],
                max: [x0 + CHUNK_SIZE, chunk_height(cx, cz), z0 + CHUNK_SIZE, 1.0],
            });
        }
    }
    chunks
}

fn chunk_height(cx: u32, cz: u32) -> f32 {
    let wave = (cx as f32 * 0.23).sin() * (cz as f32 * 0.17).cos();
    let ridge = ((cx * 13 + cz * 29) % 23) as f32 / 23.0;
    32.0 + 56.0 * (wave * 0.5 + 0.5) + 48.0 * ridge
}

fn camera() -> (Vec3, Vec3, Mat4) {
    let centre = GRID as f32 * CHUNK_SIZE * 0.5;
    let eye = Vec3::new(centre, 420.0, 1900.0);
    let target = Vec3::new(centre, 60.0, 300.0);
    let view = rh::view::look_at_mat4(eye, target, Vec3::Y);
    let projection = rh::proj::directx::perspective(FOV_Y_DEGREES.to_radians(), ASPECT, NEAR_PLANE, FAR_PLANE);
    (eye, (target - eye).normalize(), projection * view)
}

fn frustum_planes(view_proj: Mat4) -> [[f32; 4]; 6] {
    let (r0, r1) = (view_proj.row(0), view_proj.row(1));
    let (r2, r3) = (view_proj.row(2), view_proj.row(3));
    [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2].map(normalise_plane)
}

fn normalise_plane(plane: Vec4) -> [f32; 4] {
    let normal = plane.truncate();
    let inverse_length = 1.0 / normal.length();
    [
        normal.x * inverse_length,
        normal.y * inverse_length,
        normal.z * inverse_length,
        plane.w * inverse_length,
    ]
}

fn inside_frustum(planes: &[[f32; 4]; 6], lo: Vec3, hi: Vec3) -> bool {
    planes.iter().all(|plane| {
        let normal = Vec3::new(plane[0], plane[1], plane[2]);
        let corner = Vec3::new(
            if normal.x < 0.0 { lo.x } else { hi.x },
            if normal.y < 0.0 { lo.y } else { hi.y },
            if normal.z < 0.0 { lo.z } else { hi.z },
        );
        normal.dot(corner) + plane[3] >= 0.0
    })
}

fn nearest_view_depth(eye: Vec3, forward: Vec3, lo: Vec3, hi: Vec3) -> f32 {
    let centre = (lo + hi) * 0.5;
    let half_extent = (hi - lo) * 0.5;
    forward.dot(centre - eye) - forward.abs().dot(half_extent)
}

fn chunks_in_frustum(chunks: &[Chunk], planes: &[[f32; 4]; 6]) -> Vec<u32> {
    chunks
        .iter()
        .enumerate()
        .filter_map(|(index, chunk)| {
            let (lo, hi) = chunk.aabb();
            inside_frustum(planes, lo, hi).then_some(index as u32)
        })
        .collect()
}

/// Chunks well behind the synthetic wall. These must never be emitted by the
/// compute pass in the wall scene.
fn expected_occluded_ids(chunks: &[Chunk], planes: &[[f32; 4]; 6], eye: Vec3, forward: Vec3) -> Vec<u32> {
    chunks
        .iter()
        .enumerate()
        .filter_map(|(index, chunk)| {
            let (lo, hi) = chunk.aabb();
            let visible = inside_frustum(planes, lo, hi);
            let depth = nearest_view_depth(eye, forward, lo, hi);
            (visible && depth > WALL_DEPTH + GROUND_TRUTH_MARGIN).then_some(index as u32)
        })
        .collect()
}

/// Build a flat min pyramid. Level 0 is 256x256, level 8 is 1x1.
fn build_hiz(wall: bool) -> Vec<f32> {
    let mut level = vec![0.0f32; (HIZ_SIZE * HIZ_SIZE) as usize];
    if wall {
        level.fill(1.0 - WALL_DEPTH / FAR_PLANE);
    }
    let mut all_levels = level.clone();
    let (mut width, mut height) = (HIZ_SIZE, HIZ_SIZE);
    while width > 1 || height > 1 {
        let (next_width, next_height) = ((width / 2).max(1), (height / 2).max(1));
        let mut next = vec![0.0f32; (next_width * next_height) as usize];
        for y in 0..next_height {
            for x in 0..next_width {
                let mut minimum = f32::INFINITY;
                for dy in 0..2 {
                    for dx in 0..2 {
                        let sx = (x * 2 + dx).min(width - 1);
                        let sy = (y * 2 + dy).min(height - 1);
                        minimum = minimum.min(level[(sy * width + sx) as usize]);
                    }
                }
                next[(y * next_width + x) as usize] = minimum;
            }
        }
        all_levels.extend_from_slice(&next);
        level = next;
        (width, height) = (next_width, next_height);
    }
    all_levels
}

// ---------------------------------------------------------------------------
// GPU
// ---------------------------------------------------------------------------

struct Resources {
    pipeline: wgpu::ComputePipeline,
    bind_group: wgpu::BindGroup,
    hiz: wgpu::Buffer,
    counter: wgpu::Buffer,
    ids: wgpu::Buffer,
    draws: wgpu::Buffer,
    readback: wgpu::Buffer,
}

#[derive(Debug)]
struct SceneResult {
    visible_count: u32,
    visible_ids: Vec<u32>,
    draws: Vec<DrawIndexedIndirectArgs>,
}

fn init_gpu() -> Fallible<(wgpu::Device, wgpu::Queue, wgpu::AdapterInfo)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .map_err(|error| format!("no Vulkan adapter: {error}. This prototype needs a real GPU; it has no CPU fallback."))?;

    let info = adapter.get_info();
    if info.device_type == wgpu::DeviceType::Cpu {
        return Err(format!("adapter `{}` is a CPU device; not a GPU verification", info.name).into());
    }
    if !adapter.features().contains(wgpu::Features::INDIRECT_FIRST_INSTANCE) {
        return Err(format!("adapter `{}` lacks INDIRECT_FIRST_INSTANCE", info.name).into());
    }
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("chunk-lod-hi-z"),
        required_features: wgpu::Features::INDIRECT_FIRST_INSTANCE,
        required_limits: wgpu::Limits::default(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
    }))
    .map_err(|error| format!("failed to create device on `{}`: {error}", info.name))?;
    Ok((device, queue, info))
}

fn make_buffer(device: &wgpu::Device, label: &str, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage,
        mapped_at_creation: false,
    })
}

fn create_resources(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    params: &Params,
    chunks: &[Chunk],
    hiz_float_count: usize,
) -> Fallible<Resources> {
    use wgpu::BufferUsages as Usage;

    let chunk_bytes = bytemuck::cast_slice(chunks);
    let ids_size = u64::from(CHUNK_COUNT) * 4;
    let draws_size = u64::from(CHUNK_COUNT) * DRAW_ARGS_SIZE;
    let readback_size = 4 + ids_size + draws_size;
    let [
        chunk_buffer,
        hiz_buffer,
        params_buffer,
        counter_buffer,
        ids_buffer,
        draws_buffer,
        readback,
    ] = [
        ("chunks", chunk_bytes.len() as u64, Usage::STORAGE | Usage::COPY_DST),
        ("hiz", hiz_float_count as u64 * 4, Usage::STORAGE | Usage::COPY_DST),
        (
            "params",
            std::mem::size_of::<Params>() as u64,
            Usage::STORAGE | Usage::COPY_DST,
        ),
        ("count", 4, Usage::STORAGE | Usage::COPY_DST | Usage::COPY_SRC),
        ("ids", ids_size, Usage::STORAGE | Usage::COPY_SRC),
        ("draws", draws_size, Usage::STORAGE | Usage::COPY_SRC),
        ("readback", readback_size, Usage::MAP_READ | Usage::COPY_DST),
    ]
    .map(|(label, size, usage)| make_buffer(device, label, size, usage));
    queue.write_buffer(&chunk_buffer, 0, chunk_bytes);
    queue.write_buffer(&params_buffer, 0, bytemuck::bytes_of(params));

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("cull bind group layout"),
        entries: &[
            storage_entry(0, true),
            storage_entry(1, true),
            storage_entry(2, false),
            storage_entry(3, false),
            storage_entry(4, false),
            storage_entry(5, true),
        ],
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("chunk cull"),
        source: wgpu::ShaderSource::Wgsl(CULL_WGSL.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("cull pipeline layout"),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("chunk cull pipeline"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("cull bind group"),
        layout: &bind_group_layout,
        entries: &[
            binding(0, &chunk_buffer),
            binding(1, &hiz_buffer),
            binding(2, &counter_buffer),
            binding(3, &draws_buffer),
            binding(4, &ids_buffer),
            binding(5, &params_buffer),
        ],
    });
    Ok(Resources {
        pipeline,
        bind_group,
        hiz: hiz_buffer,
        counter: counter_buffer,
        ids: ids_buffer,
        draws: draws_buffer,
        readback,
    })
}

fn storage_entry(index: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: index,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn binding<'a>(index: u32, buffer: &'a wgpu::Buffer) -> wgpu::BindGroupEntry<'a> {
    wgpu::BindGroupEntry {
        binding: index,
        resource: buffer.as_entire_binding(),
    }
}

fn run_scene(device: &wgpu::Device, queue: &wgpu::Queue, resources: &Resources, hiz: &[f32]) -> Fallible<SceneResult> {
    queue.write_buffer(&resources.hiz, 0, bytemuck::cast_slice(hiz));
    queue.write_buffer(&resources.counter, 0, bytemuck::bytes_of(&0u32));
    let ids_size = u64::from(CHUNK_COUNT) * 4;
    let draws_size = u64::from(CHUNK_COUNT) * DRAW_ARGS_SIZE;

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("chunk cull encoder"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("chunk cull pass"),
            ..Default::default()
        });
        pass.set_pipeline(&resources.pipeline);
        pass.set_bind_group(0, &resources.bind_group, &[]);
        pass.dispatch_workgroups(WORKGROUP_COUNT, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&resources.counter, 0, &resources.readback, 0, 4);
    encoder.copy_buffer_to_buffer(&resources.ids, 0, &resources.readback, 4, ids_size);
    encoder.copy_buffer_to_buffer(&resources.draws, 0, &resources.readback, 4 + ids_size, draws_size);

    let submission = queue.submit(Some(encoder.finish()));
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(Duration::from_secs(30)),
        })
        .map_err(|error| format!("GPU poll failed: {error}"))?;

    let bytes = map_read(device, &resources.readback, 4 + ids_size + draws_size)?;
    let visible_count = read_u32(&bytes)?;
    ensure(visible_count <= CHUNK_COUNT, "counter overflow")?;
    let id_start = 4usize;
    let id_end = id_start + visible_count as usize * 4;
    let visible_ids = bytes[id_start..id_end]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| u32::from_le_bytes(*bytes))
        .collect();
    let draw_start = 4 + ids_size as usize;
    let draw_end = draw_start + visible_count as usize * DRAW_ARGS_BYTES;
    let draws = bytes[draw_start..draw_end]
        .as_chunks::<DRAW_ARGS_BYTES>()
        .0
        .iter()
        .map(|bytes| bytemuck::pod_read_unaligned::<DrawIndexedIndirectArgs>(&bytes[..]))
        .collect();

    Ok(SceneResult {
        visible_count,
        visible_ids,
        draws,
    })
}

fn map_read(device: &wgpu::Device, buffer: &wgpu::Buffer, size: u64) -> Fallible<Vec<u8>> {
    let slice = buffer.slice(0..size);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|error| format!("GPU poll failed while mapping: {error}"))?;
    receiver
        .recv()
        .map_err(|error| format!("map callback was dropped: {error}"))?
        .map_err(|error| format!("buffer map failed: {error}"))?;
    let bytes = slice
        .get_mapped_range()
        .map_err(|error| format!("get_mapped_range failed: {error}"))?
        .to_vec();
    buffer.unmap();
    Ok(bytes)
}

fn read_u32(bytes: &[u8]) -> Fallible<u32> {
    let array = *bytes.first_chunk::<4>().ok_or("short visible counter")?;
    Ok(u32::from_le_bytes(array))
}

// ---------------------------------------------------------------------------
// Verification and report
// ---------------------------------------------------------------------------

fn sorted_unique(ids: Vec<u32>, label: &str) -> Fallible<Vec<u32>> {
    let mut sorted = ids;
    sorted.sort_unstable();
    ensure(
        sorted.windows(2).all(|pair| pair[0] != pair[1]),
        format!("{label} emitted duplicate chunk ids"),
    )?;
    Ok(sorted)
}

fn validate(result: &SceneResult, label: &str) -> Fallible<()> {
    ensure(
        result.visible_count as usize == result.visible_ids.len() && result.visible_ids.len() == result.draws.len(),
        format!("{label}: counter, id and draw buffer lengths disagree"),
    )?;
    for (slot, record) in result.draws.iter().enumerate() {
        ensure(
            record.index_count == 6
                && record.instance_count == 1
                && record.first_index == 0
                && record.base_vertex == 0
                && record.first_instance == result.visible_ids[slot],
            format!("{label}: bad indirect record {slot}: {record:?}"),
        )?;
    }
    Ok(())
}

fn verify(
    empty: &SceneResult,
    wall: &SceneResult,
    frustum_ids: &[u32],
    expected_occluded: &[u32],
    info: &wgpu::AdapterInfo,
) -> Fallible<()> {
    let mut expected_frustum = frustum_ids.to_vec();
    expected_frustum.sort_unstable();
    let empty_ids = sorted_unique(empty.visible_ids.clone(), "empty scene")?;
    ensure(
        empty_ids == expected_frustum,
        "empty scene visible set does not match CPU frustum",
    )?;
    validate(empty, "empty")?;

    let wall_ids = sorted_unique(wall.visible_ids.clone(), "wall scene")?;
    ensure(wall.visible_count < empty.visible_count, "wall scene culled nothing")?;
    validate(wall, "wall")?;

    // Every emitted id must have survived the CPU frustum test; membership in
    // the empty scene is equivalent to frustum-visible.
    for &id in &wall_ids {
        ensure(
            empty_ids.binary_search(&id).is_ok(),
            format!("wall drew chunk {id} outside the empty-scene frustum"),
        )?;
    }
    // Every chunk comfortably behind the synthetic wall must be absent.
    for &id in expected_occluded {
        ensure(
            wall_ids.binary_search(&id).is_err(),
            format!("occluded chunk {id} was emitted by the wall scene"),
        )?;
    }

    let occluded = empty.visible_count - wall.visible_count;
    ensure(
        occluded >= expected_occluded.len() as u32,
        format!(
            "culled {occluded} chunks but ground truth requires {}",
            expected_occluded.len()
        ),
    )?;
    let frustum_culled = CHUNK_COUNT - empty.visible_count;
    let ratio = f64::from(occluded) / f64::from(empty.visible_count) * 100.0;

    println!("adapter: {} ({:?})", info.name, info.backend);
    println!(
        "HIZ chunks={CHUNK_COUNT} visible={} occluded={} max_level={HIZ_MAX_LEVEL}",
        wall.visible_count, occluded
    );
    println!(
        "HIZ empty chunks={CHUNK_COUNT} visible={} occluded=0 max_level={HIZ_MAX_LEVEL}",
        empty.visible_count
    );
    println!(
        "report: scene=wall frustum={} frustum_culled={} ground_truth_occluded={} hi_z_occluded={} occlusion_ratio={ratio:.1}% draws_checked={}",
        empty.visible_count,
        frustum_culled,
        expected_occluded.len(),
        occluded,
        wall.draws.len()
    );
    println!(
        "report: scene=empty cpu_frustum={} gpu_visible={} exact_match=true",
        expected_frustum.len(),
        empty.visible_count
    );
    println!("verify: OK");
    Ok(())
}
