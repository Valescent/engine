# Lighting pipeline

**Status:** Research; target for v0.0.9, not yet scheduled.

**Kind:** Design direction and technique survey. This page is not a milestone scope, a spec, or an
[ADR](../decisions/index.md). Nothing here is committed until a later ADR and spec say so. If this
page disagrees with a spec, the spec wins.

## Why this page exists

v0.0.8 finishes the scale work. The next two milestones are set by the owner: **v0.0.9 reworks
lighting, v0.0.10 reworks terrain** into the 1 m block plus 4 cm detail model. Terrain detail is
mostly a representation and meshing problem. Lighting is the thing that decides whether the world
looks like the reference devlogs or like a high-resolution Minecraft, so it comes first.

This page answers one question: what lighting pipeline makes a 1 m voxel world with 4 cm detail look
like the reference, on this engine, in `wgpu`, at 60 FPS?

> **The bet:** the look comes from light transport and image formation, not from voxel size. A
> directionless ambient term cannot be fixed by adding geometry. Every lighting term is a pass with
> a fallback, a budget, and a debug view, and the CPU keeps authority over what is lit.

## What v0.0.8 ships

The current model is a Minecraft-class lighting model with better plumbing.

| Area | Current behaviour | Where |
|---|---|---|
| Sky and block light | 4-bit BFS flood fill over a `3x3x3` chunk neighbourhood, one nibble per cell | `src/v0/light.rs` |
| Propagation | Six-connected, one level per step, water passes without attenuation | `spread` in `light.rs` |
| Storage | Derived, never saved, recomputed per chunk on the worker pool | `LightSnapshot`, `LightJob` |
| Vertex light | Sky, block, and 2-bit corner AO packed into one `u32` | `src/v0/mesh.rs` |
| Shading | `albedo * ao * (ambient * sky + sun * N·L * sky + moon * N·L * sky + lamp * block)` | `src/shader.wgsl` |
| Sun | A direction and colour, no visibility test | `sky.rs` |
| Fog | Linear blend to the sky colour between 70% and 100% of the far plane | `shader.wgsl` |
| Output | Linear-ish colour clamped to `[0, 1]` and written to an sRGB8 surface | `renderer.rs` |

The model already answers the questions that a directional shadow map cannot: caves are dark,
overhangs are graded, lamps fall off through space, and everything is deterministic from block data.
What it cannot answer is **which way the light comes from**, **what the sky costs at a given point**,
and **what colour the second bounce is**. Those three are most of the difference between the current
render and the reference.

## The image, decomposed

Looking at the reference frames, the visible terms rank roughly like this by perceptual weight:

| # | Term | Weight | Current | Cost to add |
|---|---|---|---|---|
| 1 | Sun and moon visibility (cast shadows) | Very high | Missing | One cascade set |
| 2 | Directional sky light (up is brighter than down) | High | Flat ambient | Sky cubemap |
| 3 | Indirect bounce (colour bleed, filled corners) | High | Missing | Coarse coloured propagation |
| 4 | Exposure and tone mapping | High | Missing, clamped | One fullscreen pass |
| 5 | Aerial perspective (distance haze, light shafts later) | Medium | Linear fog | Height fog term |
| 6 | Contact occlusion | Medium | Per-vertex AO only | Occupancy AO plus GTAO |
| 7 | Glossy response (water, wet stone) | Medium | Animated flat water | IBL + screen-space |
| 8 | Emissive bloom | Low-medium | Missing | Bloom mips |
| 9 | Cloud shadows | Low-medium | Missing | Noise LUT |

Note what is not on the list: voxel resolution. 4 cm detail changes silhouettes and small-scale
normal variation. It does not change any row above. This is why lighting ships before terrain.

## Constraints

**Graphics API.** `wgpu` 30 is a WebGPU implementation, not a native renderer with extras:

- No hardware ray tracing. Every ray is a software DDA in a compute or fragment shader.
- Storage textures must be 2D. A 3D volume cannot be written from a compute shader, so volumes are
  built on the CPU and uploaded with `write_texture`, or emulated in storage buffers.
- `maxTextureDimension3D` defaults to `2048`; `maxBindGroups` is `4`.
- No bindless descriptor arrays. Lighting resources must fit in one or two bind groups.

