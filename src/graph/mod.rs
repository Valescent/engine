//! W2 - the declarative field graph: nodes, canonical form, compiler.
//!
//! `B3.1`, `B3.2`, `B3.5`, `B3.6`, `B3.7`, `B3.8`. A [`Graph`] is data: nodes pushed in
//! declaration order, each naming its inputs by the id `push` returned. [`Graph::compile`]
//! validates it once and lowers it to a flat [`Compiled`] program that is evaluated into
//! preallocated slots in topological order, ties broken by node id.
//!
//! Canonical form is structural, not positional: encoding walks from the output through
//! the nodes, so two graphs with the same topology hash equal even when their branches
//! were pushed in a different order. `Add` and `Mul` sort their two child encodings, so
//! operand order in those two kinds is not identity either. `f32` values are encoded as
//! bit patterns with `-0.0` normalised to `+0.0`; comments and whitespace do not exist in
//! the byte stream. [`Graph::hash`] is `sha256(canonical_bytes())`.
//!
//! `canonical_bytes` is total, including for a graph that cannot compile: a cycle emits
//! a marker instead of recursing forever. Only a graph that compiles has a meaningful
//! hash, which is the same rule the loader applies to the world description.

mod compile;
pub mod node;

pub use compile::Compiled;
pub use node::{Lattice, Node, NodeSpec, Params};

use crate::error::TerrainError;
use crate::hash::sha256;

/// Maximum number of nodes in a compiled graph.
pub const MAX_NODES: usize = 256;

/// Maximum number of nodes on one dependency path.
pub const MAX_DEPTH: usize = 32;

/// Version byte of the canonical graph encoding.
const CANONICAL_VERSION: u8 = 1;

/// Marker for a missing or out-of-range output.
const NO_OUTPUT: u8 = 0;

/// Marker for an input id that does not exist.
const UNKNOWN_NODE: u8 = 0xFE;

/// Marker for a back edge encountered while encoding.
const CYCLE: u8 = 0xFF;

/// A declarative field graph.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Graph {
    nodes: Vec<NodeSpec>,
    output: Option<u32>,
}

impl Graph {
    /// An empty graph with no output.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends one node and returns its id.
    ///
    /// The id is the declaration index, so an input must name an id below its own.
    pub fn push(&mut self, node: NodeSpec) -> u32 {
        let id = u32::try_from(self.nodes.len()).unwrap_or(u32::MAX);
        self.nodes.push(node);
        id
    }

    /// Declares which node is the graph's output.
    pub fn set_output(&mut self, id: u32) {
        self.output = Some(id);
    }

    /// Returns the declared output id, if any.
    #[must_use]
    pub fn output(&self) -> Option<u32> {
        self.output
    }

    /// Returns the node with `id`, if it exists.
    #[must_use]
    pub fn node(&self, id: u32) -> Option<&NodeSpec> {
        usize::try_from(id)
            .ok()
            .and_then(|index| self.nodes.get(index))
    }

    /// Number of declared nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the graph declares no nodes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Compiles the graph into a flat evaluator.
    ///
    /// # Errors
    ///
    /// Rejects cycles, unknown or forward inputs, a node whose lattice disagrees with
    /// its inputs, a chained warp, more than one warp, a missing output, a node-count
    /// or depth overrun, malformed parameters, and non-finite values.
    pub fn compile(&self) -> Result<Compiled, TerrainError> {
        compile::compile(self)
    }

    /// Canonical bytes: the input to [`Graph::hash`].
    ///
    /// The encoding starts with a version byte, then a presence byte for the output,
    /// then one recursive record per reachable node. Each record is kind, lattice,
    /// parameters, child count, and children.
    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = vec![CANONICAL_VERSION];
        let output = self
            .output
            .and_then(|id| usize::try_from(id).ok())
            .filter(|index| *index < self.nodes.len());
        let Some(output) = output else {
            out.push(NO_OUTPUT);
            return out;
        };
        out.push(1);
        let mut visiting = vec![false; self.nodes.len()];
        self.encode(output, &mut visiting, &mut out);
        out
    }

    /// The 32-byte structural hash of the graph.
    #[must_use]
    pub fn hash(&self) -> [u8; 32] {
        sha256(&self.canonical_bytes())
    }

    fn encode(&self, index: usize, visiting: &mut [bool], out: &mut Vec<u8>) {
        if visiting[index] {
            out.push(CYCLE);
            return;
        }
        visiting[index] = true;
        let spec = &self.nodes[index];
        out.push(kind_tag(spec.kind));
        out.push(lattice_tag(spec.lattice));
        encode_params(&spec.params, out);
        let count = u16::try_from(spec.inputs.len()).unwrap_or(u16::MAX);
        out.extend_from_slice(&count.to_le_bytes());

        let commutative = matches!(spec.kind, Node::Add | Node::Mul);
        if commutative {
            let mut children: Vec<Vec<u8>> = spec
                .inputs
                .iter()
                .map(|id| {
                    let mut bytes = Vec::new();
                    self.encode_input(*id, visiting, &mut bytes);
                    bytes
                })
                .collect();
            children.sort();
            for bytes in children {
                out.extend_from_slice(&bytes);
            }
        } else {
            for id in &spec.inputs {
                self.encode_input(*id, visiting, out);
            }
        }
        visiting[index] = false;
    }

    fn encode_input(&self, id: u32, visiting: &mut [bool], out: &mut Vec<u8>) {
        match usize::try_from(id) {
            Ok(index) if index < self.nodes.len() => self.encode(index, visiting, out),
            _ => out.push(UNKNOWN_NODE),
        }
    }

    pub(crate) fn nodes(&self) -> &[NodeSpec] {
        &self.nodes
    }
}

