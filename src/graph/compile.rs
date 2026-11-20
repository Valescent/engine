//! The one-time graph compiler and the flat evaluator it produces.
//!
//! `B3.1`, `B3.2`, `B3.6`, `B3.7`. Compilation validates the graph, orders nodes
//! topologically with ties broken by node id, and lowers every node to one [`Op`] with
//! slot indices instead of node ids. Evaluation is a single pass over that program into
//! preallocated slots; there is no per-sample interpretation and no allocation.
//!
//! Output is always an `f32` because every node evaluates to `f32`; a non-`f32` output
//! is unrepresentable in this vocabulary rather than rejected at run time.
//!
//! A warp is the one node that needs a second coordinate pass: it evaluates its input's
//! dependency closure at the warped position into a separate scratch stack. Chained
//! warps and more than one warp are rejected, so the closure can never contain another
//! warp.

use std::collections::BTreeSet;

use crate::curve::Curve;
use crate::error::{ErrorKind, Stage, TerrainError};
use crate::noise::{billow_2d, fbm_2d, ridged_2d, simplex2_2d, unit, warp_2d, warp_halo_cells};

use super::Graph;
use super::MAX_DEPTH;
use super::MAX_NODES;
use super::node::{Lattice, Node, NodeSpec};

/// One lowered operation. Operands are slot indices, not node ids.
#[derive(Clone, Debug)]
enum Op {
    Noise2d {
        seed: u32,
        frequency: f32,
    },
    Fbm {
        seed: u32,
        frequency: f32,
        octaves: u32,
        lacunarity: f32,
        gain: f32,
    },
    Ridged {
        seed: u32,
        frequency: f32,
        octaves: u32,
        lacunarity: f32,
        gain: f32,
    },
    Billow {
        seed: u32,
        frequency: f32,
        octaves: u32,
        lacunarity: f32,
        gain: f32,
    },
    Warp {
        input: usize,
    },
    Terrace {
        input: usize,
        step: f32,
        sharpness: f32,
    },
    Curve {
        input: usize,
        curve: Curve,
    },
    Remap {
        input: usize,
        low: f32,
        high: f32,
    },
    Add {
        left: usize,
        right: usize,
    },
    Mul {
        left: usize,
        right: usize,
    },
    Lerp {
        a: usize,
        b: usize,
        t: usize,
    },
    Clamp {
        input: usize,
        low: f32,
        high: f32,
    },
    Const {
        value: f32,
    },
    Input,
}

impl Op {
    fn eval(&self, slots: &[f32], x: f32, z: f32) -> f32 {
        match self {
            Self::Noise2d { seed, frequency } => {
                unit(simplex2_2d(*seed, x * *frequency, z * *frequency))
            }
            Self::Fbm {
                seed,
                frequency,
                octaves,
                lacunarity,
                gain,
            } => unit(fbm_2d(
                *seed,
                x,
                z,
                *frequency,
                *octaves,
                *lacunarity,
                *gain,
            )),
            Self::Ridged {
                seed,
                frequency,
                octaves,
                lacunarity,
                gain,
            } => unit(ridged_2d(
                *seed,
                x,
                z,
                *frequency,
                *octaves,
                *lacunarity,
                *gain,
            )),
            Self::Billow {
                seed,
                frequency,
                octaves,
                lacunarity,
                gain,
            } => unit(billow_2d(
                *seed,
                x,
                z,
                *frequency,
                *octaves,
                *lacunarity,
                *gain,
            )),
            // Evaluated by `Compiled::eval_2d` before this arm can run; returning the
            // input value keeps the fallback total if that ever changes.
            Self::Warp { input } => slots[*input],
            Self::Terrace {
                input,
                step,
                sharpness,
            } => terrace(slots[*input], *step, *sharpness),
            Self::Curve { input, curve } => curve.sample(slots[*input]),
            Self::Remap { input, low, high } => {
                let value = slots[*input].clamp(0.0, 1.0);
                *low + (*high - *low) * value
            }
            Self::Add { left, right } => slots[*left] + slots[*right],
            Self::Mul { left, right } => slots[*left] * slots[*right],
            Self::Lerp { a, b, t } => {
                let from = slots[*a];
                from + (slots[*b] - from) * slots[*t]
            }
            Self::Clamp { input, low, high } => slots[*input].clamp(*low, *high),
            Self::Const { value } => *value,
            Self::Input => x,
        }
    }
}