This rules out the VXGI shape (GPU voxelization into mipmapped 3D storage textures) and pushes the
design toward **CPU-built volumes, GPU-consumed volumes**, which happens to match the existing
worker pipeline.

**World model.** 1 m blocks, `32³` chunks, four columns tall (128 blocks), render distance `8..=32`
chunks, three LODs at 1/2/4 blocks per cell. Lighting must survive streaming, edits, LOD selection,
and a 384 m horizon.

**Reference machine and budget.** 60 FPS at `render_distance_chunks = 24`. The lighting budget for
v0.0.9 is **4.5 ms at 1080p on the reference machine**, with a low tier that turns features off
rather than dropping frames.

**Ownership.** Light remains derived state. The CPU decides what emits, what is enclosed, and what a
chunk's light level is. The GPU decides what the camera sees. Nothing lighting-related is saved.

## Design pillars

1. **The CPU light field stays.** BFS sky and block light is the gameplay light and the cheapest
   leak guard available: it already knows a cave from a courtyard. It stays scalar and authoritative;
   the GPU adds colour, direction, and bounce on top. It does not replace it.
2. **Geometry is already on the CPU.** Volumes are populated from chunk data and, for the stretch
   stage, the existing LOD downsample, on the worker pool. No GPU voxelization. One derived occupancy
   field feeds AO, light propagation weighting, and later heat and water — derive it once, agree on
   its meaning.
3. **Every term is optional.** Each pass has a config toggle and a debug view so an A/B screenshot
   is one key press, and so a low tier is a subtraction rather than a rewrite.
4. **No TAA dependency.** Nothing in v0.0.9 may require a temporal antialiasing pass to look stable.
   Coarse propagation and probe GI are low frequency by construction; screen-space effects are not in
   this milestone.
5. **Measure or it did not happen.** Each stage gets a GPU timer scope and a frame-time line in the
   overlay, matching the existing `FrameStats` path.

## Survey

| Technique | Gives | Cost | Leaks | Dynamic edits | `wgpu` fit | Verdict |
|---|---|---|---|---|---|---|
| BFS voxel light (current) | Cave darkness, lamp falloff | Low, CPU | None | Immediate | Already shipping | **Keep as base term** |
| Coloured coarse-grid propagation + occupancy AO | Bounce, coloured lamps, crevice darkening | Low, GPU cellular automata | Occupancy weighting limits it | Immediate | Straightforward compute | **Take, core** |
| Cascaded shadow maps | Sun/moon visibility, contact | Low-medium | Acne, peter-panning | Immediate | Straightforward | **Take** |
| Sky IBL (cubemap + mips) | Directional ambient | Low | None | Per sun move | Straightforward | **Take** |
| GTAO | Contact occlusion | Low, half-res | Over-darkening | Immediate | Straightforward | **Take** |
| Height fog with sun term | Aerial perspective | Very low | None | Per frame | Straightforward | **Take** |
| DDGI probes | Diffuse bounce, colour bleed | Medium | Manageable with depth moments | Probes retrace | Storage buffers, no RT needed | **Stretch** |
| Voxel cone tracing (VXGI) | Bounce + glossy | High | Clipmap seams | Revoxelization | Awkward: needs 3D storage mips | **Reject** |
| SDFGI (Godot-style) | Dynamic GI over wide area | High | Cascade shifts, leaks | SDF rebuild | Heavy, no SDF infrastructure | **Reject** |
| Light propagation volumes | Bounce | Medium | Strong banding, leaks | Yes | Poor quality | **Reject** |
| Reflective shadow maps | One-bounce direct | Medium | Many | Yes | Needs G-buffer | **Reject** |
| Per-pixel path tracing + SVGF | Reference-grade | High | None | Yes | Needs TAA, denoiser, GPU world | **Later** |
| Hardware ray tracing | Reference-grade, cheaper | N/A | None | Yes | Unavailable | **Unavailable** |

Three notes on the table.

*Voxel cone tracing* is the classic answer and produces beautiful glossy bounce, but it wants a
mipmapped 3D radiance volume written by compute. WebGPU forbids 3D storage textures, so the mip chain
must be built on the CPU for every edited brick. The quality gain over probes is mostly specular,
which is not the v0.0.9 problem.