fn kind_tag(kind: Node) -> u8 {
    match kind {
        Node::Noise2d => 1,
        Node::Fbm => 2,
        Node::Ridged => 3,
        Node::Billow => 4,
        Node::Warp => 5,
        Node::Terrace => 6,
        Node::Curve => 7,
        Node::Remap => 8,
        Node::Add => 9,
        Node::Mul => 10,
        Node::Lerp => 11,
        Node::Clamp => 12,
        Node::Const => 13,
        Node::Input => 14,
    }
}

fn lattice_tag(lattice: Lattice) -> u8 {
    match lattice {
        Lattice::Macro => 0,
        Lattice::Block => 1,
    }
}

fn push_f32(out: &mut Vec<u8>, value: f32) {
    let bits = if value == 0.0 { 0 } else { value.to_bits() };
    out.extend_from_slice(&bits.to_le_bytes());
}

fn encode_params(params: &Params, out: &mut Vec<u8>) {
    match params {
        Params::None => {}
        Params::Const { value } => push_f32(out, *value),
        Params::Noise { seed, frequency } => {
            out.extend_from_slice(&seed.to_le_bytes());
            push_f32(out, *frequency);
        }
        Params::Fractal {
            seed,
            frequency,
            octaves,
            lacunarity,
            gain,
        } => {
            out.extend_from_slice(&seed.to_le_bytes());
            push_f32(out, *frequency);
            out.extend_from_slice(&octaves.to_le_bytes());
            push_f32(out, *lacunarity);
            push_f32(out, *gain);
        }
        Params::Warp {
            seed_a,
            seed_b,
            frequency,
            amplitude,
        } => {
            out.extend_from_slice(&seed_a.to_le_bytes());
            out.extend_from_slice(&seed_b.to_le_bytes());
            push_f32(out, *frequency);
            push_f32(out, *amplitude);
        }
        Params::Terrace { step, sharpness } => {
            push_f32(out, *step);
            push_f32(out, *sharpness);
        }
        Params::Curve(curve) => {
            out.push(u8::from(curve.is_linear()));
            let count = u16::try_from(curve.points().len()).unwrap_or(u16::MAX);
            out.extend_from_slice(&count.to_le_bytes());
            for (x, y) in curve.points() {
                push_f32(out, *x);
                push_f32(out, *y);
            }
        }
        Params::Remap { low, high } | Params::Clamp { low, high } => {
            push_f32(out, *low);
            push_f32(out, *high);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::Curve;
    use crate::error::ErrorKind;

    fn invalid_rule(error: &TerrainError) -> &'static str {
        if let ErrorKind::GraphInvalid { rule, .. } = &error.kind {
            rule
        } else {
            panic!("expected a graph error, got {error:?}");
        }
    }

    const BLOCK: Lattice = Lattice::Block;
    const MACRO: Lattice = Lattice::Macro;

    fn constant(graph: &mut Graph, value: f32) -> u32 {
        graph.push(NodeSpec::constant(BLOCK, value))
    }

    fn assert_rejects(graph: &Graph, expected: &str) {
        let error = graph
            .compile()
            .err()
            .unwrap_or_else(|| panic!("expected a {expected} rejection"));
        assert_eq!(invalid_rule(&error), expected);
    }

    #[test]
    fn compiler_rejects_bad_graphs() {
        // A cycle takes priority over the forward-input rule.
        let mut cycle = Graph::new();
        cycle.push(NodeSpec::add(BLOCK, 1, 1));
        let second = cycle.push(NodeSpec::add(BLOCK, 0, 0));
        cycle.set_output(second);
        assert_rejects(&cycle, "cycle");

        // An input id that does not exist.
        let mut unknown = Graph::new();
        let base = constant(&mut unknown, 0.5);
        let out = unknown.push(NodeSpec::add(BLOCK, base, 9));
        unknown.set_output(out);
        assert_rejects(&unknown, "unknown input");

        // A chained warp.
        let mut chained = Graph::new();
        let noise = chained.push(NodeSpec::noise2d(MACRO, 1, 0.01));
        let inner = chained.push(NodeSpec::warp(MACRO, 2, 3, 0.01, 8.0, noise));
        let outer = chained.push(NodeSpec::warp(MACRO, 4, 5, 0.01, 8.0, inner));
        chained.set_output(outer);
        assert_rejects(&chained, "warp chain");

        // Two independent warps are also rejected.
        let mut warps = Graph::new();
        let first_noise = warps.push(NodeSpec::noise2d(MACRO, 1, 0.01));
        let first_warp = warps.push(NodeSpec::warp(MACRO, 2, 3, 0.01, 8.0, first_noise));
        let second_noise = warps.push(NodeSpec::noise2d(MACRO, 4, 0.01));
        let second_warp = warps.push(NodeSpec::warp(MACRO, 5, 6, 0.01, 8.0, second_noise));
        let both = warps.push(NodeSpec::add(MACRO, first_warp, second_warp));
        warps.set_output(both);
        assert_rejects(&warps, "multiple warps");

        // A macro warp.
        let mut non_macro = Graph::new();
        let block_noise = non_macro.push(NodeSpec::noise2d(BLOCK, 1, 0.01));
        let block_warp = non_macro.push(NodeSpec::warp(BLOCK, 2, 3, 0.01, 8.0, block_noise));
        non_macro.set_output(block_warp);
        assert_rejects(&non_macro, "warp lattice");

        // A child on a different lattice from its parent.
        let mut lattice = Graph::new();
        let macro_base = lattice.push(NodeSpec::constant(MACRO, 0.5));
        let block_out = lattice.push(NodeSpec::add(BLOCK, macro_base, macro_base));
        lattice.set_output(block_out);
        assert_rejects(&lattice, "lattice");

        // One node longer than the depth cap.
        let mut deep = Graph::new();
        let mut previous = constant(&mut deep, 0.5);
        for _ in 0..MAX_DEPTH {
            previous = deep.push(NodeSpec::add(BLOCK, previous, previous));
        }
        deep.set_output(previous);
        assert_rejects(&deep, "depth cap");

        // One node over the node cap.
        let mut wide = Graph::new();
        for _ in 0..=MAX_NODES {
            constant(&mut wide, 0.5);
        }
        wide.set_output(0);
        assert_rejects(&wide, "node cap");

        // No declared output.
        let mut missing = Graph::new();
        constant(&mut missing, 0.5);
        assert_rejects(&missing, "missing output");

        // A forward input without a cycle.
        let mut forward = Graph::new();
        let out = forward.push(NodeSpec::add(BLOCK, 1, 1));
        constant(&mut forward, 0.5);
        forward.set_output(out);
        assert_rejects(&forward, "forward input");

        // Non-finite parameters and a mismatched parameter block.
        let mut non_finite = Graph::new();
        let nan = constant(&mut non_finite, f32::NAN);
        non_finite.set_output(nan);
        assert_rejects(&non_finite, "finite");

        let mut wrong_params = Graph::new();
        let mut spec = NodeSpec::constant(BLOCK, 0.5);
        spec.kind = Node::Add;
        let id = wrong_params.push(spec);
        wrong_params.set_output(id);
        assert_rejects(&wrong_params, "params");

        let mut wrong_arity = Graph::new();
        let mut spec = NodeSpec::constant(BLOCK, 0.5);
        spec.inputs = vec![0];
        let id = wrong_arity.push(spec);
        wrong_arity.set_output(id);
        assert_rejects(&wrong_arity, "arity");
    }

    #[test]
    fn graph_hash_ignores_declaration_order() {
        let mut first = Graph::new();
        let low = constant(&mut first, 0.25);
        let high = constant(&mut first, 0.75);
        let sum = first.push(NodeSpec::add(BLOCK, low, high));
        first.set_output(sum);

        let mut second = Graph::new();
        let high = constant(&mut second, 0.75);
        let low = constant(&mut second, 0.25);
        let sum = second.push(NodeSpec::add(BLOCK, high, low));
        second.set_output(sum);

        assert_eq!(first.hash(), second.hash());
        assert_eq!(first.canonical_bytes(), second.canonical_bytes());

        let mut changed = Graph::new();
        let low = constant(&mut changed, 0.5);
        let high = constant(&mut changed, 0.75);
        let sum = changed.push(NodeSpec::add(BLOCK, low, high));
        changed.set_output(sum);
        assert_ne!(first.hash(), changed.hash());

        let mut positive = Graph::new();
        let value = constant(&mut positive, 0.0);
        positive.set_output(value);
        let mut negative = Graph::new();
        let value = constant(&mut negative, -0.0);
        negative.set_output(value);
        assert_eq!(positive.hash(), negative.hash());
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn eval_order_is_topological() {
        let mut graph = Graph::new();
        let base = constant(&mut graph, 0.5);
        let left = graph.push(NodeSpec::add(BLOCK, base, base));
        let right = graph.push(NodeSpec::mul(BLOCK, base, base));
        let output = graph.push(NodeSpec::add(BLOCK, left, right));
        graph.set_output(output);

        let compiled = graph.compile().unwrap();
        assert_eq!(compiled.order(), &[0, 1, 2, 3]);
        assert_eq!(compiled.slot_count(), 4);
        assert_eq!(compiled.warp_halo_cells(), 0);
        assert_eq!(compiled.eval_2d(0.0, 0.0), 1.25);
        assert_eq!(compiled.eval_2d(99.0, -42.0), 1.25);
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn eval_is_deterministic_and_stays_in_range() {
        let mut graph = Graph::new();
        let noise = graph.push(NodeSpec::fbm(MACRO, 0x5EED, 1.0 / 256.0, 4, 2.0, 0.5));
        let curve = Curve::new(&[(0.0, 0.0), (0.5, 0.25), (1.0, 1.0)]).unwrap();
        let shaped = graph.push(NodeSpec::curve(MACRO, curve, noise));
        graph.set_output(shaped);

        let compiled = graph.compile().unwrap();
        for index in 0..128 {
            let x = index as f32 * 0.5;
            let value = compiled.eval_2d(x, -x);
            assert!((0.0..=1.0).contains(&value), "{value} at {x}");
            assert_eq!(value, compiled.eval_2d(x, -x));
        }
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn warp_samples_the_input_at_the_warped_position() {
        use crate::noise::{simplex2_2d, unit, warp_2d};

        let mut graph = Graph::new();
        let noise = graph.push(NodeSpec::noise2d(MACRO, 17, 1.0 / 32.0));
        let warp = graph.push(NodeSpec::warp(MACRO, 21, 22, 1.0 / 48.0, 8.0, noise));
        graph.set_output(warp);
        let compiled = graph.compile().unwrap();
        assert_eq!(compiled.warp_halo_cells(), 3);

        let frequency = 1.0 / 32.0;
        for index in 0..64 {
            let x = index as f32 * 3.0;
            let (offset_x, offset_z) = warp_2d(21, 22, x, 0.0, 1.0 / 48.0, 8.0);
            let expected = unit(simplex2_2d(
                17,
                (x + offset_x) * frequency,
                offset_z * frequency,
            ));
            assert_eq!(compiled.eval_2d(x, 0.0), expected, "at {x}");
        }
    }

    #[test]
    fn hash_of_an_uncompilable_graph_is_stable() {
        let mut graph = Graph::new();
        graph.push(NodeSpec::add(BLOCK, 1, 1));
        let second = graph.push(NodeSpec::add(BLOCK, 0, 0));
        graph.set_output(second);
        assert_eq!(graph.hash(), graph.hash());
        assert!(graph.canonical_bytes().contains(&CYCLE));
    }

    #[test]
    fn graph_hash_is_pinned() {
        let mut graph = Graph::new();
        let noise = graph.push(NodeSpec::fbm(MACRO, 0x5EED, 1.0 / 256.0, 4, 2.0, 0.5));
        let curve = Curve::new(&[(0.0, 0.0), (0.5, 0.25), (1.0, 1.0)]).unwrap();
        let shaped = graph.push(NodeSpec::curve(MACRO, curve, noise));
        graph.set_output(shaped);

        assert_eq!(hex(graph.hash()), GRAPH_HASH_PINNED);
    }

    const GRAPH_HASH_PINNED: &str =
        "0dcf02b468c68b756a988cfe2df5baa5c2a23e17a3ff430a8fb67ad892a90259";

    fn hex(digest: [u8; 32]) -> String {
        use std::fmt::Write as _;

        digest.iter().fold(String::new(), |mut out, byte| {
            write!(out, "{byte:02x}").unwrap();
            out
        })
    }

    #[test]
    fn nodes_are_reachable_by_id() {
        let mut graph = Graph::new();
        let id = constant(&mut graph, 0.5);
        assert_eq!(graph.len(), 1);
        assert!(!graph.is_empty());
        assert_eq!(graph.output(), None);
        assert!(graph.node(id).is_some());
        assert!(graph.node(7).is_none());
        graph.set_output(id);
        assert_eq!(graph.output(), Some(id));
        assert_eq!(graph.nodes().len(), 1);
    }
}