/// The warp's second pass: which slots to recompute, and where.
#[derive(Clone, Debug)]
struct WarpPlan {
    slot: usize,
    input: usize,
    members: Vec<bool>,
    seed_a: u32,
    seed_b: u32,
    frequency: f32,
    amplitude: f32,
}

/// A compiled, flat, topologically ordered field program.
#[derive(Clone, Debug)]
pub struct Compiled {
    ops: Vec<Op>,
    order: Vec<u32>,
    output: usize,
    warp: Option<WarpPlan>,
}

impl Compiled {
    /// Evaluates the graph's output at one world position.
    ///
    /// `x` and `z` are block coordinates. Every slot is written once per pass, in
    /// topological order. The result is a pure function of the compiled program and the
    /// coordinates.
    pub fn eval_2d(&self, x: f32, z: f32) -> f32 {
        let mut slots = [0.0_f32; MAX_NODES];
        for (index, op) in self.ops.iter().enumerate() {
            if let Some(plan) = &self.warp
                && index == plan.slot
            {
                slots[index] = self.eval_warp(plan, x, z);
                continue;
            }
            slots[index] = op.eval(&slots, x, z);
        }
        slots[self.output]
    }

    fn eval_warp(&self, plan: &WarpPlan, x: f32, z: f32) -> f32 {
        let (offset_x, offset_z) = warp_2d(
            plan.seed_a,
            plan.seed_b,
            x,
            z,
            plan.frequency,
            plan.amplitude,
        );
        let warped_x = x + offset_x;
        let warped_z = z + offset_z;
        let mut scratch = [0.0_f32; MAX_NODES];
        for (index, op) in self.ops.iter().enumerate().take(plan.slot) {
            if plan.members[index] {
                scratch[index] = op.eval(&scratch, warped_x, warped_z);
            }
        }
        scratch[plan.input]
    }

    /// Node ids in evaluation order: slot `i` was produced by `order()[i]`.
    #[must_use]
    pub fn order(&self) -> &[u32] {
        &self.order
    }

    /// Number of slots the program uses.
    #[must_use]
    pub fn slot_count(&self) -> usize {
        self.ops.len()
    }

    /// Extra tile halo cells the graph's warp forces, or `0` when it has no warp.
    ///
    /// `B2.1`: the halo is `ceil(amplitude / MACRO_CELL) + 1`, taken over the graph.
    #[must_use]
    pub fn warp_halo_cells(&self) -> i32 {
        self.warp
            .as_ref()
            .map_or(0, |plan| warp_halo_cells(plan.amplitude))
    }
}

fn invalid(node: usize, rule: &'static str) -> TerrainError {
    TerrainError::new(
        Stage::Graph,
        ErrorKind::GraphInvalid {
            node: u32::try_from(node).unwrap_or(u32::MAX),
            rule,
        },
    )
}

/// Compiles `graph`, or explains why it cannot be evaluated deterministically.
pub(crate) fn compile(graph: &Graph) -> Result<Compiled, TerrainError> {
    let nodes = graph.nodes();
    if nodes.len() > MAX_NODES {
        return Err(invalid(0, "node cap"));
    }
    let output = graph.output().ok_or_else(|| invalid(0, "missing output"))?;
    let output = usize::try_from(output).map_err(|_| invalid(0, "missing output"))?;
    if output >= nodes.len() {
        return Err(invalid(output, "missing output"));
    }

    check_structure(nodes)?;
    check_cycle(nodes)?;
    check_warp(nodes)?;
    check_depth(nodes)?;

    let order = topological_order(nodes);
    if order.len() != nodes.len() {
        return Err(invalid(0, "cycle"));
    }
    let mut slot_of = vec![usize::MAX; nodes.len()];
    for (slot, index) in order.iter().enumerate() {
        slot_of[*index] = slot;
    }

    let mut ops = Vec::with_capacity(nodes.len());
    for index in &order {
        ops.push(build_op(&nodes[*index], &slot_of));
    }
    let warp = warp_plan(nodes, &slot_of);
    Ok(Compiled {
        ops,
        order: order
            .iter()
            .map(|index| u32::try_from(*index).unwrap_or(u32::MAX))
            .collect(),
        output: slot_of[output],
        warp,
    })
}