*Per-pixel path tracing* is the long-term target: a few hemisphere rays per pixel at reduced
resolution, accumulated over frames with a variance-guided denoiser. It looks superb and it is where
this ends up if the world ever lives on the GPU, but it needs a stable TAA history, SVGF, and the
whole world resident in VRAM. Adding it before shadows and tone mapping would be building the roof
first.

The reference engine in the devlogs is worth naming here, because it is not a path tracer and not a
DDGI implementation. Its author [describes](https://reddit.com/r/VoxelGameDev/comments/1uimbj6/adding_caves_to_my_micro_voxel_engine_devlog_3/oyj12qh/)
a coarse light volume running one cellular-automata propagation step per cell per frame, bounded to
the volumes around light sources, with intensity and colour propagation and
[a per-cell occupancy value](https://reddit.com/r/GraphicsProgramming/comments/1udkyzj/i_built_a_microvoxel_engine/)
that both weights propagation and darkens crevices. It costs about 0.3 ms. That is a different point
in the design space from probe GI and from BFS: cheaper than both, less directional than probes, and
it produces the coloured lamp falloff and dark crevices that make the frames read well. The lesson is
that colour and sub-block occupancy buy more image than transport accuracy does, so the milestone
budget goes there first.

## Decision: the v0.0.9 pipeline

Eight core stages plus one stretch stage. Each is independently toggleable.

```text
CPU per chunk                       GPU per frame
---------------------------         ------------------------------------------
generate -> BFS sky light  --->     A. shadow cascades        (depth only)
         -> occupancy build  --->   B. occupancy volume       (R8 3D, 1 m, worker-built)
         -> lamp list        --->   C. coloured propagation   (compute CA, storage buffer)
                                    D. opaque pass            (HDR)
                                         sky IBL x BFS sky light
                                         + propagated colour
                                         + shadowed sun
                                         + occupancy AO
                                    E. GTAO                   (half res)
                                    F. height fog
                                    G. water pass
                                    H. bloom mips
                                    I. composite: exposure, AgX, grade, dither
```

Stage 9 is the stretch: probe GI over a voxel clipmap, kept for directionality if stages 1-8 come in
under budget.

### 1. HDR and image formation

Add one `Rgba16Float` scene target with `RENDER_ATTACHMENT | TEXTURE_BINDING`, one `Depth32Float`
depth target (already exists), and a fullscreen composite pass.

- **Exposure.** Manual, driven by the time-of-day curve in `sky.rs`, with a `[lighting] exposure`
  multiplier. Automatic eye adaptation is deferred: it pumps during cave transitions and needs
  temporal smoothing that is out of scope.
- **Tonemap.** AgX by default. Its highlight desaturation and hue preservation are what make strong
  directional sun plus warm lamps read as filmic; a per-channel curve clips to ugly primaries. Keep
  Khronos PBR Neutral as an alternative for colour-accurate screenshots.
- **Grade.** Post-tonemap lift/gamma/gain plus saturation, all in one pass. No LUT in v0.0.9.
- **Dither.** A 4x4 ordered dither before the sRGB write kills banding in dark gradients, which a
  16-bit target does not fix after tonemap quantisation.
- **Bloom.** Five-mip dual-filter bloom at quarter resolution, threshold from the exposure, tinted
  by the sun colour. It is what makes lamps and sun-lit water look expensive.

The composite pass is also where per-effect debug views live (`sky`, `sun`, `propagation`, `ao`,
`shadow`, `final`), matching the existing unlit debug flag.

This stage changes no world data and no CPU work, which is why it goes first.

### 2. Sun and moon shadows

Three cascades, `2048 x 2048` `Depth32Float` each, fitted to the camera frustum with a stable
texel-snapped light-space origin to stop shimmering.

| Cascade | Range | Chunks drawn | Bias |
|---|---|---|---|
| 0 | 32 m | LOD0 | Normal offset 0.02 m, slope-scaled depth bias |
| 1 | 96 m | LOD1 | Same, larger offset |
| 2 | 288 m | LOD2 | Same, larger offset |

- Voxel geometry is axis-aligned, so bias tuning is stable and acne is a solved problem once the
  normal offset is right. Use 3x3 PCF; the reference's softness comes from distance, not from a
  large kernel.
- Cascade selection per fragment with a 10% blend band, cross-faded in the shadow lookup. No
  cascaded shadow map stitching artifacts around chunk seams.
- Shadow casters are chunk meshes. Water does not cast. Leaves cast, and are dense enough in the
  current tree positions to be worth keeping.
- LOD2 shadows are blocky at 288 m. That is correct: it matches the LOD2 silhouettes a player sees
  at that distance.
- The shadow term multiplies only the sun and moon, never ambient: ambient has its own visibility
  from the sky cubemap, the occupancy volume, and the baked sky light.
- Cloud shadows: one scrolling noise LUT sampled in the same term. Cheap, and it ties the sky to
  the ground.

Rejected alternative: ray-marched sun shadows against a clipmap. They give contact-hardening for
free and need no extra passes, but they cost a full-screen ray march before the clipmap exists, and
they lose the cascade-to-LOD match. Once the clipmap lands for stage 9, this becomes a candidate to
replace cascade 0 only.

### 3. The occupancy field

One derived field, defined once, consumed by lighting now and by water and heat later.

**Definition.** One byte per `1 m` cell: the fraction of the cell volume that is solid, `0..=255`.
Today a cell is one block, so the value is binary. After v0.0.10 adds 4 cm detail inside the block,
the same byte becomes continuous with no format change. That is the point of defining it now.

**Storage.** A `128 x 64 x 128` region at 1 m centred on the player, `R8Unorm`, `COPY_DST |
TEXTURE_BINDING`. That is 1 MB, not 8, because it is one channel. It is a 3D texture rather than a
compute-writable volume because WebGPU forbids 3D storage textures; the CPU fills it, which is where
the data already lives.

**Population.** Built on the worker pool from the same chunk snapshots generation already produces,
in `32 x 32 x 32` bricks aligned to chunks. A committed chunk or an edit writes its bricks with
`queue.write_texture` from a staging buffer; the main thread only submits. No per-frame scan.

**Occlusion weight.** Propagation and shading read the cell plus its six neighbours and compute
`openness = 1 - mean(occupancy of the six neighbours)`. At 1 m blocks that is exactly the fraction of
open neighbours; after detail lands the same expression reads fractional occupancy. No extra storage,
no format migration.

**Why bother.** It is the cheapest approximation of "how enclosed is this space" that is not a ray
cast. It darkens crevices, gates propagation, and later gates water flow and heat insulation. The
reference engine uses exactly this one field for lighting, heat, and water, and that reuse is the
reason its simulation stack stays cheap.

### 4. Coloured light propagation

This is the core new lighting subsystem, and it replaces the probe grid as the source of local bounce.

**Grid.** The same `128 x 64 x 128` region as the occupancy field, at 1 m cells. Light lives in a
`RGBA8` storage buffer, `4 MB`, double-buffered so a frame never reads what it is writing. RGB is
colour times intensity, A is reserved.

**Step.** A cellular automaton, not a BFS. Each iteration, every cell decays its own value, takes the
strongest of its six neighbours, attenuates that by the destination cell's occupancy, applies a
per-material decay, and keeps the maximum of the two. Colour rides along with intensity, so a torch
gives orange falloff and a lamp gives warm white. The self-decay is what lets a broken lamp fade out
instead of leaving its light stranded in the buffer.

- Two iterations per frame, bounded to the volume around each emitter. Static lamps converge in a
  handful of frames; nothing about the cost depends on how many cells are lit, only how many
  emitters are active.
- Emitters are seeded from the existing lamp list each frame: position, colour, intensity. That list
  is a few dozen entries at most.
- Propagation does not handle sky light. BFS already does that exactly, on the CPU, for free. Do not
  duplicate it.
- An edit re-seeds the affected cells and the automaton reconverges over the next few frames. A
  torch going out fades over roughly a second, which reads as a lamp dimming rather than a pop.

**Why an automaton rather than BFS on the GPU.** A queue-based BFS has unbounded worst-case cost and
needs atomics on a shared frontier. A fixed iteration count per frame has a flat cost, is trivial to
bound, and converges to the same steady state for static lights. For dynamic lamps, latency of a few
frames is invisible.

**Consumption.** The opaque fragment shader samples the buffer trilinearly by hand (eight loads) at
the shaded point, adds it to the ambient term, and multiplies by the surface albedo. There is no copy
back into a texture and no third pass.

**Fallback.** Outside the volume, or with the stage disabled, shading uses the existing scalar block
light. The pipeline must look coherent with every stage off, not merely survive it.

**Cost.** `0.4 ms` at two iterations over the bounded region, `8 MB` for the double buffer. The
occupancy gate is what keeps it that low: without it, light floods through walls and the region has to
grow to compensate.

### 5. Sky light and the leak guard

Render the existing time-of-day sky into a small cubemap (`128 x 128` per face, six mips), prefilter
it, and sample along the world normal per fragment. This is the entire "directional ambient" jump: an
overhang's underside becomes dark, a hillside facing the sun becomes warm, and a wall facing away
cools down. Update one face per frame while the sun moves.

Two guards keep it honest:

- **Baked sky light scales the result**, `sky_light / 15` as a soft visibility mask. A cave with a
  propagation leak still reads dark because BFS says its sky access is zero. This is the single
  cheapest anti-leak measure in the design and the reason the CPU light field stays.
- **Occupancy openness** multiplies the ambient term, so narrow slots and overhangs lose sky before
  GTAO even runs.

### 6. Contact occlusion

Half-resolution GTAO over the depth buffer, 8 slices by 8 steps, depth-aware upsample, multiplied
into the ambient and propagation terms.

Three occlusion terms now exist, and they must not be multiplied together:

| Term | Scale | Source |
|---|---|---|
| Vertex AO | Geometry, 1 m corners | Baked into the mesh |
| Occupancy AO | Crevices, 1 m cells | Derived field |
| GTAO | Contact, centimetres to metres | Screen space |

Combine with `max` of the three scaling factors. Multiply and every corner crushes to black; this is
the most likely visual regression in the milestone, so it gets a fixed test scene.

### 7. Atmosphere

Replace the linear fog blend with an analytic exponential height fog evaluated in the fragment
shader:

- Density falls off with altitude, so valleys hold haze and peaks do not.
- In-scattering is tinted by the sun colour and boosted toward the sun by a cheap Henyey-Greenstein
  lobe. This gives the warm rim around the sun at dawn without a volumetric pass.
- Blend against the sky cubemap, not a flat `sky_colour`, so the horizon matches the actual sky.
- Keep the current 70-100% fade band as the outer limit so the streaming edge stays hidden.

Froxel volumetrics and ray-marched clouds are not in v0.0.9. Cloud shadows are (see above), because
they cost one texture fetch.

### 8. Water and glossy response

Water already has animated normals. Add:

- Sky cubemap reflection with a Schlick Fresnel term, tinted by depth.
- Propagated colour for the diffuse term of shallow water and the terrain under it.
- Specular highlight from the sun, so the sun path across water is visible.
- Screen-space reflections are deferred. A `128 x 128` sky cubemap plus the sun highlight covers
  the reference look at the current art scale.

Solid blocks stay Lambertian in v0.0.9. There is no roughness or metallic channel, no normal maps,
and no material system. The reference look does not need them at 1 m blocks; it needs direction and
colour.

### 9. Stretch: probe GI over a voxel clipmap

Only if stages 1-8 land under budget. This buys what propagation cannot: directionality, bounce into
enclosed rooms, and colour bleeding from large surfaces.

**Probe grid.** One cascade: 2 m spacing over a `64 x 32 x 64 m` box centred on the player,
`33 x 17 x 33 = 18,513` probes. Probes scroll with the player in whole cells; displaced probes copy
their cached radiance and retrace over the next frames. Probes in solid cells relocate to the nearest
open cell and skip rendering until then.

**Probe contents**, per the DDGI layout:

| Buffer | Resolution | Format | Bytes |
|---|---|---|---|
| Irradiance | 8x8 octahedral | `R11G11B10F` packed into a `u32` | 256 |
| Depth moments | 16x16 octahedral | `RG16F` (mean and mean squared) | 1,024 |
| **Total** | | | **~1.2 KB** |

`18,513 x 1.2 KB = 22 MB`, stored in two storage buffers with a one-texel gutter so bilinear
sampling never crosses a probe boundary. The depth moments drive a Chebyshev visibility test, which
is what separates DDGI from classic irradiance probes and what stops bounce light shining through
walls.

**Clipmap.** The ray-tracing target, built on the CPU and reusing the LOD path:

| Cascade | Cell size | Extent | Cells | Memory (`RGBA8`) |
|---|---|---|---|---|
| 0 | 1 m | 128 m | `128 x 128 x 128` | 8.4 MB |
| 1 | 2 m | 256 m | `128 x 128 x 128` | 8.4 MB |
| 2 | 4 m | 512 m | `128 x 128 x 128` | 8.4 MB |

Each cascade is an aligned group of `4 x 4 x 4` chunks: the source data is already `32 x 32 x 32`, the
existing LOD1/LOD2 downsample produces the 2 m and 4 m versions, and the worker pool already runs the
code. RGB carries albedo, A carries solidity.

Traversal is a two-level DDA: a brick-level skip over `32 x 32 x 32` bricks (one bit each), then a
cell-level DDA inside a non-empty brick. The brick is the chunk, so the hierarchy costs nothing extra.

**Update budget.** `18,513 x 64 = 1.18 M` rays if everything retraces in one frame. Update a fixed
quota per frame (1/8 of probes, round robin, nearest first) with hysteresis. Target `1.5 ms`, `47 MB`
including the clipmap. Edits dirty the bricks they touch and those probes retrace first.

**Consumption.** Trilinear weights over the eight surrounding probes, each weighted by backface
culling and the Chebyshev visibility term, plus a view bias. Added to ambient and sun in the opaque
shader. No denoiser: probe irradiance is low frequency by construction.

**Not sampled per pixel.** One hemisphere ray per pixel needs a denoiser and TAA to be stable. Probes
amortise the work across many pixels. The cost is blur inside a 2 m room, which is acceptable for a
voxel world whose smallest gameplay space is a block.


## Memory and frame budget

At 1080p, `render_distance_chunks = 12`, reference machine:

| Stage | Budget | Memory |
|---|---|---|
| Shadow cascades (3 x `2048 x 2048`) | 1.5 ms | 50 MB |
| Occupancy volume (`128 x 64 x 128`, R8) | worker-built, upload only | 1 MB |
| Coloured propagation (2 iterations, double-buffered `RGBA8`) | 0.4 ms | 8 MB |
| Sky cubemap | 0.1 ms amortised | 1 MB |
| GTAO (half res) | 0.7 ms | 4 MB |
| Bloom | 0.4 ms | 2 MB |
| Composite | 0.3 ms | 10 MB (HDR target) |
| **Core total** | **~3.4 ms** | **~76 MB** |
| Stretch: probe grid + clipmap | +1.5 ms | +47 MB |

The core is a quarter cheaper and 38 MB lighter than the probe-first plan, which is what buys the
stretch stage its room. A low tier drops shadow cascade 2 and half-res GTAO, and runs propagation on
2 m cells: `~1.9 ms`, `~49 MB`. Every tier is a subtraction, and no tier changes what the CPU
computes.

## Failure modes

| Symptom | Cause | Mitigation |
|---|---|---|
| Light through a 1-block wall | Propagation attenuation too weak | Occupancy gate plus neighbour term, tune decay per material |
| Light crawling across a wall | Too few iterations per frame | Two iterations, bounded region, fade on convergence if visible |
| Lamps take a second to light | Automaton latency | Accept for static lamps; seed cells directly at the emitter |
| Warm corners saturated to white | RGB8 clipping in the buffer | Reserve headroom, tonemap in linear, clamp only at composite |
| Colour bleeds across biomes | No material absorption | Sample material at each step during propagation |
| Cost scales with lamp count | Unbounded region per emitter | Radius cap per emitter, budget the emitter list |
| Shadow acne on flat ground | Bias too small for cascade texel size | Normal offset plus slope-scaled depth bias, per cascade |
| Peter-panning on thin blocks | Normal offset too large | Cap at half a block; leaves use a smaller offset |
| Cascade seam band | Blend band too narrow | 10% cross-fade, texel-snapped origins |
| Cavities crushed black | GTAO times vertex AO times occupancy | Combine with `max`, not product |
| Sunset banding | 8-bit output, no dither | Ordered dither in the composite |
| Frame spikes on load | Occupancy upload on the main thread | Staging buffer built on the worker pool |
| Stretch: bounce through a wall | Probe inside geometry | Relocation, Chebyshev depth test, baked sky light multiplier |
| Stretch: visible probe lattice | Trilinear on a sparse grid | Hysteresis, 2 m spacing, view bias |

## Testing

CPU-testable, and therefore required:

- Cascade fitting: split distances, texel snapping, orthographic matrices, lid stability under
  camera rotation.
- Occupancy derivation: cell from block, solid fraction, brick alignment with `32³` chunks, boundary
  bricks, volume-to-world mapping, player recentring, edit invalidation.
- Propagation: monotonic decay along a line, no energy growth across iterations, colour carried with
  intensity, emitter seeding, radius bound, steady state independent of iteration order.
- Config parsing and tier selection, including every combination of toggles.
- Tonemap and dither functions: monotonic, no NaN at zero, exact black, exact white.
- Stretch only: probe addressing, octahedral round trip, gutter indexing, relocation, clipmap DDA
  against a hand-built volume.

Not unit-testable, and therefore manual with recorded captures:

- Shadow acne and peter-panning across a fixed scene at fixed sun angles.
- Propagation convergence after a dig, light in a one-block-wall room, and a lamp being broken.
- Combined occlusion at a corner: vertex AO, occupancy AO, and GTAO must not stack to black.
- Banding in a dusk gradient at 1080p.
- A/B screenshots for every stage toggle, stored beside the release notes.

The existing rule holds: no golden GPU images in CI, because driver differences make them lie.

## Slices

1. **HDR and composite.** Scene target, exposure, AgX, grade, dither, debug views. No new world data.
2. **Shadows.** Three cascades, PCF, per-LOD casting, cloud shadows.
3. **Occupancy field.** Derived on the worker pool, `R8` volume, brick upload, debug view. No shader
   change yet, so this slice is testable on its own.
4. **Coloured propagation.** Emitter list, automaton, buffer sampling in the opaque pass, edit
   invalidation, lamp breaking. This is the slice that changes the look.
5. **Sky, occlusion, atmosphere.** Cubemap IBL, sky-light leak guard, occupancy AO, GTAO, height fog.
6. **Water and polish.** IBL reflection, propagated colour, sun highlight, bloom tuning, capture set.
7. **Budget and tiers.** Timers per stage, low tier, memory assertions, benchmarks, docs.
8. **Stretch: probes.** Clipmap, probe grid, blending, consumption, relocation.

Slices 1-5 are the milestone. Slice 3 is deliberately shippable before any shading changes, and
slice 4 can be A/B'd against the old scalar light. If slice 4 overruns, the fallback is the existing
BFS path with the new HDR and shadows, which already looks different.

## What this is not

- Not baked lighting. The world is destructible and streamed.
- Not a material system. No roughness, metallic, or normal maps.
- Not screen-space GI, SSR, or ray-traced reflections.
- Not froxel volumetrics, volumetric clouds, or weather.
- Not 4 cm lighting. Detail is shaded by the 1 m terms plus local AO; per-detail lighting is a
  v0.0.10 problem and probably a normal/AO problem, not a transport problem.
- Not TAA. If a technique needs temporal antialiasing to be stable, it is not in this milestone.
- Not probe GI in the core scope. Stage 9 is a stretch, not a dependency.

## Open questions

- **Coarse cell size.** 1 m matches the block grid and makes occupancy binary until v0.0.10. 2 m makes
  occupancy fractional today at the cost of resolution in caves. Start at 1 m, measure the low tier.
- **Automaton versus BFS on the GPU.** Two fixed iterations per frame has flat cost; a frontier BFS
  converges faster but has a worst case. Try the automaton first.
- **Colour precision.** `RGBA8` is 4 MB and clips; `R11G11B10F` in a `u32` is the same footprint with
  more headroom and more ALU. Measure the clipping on a night scene with two lamp colours.
- **Do we still need scalar block light?** Once colour propagates, the 4-bit block channel is mainly a
  fallback and a gameplay signal. Keeping it costs little; removing it simplifies the vertex format.
- **Volume coverage.** `128 x 64 x 128` follows the player horizontally but not into deep caves or
  towers. Track the eye vertically, or accept the fallback outside it?
- **Occupancy AO versus GTAO.** They overlap in the 1-3 m range. Is the occupancy term worth keeping
  once GTAO exists, or is it only a propagation gate?
- **Probe spacing** (stretch). 2 m is the starting point. Is 1 m worth 4x the memory and rays?
- **Vertical probe coverage** (stretch). One cascade covers plus or minus 16 m.
- **Clipmap mips** (stretch). Skip-empty DDA versus CPU-generated mips.
- **Cascade count.** Two shadow cascades at `2048 x 2048` may beat three at `1536 x 1536`. Measure.
- **Tone curve.** AgX versus a tuned filmic curve. AgX is the safer default; it is also more code.
- **Where detail AO comes from** in v0.0.10: screen-space derivatives of the detail shell, a
  precomputed per-face AO texture, or a short in-cell ray. The occupancy field is designed to absorb
  the answer without a format change.

## References

**Reference engine (MishMash, "Micro Voxel Engine").** The project this page measures itself against.
C++, Metal, M1 Pro; 10-20 cm voxels; no source published.

- [Lighting post](https://reddit.com/r/GraphicsProgramming/comments/1udkyzj/i_built_a_microvoxel_engine/):
  coloured propagation, "Voxel occupancy occlusion", 0.3 ms, 3.2 GB.
- [Automaton detail](https://reddit.com/r/VoxelGameDev/comments/1uimbj6/adding_caves_to_my_micro_voxel_engine_devlog_3/oyj12qh/):
  one iteration per cell per frame, bounded around lights, ~0.5% of frame cost, torch reaches three
  coarse cells, intensity and colour propagation, occupancy weighting.
- [Early direct light](https://reddit.com/r/VoxelGameDev/comments/1u2jx1h/new_large_scale_voxel_engine_in_c_and_metal/or2ct7q/):
  single-cascade PCF shadow map, no vertex normals, distance-based shading, darkening baked into voxel
  data.
- [Occupancy reused for heat](https://reddit.com/r/VoxelGameDev/comments/1v2w4r2/adding_heat_simulation_and_survival_to_my_micro/oz0e0rt/):
  the same percentage-full field drives insulation.
- [Occupancy reused for water](https://reddit.com/r/VoxelGameDev/comments/1upohtk/simulating_procedural_ponds_in_my_micro_voxel/ow6dlv2/):
  1 m coarse cells, flow gated by cell occupancy.
- [Video series](https://www.youtube.com/@MishMash95): music and on-screen text only, no captions.

- Majercik, Guertin, Nowrouzezahrai, McGuire. *Dynamic Diffuse Global Illumination with Ray-Traced
  Irradiance Fields.* JCGT 8(2), 2019. <https://jcgt.org/published/0008/02/01/>
- NVIDIA. *RTXGI-DDGI* documentation: probe layout, blending, relocation, and leak rules.
  <https://github.com/NVIDIAGameWorks/RTXGI-DDGI>
- Hillaire. *A Scalable and Production Ready Sky and Atmosphere Rendering Technique.* EGSR 2020.
  <https://sebh.github.io/publications/egsr2020.pdf>
- Wronski. *Volumetric Fog: Unified Compute Shader Based Solution to Atmospheric Scattering.*
  SIGGRAPH 2014. The froxel-grid reference for later milestones.
- Schied et al. *Spatiotemporal Variance-Guided Filtering.* HPG 2017. The denoiser to revisit if
  per-pixel tracing replaces probes.
- Crassin et al. *Interactive Indirect Illumination Using Voxel Cone Tracing.* PG 2011, and NVIDIA
  VXGI. The rejected cone-tracing shape, kept here for the record.
- Godot Engine. *Introduction to global illumination.* SDFGI and VoxelGI tradeoffs and known leak
  behaviour. <https://docs.godotengine.org/en/stable/tutorials/3d/global_illumination/introduction_to_global_illumination.html>
- Gustafsson / Tuxedo Labs. Teardown rendering breakdowns: voxel DDA with an occlusion-only
  structure, two ambient rays per pixel, spiral blur. The per-pixel target for later.
  <https://acko.net/blog/teardown-frame-teardown>
- Sobotka. *AgX*; Khronos. *PBR Neutral Tone Mapper* (2024). The two tone curves under
  consideration.
