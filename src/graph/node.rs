//! The closed node vocabulary and the data a node carries.
//!
//! `B3.1`, `B3.5`, `B3.7`. A node is plain data: a kind, a declared lattice, its input
//! ids, and one parameter block whose variant is fixed by the kind. Adding a node kind
//! is a code change and a generator version bump; editing graph data is not.

use crate::curve::Curve;

/// The sampling lattice a node declares.
///
/// `B3.7`: macro nodes evaluate on the 4-block lattice; block nodes evaluate at block
/// resolution. The compiler rejects a node whose children have a different lattice.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Lattice {
    /// The global 4-block macro lattice, sampled at `4k + 0.5`.
    Macro,
    /// Per-block sampling at `block + 0.5`.
    Block,
}

/// The closed set of node kinds.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Node {
    /// One `OpenSimplex2` 2D sample.
    Noise2d,
    /// Fractal Brownian motion.
    Fbm,
    /// Ridged fractal.
    Ridged,
    /// Billow fractal.
    Billow,
    /// A 2D domain warp of one input.
    Warp,
    /// A terrace transform of one input.
    Terrace,
    /// A monotone curve applied to one input.
    Curve,
    /// A linear remap of one input from `[0, 1]` to `[low, high]`.
    Remap,
    /// Adds two inputs.
    Add,
    /// Multiplies two inputs.
    Mul,
    /// Interpolates `a` toward `b` by `t`.
    Lerp,
    /// Clamps one input to `[low, high]`.
    Clamp,
    /// A constant value.
    Const,
    /// The sample position input: yields the sample's `x` coordinate in blocks.
    Input,
}

/// The parameter block of a node. The variant must match [`NodeSpec::kind`].
#[derive(Clone, Debug, PartialEq)]
pub enum Params {
    /// No parameters.
    None,
    /// A constant value.
    Const {
        /// The constant itself.
        value: f32,
    },
    /// One noise tap.
    Noise {
        /// Stream seed.
        seed: u32,
        /// Cycles per block.
        frequency: f32,
    },
    /// A fractal source.
    Fractal {
        /// Stream seed; octave `i` uses `seed + i`.
        seed: u32,
        /// Cycles per block for the first octave.
        frequency: f32,
        /// Number of octaves.
        octaves: u32,
        /// Frequency multiplier per octave.
        lacunarity: f32,
        /// Amplitude multiplier per octave.
        gain: f32,
    },
    /// A 2D domain warp.
    Warp {
        /// Seed of the `x` offset tap.
        seed_a: u32,
        /// Seed of the `z` offset tap.
        seed_b: u32,
        /// Cycles per block.
        frequency: f32,
        /// Maximum offset in blocks.
        amplitude: f32,
    },
    /// A terrace transform.
    Terrace {
        /// Distance between treads, in the input's units.
        step: f32,
        /// Riser shape in `[0, 1]`; `0` is linear, `1` is a smooth S-riser.
        sharpness: f32,
    },
    /// A curve transform.
    Curve(Curve),
    /// A linear remap of `[0, 1]` to `[low, high]`.
    Remap {
        /// Output at input `0`.
        low: f32,
        /// Output at input `1`.
        high: f32,
    },
    /// A clamp.
    Clamp {
        /// Lower bound.
        low: f32,
        /// Upper bound.
        high: f32,
    },
}

/// One declarative node.
#[derive(Clone, Debug, PartialEq)]
pub struct NodeSpec {
    /// Node kind.
    pub kind: Node,
    /// Declared sampling lattice.
    pub lattice: Lattice,
    /// Input node ids, in evaluation order.
    pub inputs: Vec<u32>,
    /// Parameter block; the variant must match `kind`.
    pub params: Params,
}

impl NodeSpec {
    /// Number of inputs the kind requires.
    #[must_use]
    pub fn arity(&self) -> usize {
        match self.kind {
            Node::Noise2d | Node::Fbm | Node::Ridged | Node::Billow | Node::Const | Node::Input => {
                0
            }
            Node::Warp | Node::Terrace | Node::Curve | Node::Remap | Node::Clamp => 1,
            Node::Add | Node::Mul => 2,
            Node::Lerp => 3,
        }
    }

    /// Whether the parameter block matches the node kind.
    #[must_use]
    pub fn params_match(&self) -> bool {
        matches!(
            (self.kind, &self.params),
            (Node::Noise2d, Params::Noise { .. })
                | (
                    Node::Fbm | Node::Ridged | Node::Billow,
                    Params::Fractal { .. }
                )
                | (Node::Warp, Params::Warp { .. })
                | (Node::Terrace, Params::Terrace { .. })
                | (Node::Curve, Params::Curve(_))
                | (Node::Remap, Params::Remap { .. })
                | (Node::Clamp, Params::Clamp { .. })
                | (Node::Const, Params::Const { .. })
                | (
                    Node::Add | Node::Mul | Node::Lerp | Node::Input,
                    Params::None
                )
        )
    }

    /// Every `f32` in the parameter block, for finiteness checks.
    #[must_use]
    pub fn floats(&self) -> Vec<f32> {
        match &self.params {
            Params::None => Vec::new(),
            Params::Const { value } => vec![*value],
            Params::Noise { frequency, .. } => vec![*frequency],
            Params::Fractal {
                frequency,
                lacunarity,
                gain,
                ..
            } => vec![*frequency, *lacunarity, *gain],
            Params::Warp {
                frequency,
                amplitude,
                ..
            } => vec![*frequency, *amplitude],
            Params::Terrace { step, sharpness } => vec![*step, *sharpness],
            Params::Curve(curve) => curve
                .points()
                .iter()
                .flat_map(|point| [point.0, point.1])
                .collect(),
            Params::Remap { low, high } | Params::Clamp { low, high } => vec![*low, *high],
        }
    }

