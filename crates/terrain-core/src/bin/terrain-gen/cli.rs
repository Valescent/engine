use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Order {
    #[default]
    Forward,
    Reverse,
    Shuffled,
    Numeric(u64),
}

impl Order {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "forward" => Ok(Self::Forward),
            "reverse" => Ok(Self::Reverse),
            "shuffled" => Ok(Self::Shuffled),
            _ => value
                .parse::<u64>()
                .map(Self::Numeric)
                .map_err(|_| format!("invalid --order {value:?}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Options {
    pub(crate) json: bool,
    pub(crate) threads: usize,
    pub(crate) order: Order,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            json: false,
            threads: 1,
            order: Order::Forward,
        }
    }
}

/// Removes the global options from `args`, leaving subcommand arguments intact.
pub(crate) fn parse_globals(args: &[String]) -> Result<(Options, Vec<String>), String> {
    let mut options = Options::default();
    let mut rest = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => {
                options.json = true;
                index += 1;
            }
            "--threads" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "--threads needs a value".to_owned())?;
                let threads = value
                    .parse::<usize>()
                    .map_err(|_| format!("invalid --threads {value:?}"))?;
                if threads == 0 {
                    return Err("--threads must be at least 1".to_owned());
                }
                options.threads = threads;
                index += 2;
            }
            "--order" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "--order needs a value".to_owned())?;
                options.order = Order::parse(value)?;
                index += 2;
            }
            other => {
                rest.push(other.to_owned());
                index += 1;
            }
        }
    }
    Ok((options, rest))
}

/// A left-to-right cursor over subcommand arguments.
pub(crate) struct Cursor<'a> {
    args: &'a [String],
    index: usize,
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(args: &'a [String]) -> Self {
        Self { args, index: 0 }
    }

    pub(crate) fn arg(&mut self) -> Option<&'a str> {
        let value = self.args.get(self.index)?;
        self.index += 1;
        Some(value.as_str())
    }

    pub(crate) fn peek(&self) -> Option<&'a str> {
        self.args.get(self.index).map(String::as_str)
    }

    pub(crate) fn value(&mut self, name: &str) -> Result<&'a str, String> {
        self.arg().ok_or_else(|| format!("--{name} needs a value"))
    }
}

pub(crate) fn number<T>(value: &str, name: &str) -> Result<T, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse::<T>()
        .map_err(|error| format!("invalid --{name} {value:?}: {error}"))
}

/// Parses a `--key value` list into a map. Every token must be a flag.
pub(crate) fn parse_pairs(args: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut cursor = Cursor::new(args);
    let mut pairs = BTreeMap::new();
    while let Some(argument) = cursor.arg() {
        let Some(key) = argument.strip_prefix("--") else {
            return Err(format!("unexpected argument {argument:?}"));
        };
        let value = cursor
            .arg()
            .ok_or_else(|| format!("--{key} needs a value"))?;
        if pairs.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(format!("duplicate --{key}"));
        }
    }
    Ok(pairs)
}

pub(crate) fn parse_seed(value: &str) -> Result<u64, String> {
    let parsed = value
        .strip_prefix("0x")
        .map_or_else(|| value.parse::<u64>(), |hex| u64::from_str_radix(hex, 16));
    parsed.map_err(|error| format!("invalid seed {value:?}: {error}"))
}

// ---------------------------------------------------------------------------
// Deterministic work ordering
// ---------------------------------------------------------------------------

/// A fixed permutation of `0..len`. `Numeric(n)` seeds the same shuffle that
/// `Shuffled` seeds from `shuffled_key`; neither reads the clock or an RNG. For
/// `sweep`, `shuffled_key` is the first canonical seed, so the permutation is a
/// function of the seed list alone.
pub(crate) fn permutation(len: usize, order: Order, shuffled_key: u64) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..len).collect();
    match order {
        Order::Forward => {}
        Order::Reverse => indices.reverse(),
        Order::Shuffled => shuffle(&mut indices, shuffled_key),
        Order::Numeric(key) => shuffle(&mut indices, key),
    }
    indices
}

#[allow(clippy::cast_possible_truncation)]
fn shuffle(indices: &mut [usize], key: u64) {
    if indices.len() < 2 {
        return;
    }
    let mut state = terrain_core::hash::mix(key);
    for slot in (1..indices.len()).rev() {
        state = terrain_core::hash::mix(state ^ u64::try_from(slot).unwrap_or(0));
        // `slot + 1 <= len <= usize::MAX`, so the cast is in range on every
        // supported target.
        let offset = (state % u64::try_from(slot + 1).unwrap_or(u64::MAX)) as usize;
        indices.swap(slot, offset);
    }
}