/// Checks arity, parameter variants, input ids, lattice agreement, and finiteness.
fn check_structure(nodes: &[NodeSpec]) -> Result<(), TerrainError> {
    for (index, node) in nodes.iter().enumerate() {
        if !node.params_match() {
            return Err(invalid(index, "params"));
        }
        if node.inputs.len() != node.arity() {
            return Err(invalid(index, "arity"));
        }
        if node.floats().iter().any(|value| !value.is_finite()) {
            return Err(invalid(index, "finite"));
        }
        for input in &node.inputs {
            let child = usize::try_from(*input).map_err(|_| invalid(index, "unknown input"))?;
            if child >= nodes.len() {
                return Err(invalid(index, "unknown input"));
            }
            if nodes[child].lattice != node.lattice {
                return Err(invalid(index, "lattice"));
            }
        }
    }
    Ok(())
}

/// Rejects a self-loop or back edge before the forward-input rule can mask it.
fn check_cycle(nodes: &[NodeSpec]) -> Result<(), TerrainError> {
    let mut state = vec![0_u8; nodes.len()];
    for start in 0..nodes.len() {
        visit(start, nodes, &mut state)?;
    }
    Ok(())
}

fn visit(index: usize, nodes: &[NodeSpec], state: &mut [u8]) -> Result<(), TerrainError> {
    match state[index] {
        1 => return Err(invalid(index, "cycle")),
        2 => return Ok(()),
        _ => {}
    }
    state[index] = 1;
    for input in &nodes[index].inputs {
        let Ok(child) = usize::try_from(*input) else {
            continue;
        };
        if child < nodes.len() {
            visit(child, nodes, state)?;
        }
    }
    state[index] = 2;
    Ok(())
}

/// Rejects a chained warp, a non-macro warp, and more than one warp.
fn check_warp(nodes: &[NodeSpec]) -> Result<(), TerrainError> {
    let warps: Vec<usize> = nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| (node.kind == Node::Warp).then_some(index))
        .collect();
    if warps.is_empty() {
        return Ok(());
    }
    for &warp in &warps {
        if nodes[warp].lattice != Lattice::Macro {
            return Err(invalid(warp, "warp lattice"));
        }
    }
    for &warp in &warps {
        let members = closure(nodes, nodes[warp].inputs[0]);
        if members
            .iter()
            .enumerate()
            .any(|(index, member)| *member && nodes[index].kind == Node::Warp)
        {
            return Err(invalid(warp, "warp chain"));
        }
    }
    if warps.len() > 1 {
        return Err(invalid(warps[1], "multiple warps"));
    }
    Ok(())
}

fn closure(nodes: &[NodeSpec], root: u32) -> Vec<bool> {
    let mut members = vec![false; nodes.len()];
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        let Ok(index) = usize::try_from(id) else {
            continue;
        };
        if index >= nodes.len() || members[index] {
            continue;
        }
        members[index] = true;
        stack.extend(nodes[index].inputs.iter().copied());
    }
    members
}

/// Rejects declaration orders that are not topological and chains that are too long.
fn check_depth(nodes: &[NodeSpec]) -> Result<(), TerrainError> {
    let mut depth = vec![1_usize; nodes.len()];
    for (index, node) in nodes.iter().enumerate() {
        for input in &node.inputs {
            let child = usize::try_from(*input).unwrap_or(usize::MAX);
            if child >= index {
                return Err(invalid(index, "forward input"));
            }
            depth[index] = depth[index].max(depth[child] + 1);
        }
        if depth[index] > MAX_DEPTH {
            return Err(invalid(index, "depth cap"));
        }
    }
    Ok(())
}