    /// One `OpenSimplex2` source.
    #[must_use]
    pub fn noise2d(lattice: Lattice, seed: u32, frequency: f32) -> Self {
        Self {
            kind: Node::Noise2d,
            lattice,
            inputs: Vec::new(),
            params: Params::Noise { seed, frequency },
        }
    }

    /// One fractal source of the given kind.
    #[must_use]
    pub fn fractal(
        kind: Node,
        lattice: Lattice,
        seed: u32,
        frequency: f32,
        octaves: u32,
        lacunarity: f32,
        gain: f32,
    ) -> Self {
        debug_assert!(matches!(kind, Node::Fbm | Node::Ridged | Node::Billow));
        Self {
            kind,
            lattice,
            inputs: Vec::new(),
            params: Params::Fractal {
                seed,
                frequency,
                octaves,
                lacunarity,
                gain,
            },
        }
    }

    /// Fractal Brownian motion.
    #[must_use]
    pub fn fbm(
        lattice: Lattice,
        seed: u32,
        frequency: f32,
        octaves: u32,
        lacunarity: f32,
        gain: f32,
    ) -> Self {
        Self::fractal(
            Node::Fbm,
            lattice,
            seed,
            frequency,
            octaves,
            lacunarity,
            gain,
        )
    }

    /// A ridged fractal.
    #[must_use]
    pub fn ridged(
        lattice: Lattice,
        seed: u32,
        frequency: f32,
        octaves: u32,
        lacunarity: f32,
        gain: f32,
    ) -> Self {
        Self::fractal(
            Node::Ridged,
            lattice,
            seed,
            frequency,
            octaves,
            lacunarity,
            gain,
        )
    }

    /// A billow fractal.
    #[must_use]
    pub fn billow(
        lattice: Lattice,
        seed: u32,
        frequency: f32,
        octaves: u32,
        lacunarity: f32,
        gain: f32,
    ) -> Self {
        Self::fractal(
            Node::Billow,
            lattice,
            seed,
            frequency,
            octaves,
            lacunarity,
            gain,
        )
    }

    /// A 2D domain warp of `input`.
    #[must_use]
    pub fn warp(
        lattice: Lattice,
        seed_a: u32,
        seed_b: u32,
        frequency: f32,
        amplitude: f32,
        input: u32,
    ) -> Self {
        Self {
            kind: Node::Warp,
            lattice,
            inputs: vec![input],
            params: Params::Warp {
                seed_a,
                seed_b,
                frequency,
                amplitude,
            },
        }
    }

    /// A terrace transform of `input`.
    #[must_use]
    pub fn terrace(lattice: Lattice, step: f32, sharpness: f32, input: u32) -> Self {
        Self {
            kind: Node::Terrace,
            lattice,
            inputs: vec![input],
            params: Params::Terrace { step, sharpness },
        }
    }

    /// A curve transform of `input`.
    #[must_use]
    pub fn curve(lattice: Lattice, curve: Curve, input: u32) -> Self {
        Self {
            kind: Node::Curve,
            lattice,
            inputs: vec![input],
            params: Params::Curve(curve),
        }
    }

    /// A linear remap of `input` from `[0, 1]` to `[low, high]`.
    #[must_use]
    pub fn remap(lattice: Lattice, low: f32, high: f32, input: u32) -> Self {
        Self {
            kind: Node::Remap,
            lattice,
            inputs: vec![input],
            params: Params::Remap { low, high },
        }
    }

    /// Adds `left` and `right`.
    #[must_use]
    pub fn add(lattice: Lattice, left: u32, right: u32) -> Self {
        Self {
            kind: Node::Add,
            lattice,
            inputs: vec![left, right],
            params: Params::None,
        }
    }

    /// Multiplies `left` and `right`.
    #[must_use]
    pub fn mul(lattice: Lattice, left: u32, right: u32) -> Self {
        Self {
            kind: Node::Mul,
            lattice,
            inputs: vec![left, right],
            params: Params::None,
        }
    }

    /// Interpolates `a` toward `b` by `t`.
    #[must_use]
    pub fn lerp(lattice: Lattice, a: u32, b: u32, t: u32) -> Self {
        Self {
            kind: Node::Lerp,
            lattice,
            inputs: vec![a, b, t],
            params: Params::None,
        }
    }

    /// Clamps `input` to `[low, high]`.
    #[must_use]
    pub fn clamp(lattice: Lattice, low: f32, high: f32, input: u32) -> Self {
        Self {
            kind: Node::Clamp,
            lattice,
            inputs: vec![input],
            params: Params::Clamp { low, high },
        }
    }

    /// A constant value.
    #[must_use]
    pub fn constant(lattice: Lattice, value: f32) -> Self {
        Self {
            kind: Node::Const,
            lattice,
            inputs: Vec::new(),
            params: Params::Const { value },
        }
    }

    /// The sample position input.
    ///
    /// A scalar graph evaluates one value, so this yields the sample's `x` coordinate in
    /// blocks. Noise nodes receive both coordinates directly.
    #[must_use]
    pub fn input(lattice: Lattice) -> Self {
        Self {
            kind: Node::Input,
            lattice,
            inputs: Vec::new(),
            params: Params::None,
        }
    }
}