fn topological_order(nodes: &[NodeSpec]) -> Vec<usize> {
    let mut indegree = vec![0_usize; nodes.len()];
    let mut consumers = vec![Vec::new(); nodes.len()];
    for (index, node) in nodes.iter().enumerate() {
        for input in &node.inputs {
            let Ok(child) = usize::try_from(*input) else {
                continue;
            };
            if child < nodes.len() {
                indegree[index] += 1;
                consumers[child].push(index);
            }
        }
    }
    let mut ready: BTreeSet<usize> = indegree
        .iter()
        .enumerate()
        .filter_map(|(index, degree)| (*degree == 0).then_some(index))
        .collect();
    let mut order = Vec::with_capacity(nodes.len());
    while let Some(index) = ready.pop_first() {
        order.push(index);
        for consumer in &consumers[index] {
            indegree[*consumer] -= 1;
            if indegree[*consumer] == 0 {
                ready.insert(*consumer);
            }
        }
    }
    order
}

fn build_op(node: &NodeSpec, slot_of: &[usize]) -> Op {
    let input = |position: usize| -> usize {
        let id = node.inputs[position];
        let index = usize::try_from(id).unwrap_or(0);
        slot_of.get(index).copied().unwrap_or(0)
    };
    match &node.params {
        super::node::Params::Noise { seed, frequency } => Op::Noise2d {
            seed: *seed,
            frequency: *frequency,
        },
        super::node::Params::Fractal {
            seed,
            frequency,
            octaves,
            lacunarity,
            gain,
        } => match node.kind {
            Node::Ridged => Op::Ridged {
                seed: *seed,
                frequency: *frequency,
                octaves: *octaves,
                lacunarity: *lacunarity,
                gain: *gain,
            },
            Node::Billow => Op::Billow {
                seed: *seed,
                frequency: *frequency,
                octaves: *octaves,
                lacunarity: *lacunarity,
                gain: *gain,
            },
            _ => Op::Fbm {
                seed: *seed,
                frequency: *frequency,
                octaves: *octaves,
                lacunarity: *lacunarity,
                gain: *gain,
            },
        },
        super::node::Params::Warp { .. } => Op::Warp { input: input(0) },
        super::node::Params::Terrace { step, sharpness } => Op::Terrace {
            input: input(0),
            step: *step,
            sharpness: *sharpness,
        },
        super::node::Params::Curve(curve) => Op::Curve {
            input: input(0),
            curve: curve.clone(),
        },
        super::node::Params::Remap { low, high } => Op::Remap {
            input: input(0),
            low: *low,
            high: *high,
        },
        super::node::Params::Clamp { low, high } => Op::Clamp {
            input: input(0),
            low: *low,
            high: *high,
        },
        super::node::Params::Const { value } => Op::Const { value: *value },
        super::node::Params::None => match node.kind {
            Node::Add => Op::Add {
                left: input(0),
                right: input(1),
            },
            Node::Mul => Op::Mul {
                left: input(0),
                right: input(1),
            },
            Node::Lerp => Op::Lerp {
                a: input(0),
                b: input(1),
                t: input(2),
            },
            _ => Op::Input,
        },
    }
}

fn warp_plan(nodes: &[NodeSpec], slot_of: &[usize]) -> Option<WarpPlan> {
    let index = nodes.iter().position(|node| node.kind == Node::Warp)?;
    let node = &nodes[index];
    let super::node::Params::Warp {
        seed_a,
        seed_b,
        frequency,
        amplitude,
    } = &node.params
    else {
        return None;
    };
    let members = closure(nodes, node.inputs[0]);
    // Reorder the membership mask into slot order.
    let mut slots = vec![false; nodes.len()];
    for (declared, member) in members.into_iter().enumerate() {
        if member {
            slots[slot_of[declared]] = true;
        }
    }
    let input = usize::try_from(node.inputs[0]).unwrap_or(0);
    Some(WarpPlan {
        slot: slot_of[index],
        input: slot_of.get(input).copied().unwrap_or(0),
        members: slots,
        seed_a: *seed_a,
        seed_b: *seed_b,
        frequency: *frequency,
        amplitude: *amplitude,
    })
}

/// One terrace step: floor to the nearest tread, then blend the riser between linear
/// and an S-curve. `sharpness` is clamped to `[0, 1]`.
fn terrace(value: f32, step: f32, sharpness: f32) -> f32 {
    if step <= 0.0 {
        return value;
    }
    let scaled = value / step;
    let base = scaled.floor();
    let fraction = scaled - base;
    let smooth = fraction * fraction * (3.0 - 2.0 * fraction);
    let shaped = fraction + (smooth - fraction) * sharpness.clamp(0.0, 1.0);
    (base + shaped) * step
}
