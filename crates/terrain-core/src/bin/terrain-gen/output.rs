use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use terrain_core::error::TerrainError;

pub(crate) const EXIT_OK: u8 = 0;
pub(crate) const EXIT_INVARIANT: u8 = 1;
pub(crate) const EXIT_USAGE: u8 = 2;
pub(crate) const EXIT_TERRAIN: u8 = 3;

// ---------------------------------------------------------------------------
// Failures and exit codes
// ---------------------------------------------------------------------------

pub(crate) enum CliFailure {
    Usage(String),
    Description {
        key: &'static str,
        rule: &'static str,
    },
    Terrain(Box<TerrainError>),
    /// The command ran and found the input inconsistent with this build: a
    /// header identity mismatch or an unsupported generator version. Distinct
    /// from `Usage` because no argument was malformed.
    Invariant(String),
}

impl From<String> for CliFailure {
    fn from(message: String) -> Self {
        Self::Usage(message)
    }
}

impl From<TerrainError> for CliFailure {
    fn from(error: TerrainError) -> Self {
        Self::Terrain(Box::new(error))
    }
}

pub(crate) fn usage_error(json: bool, message: &str) -> u8 {
    eprintln!("{message}");
    if json {
        println!("{{\"error\":{},\"kind\":\"usage\"}}", json_str(message));
    }
    EXIT_USAGE
}

pub(crate) fn report(failure: &CliFailure, json: bool, command: &str) -> u8 {
    let (code, kind, message) = match failure {
        CliFailure::Usage(message) => (EXIT_USAGE, "usage", message.clone()),
        CliFailure::Description { key, rule } => (
            EXIT_USAGE,
            "description",
            format!("description key {key:?} violates {rule:?}"),
        ),
        CliFailure::Terrain(error) => (EXIT_TERRAIN, "terrain", error.to_string()),
        CliFailure::Invariant(message) => (EXIT_INVARIANT, "invariant", message.clone()),
    };
    if json {
        println!(
            "{{\"subcommand\":{},\"error\":{},\"kind\":{}}}",
            json_str(command),
            json_str(&message),
            json_str(kind)
        );
    }
    eprintln!("{message}");
    code
}

// ---------------------------------------------------------------------------
// Stable output helpers
// ---------------------------------------------------------------------------

pub(crate) fn json_str(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if u32::from(control) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(control));
            }
            plain => out.push(plain),
        }
    }
    out.push('"');
    out
}

pub(crate) fn hex32(bytes: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

pub(crate) fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(crate) fn hex_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

// ---------------------------------------------------------------------------
// Minimal JSON reader
// ---------------------------------------------------------------------------
//
// The project has no JSON crate and this binary must not add one, so a report
// bundle is read with this small, total parser. It accepts standard JSON,
// rejects duplicate object keys and trailing bytes, and bounds nesting depth so
// malformed input is a typed usage error rather than a panic.

#[derive(Clone, Debug)]
pub(crate) enum Json {
    Null,
    Bool,
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

impl Json {
    fn parse(text: &str) -> Result<Self, String> {
        let mut parser = JsonParser {
            bytes: text.as_bytes(),
            at: 0,
        };
        parser.skip_space();
        let value = parser.value(0)?;
        parser.skip_space();
        if parser.at != parser.bytes.len() {
            return Err(format!("trailing data at byte {}", parser.at));
        }
        Ok(value)
    }

    pub(crate) fn as_object(&self) -> Option<&BTreeMap<String, Json>> {
        match self {
            Self::Object(map) => Some(map),
            _ => None,
        }
    }

    pub(crate) fn as_array(&self) -> Option<&[Json]> {
        match self {
            Self::Array(values) => Some(values),
            _ => None,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> JsonParser<'a> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.at += 1;
        Some(byte)
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), String> {
        match self.bump() {
            Some(found) if found == byte => Ok(()),
            Some(found) => Err(format!(
                "expected {:?} at byte {}, found {:?}",
                char::from(byte),
                self.at - 1,
                char::from(found)
            )),
            None => Err("unexpected end of input".to_owned()),
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > 32 {
            return Err("nesting is too deep".to_owned());
        }
        match self
            .peek()
            .ok_or_else(|| "unexpected end of input".to_owned())?
        {
            b'{' => self.object(depth),
            b'[' => self.array(depth),
            b'"' => self.string().map(Json::String),
            b't' => self.literal(b"true").map(|()| Json::Bool),
            b'f' => self.literal(b"false").map(|()| Json::Bool),
            b'n' => self.literal(b"null").map(|()| Json::Null),
            b'-' | b'0'..=b'9' => self.number().map(Json::Number),
            other => Err(format!("unexpected byte {other:#04x} at byte {}", self.at)),
        }
    }

    fn literal(&mut self, word: &[u8]) -> Result<(), String> {
        let end = self
            .at
            .checked_add(word.len())
            .ok_or_else(|| "input is too long".to_owned())?;
        if self.bytes.get(self.at..end) == Some(word) {
            self.at = end;
            Ok(())
        } else {
            Err(format!("invalid literal at byte {}", self.at))
        }
    }

    fn number(&mut self) -> Result<String, String> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        if !matches!(self.peek(), Some(b'0'..=b'9')) {
            return Err(format!("invalid number at byte {start}"));
        }
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.at += 1;
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(format!("invalid number at byte {start}"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.at += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(format!("invalid number at byte {start}"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.at += 1;
            }
        }
        let raw = self
            .bytes
            .get(start..self.at)
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .ok_or_else(|| format!("invalid number at byte {start}"))?;
        Ok(raw.to_owned())
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        loop {
            match self.bump() {
                Some(b'"') => break,
                Some(b'\\') => self.escape(&mut out)?,
                Some(byte) if byte < 0x20 => {
                    return Err(format!("control byte {byte:#04x} in a string"));
                }
                Some(byte) => out.push(byte),
                None => return Err("unterminated string".to_owned()),
            }
        }
        String::from_utf8(out).map_err(|_| "string is not UTF-8".to_owned())
    }

    fn escape(&mut self, out: &mut Vec<u8>) -> Result<(), String> {
        match self.bump() {
            Some(b'"') => out.push(b'"'),
            Some(b'\\') => out.push(b'\\'),
            Some(b'/') => out.push(b'/'),
            Some(b'b') => out.push(0x08),
            Some(b'f') => out.push(0x0c),
            Some(b'n') => out.push(b'\n'),
            Some(b'r') => out.push(b'\r'),
            Some(b't') => out.push(b'\t'),
            Some(b'u') => {
                let unit = self.hex4()?;
                let character = char::from_u32(u32::from(unit))
                    .ok_or_else(|| "invalid \\u escape".to_owned())?;
                let mut buffer = [0_u8; 4];
                out.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
            }
            Some(other) => return Err(format!("invalid escape {other:#04x}")),
            None => return Err("unterminated escape".to_owned()),
        }
        Ok(())
    }

    fn hex4(&mut self) -> Result<u16, String> {
        let raw = self
            .take(4)
            .ok_or_else(|| "unterminated \\u escape".to_owned())?;
        let text = std::str::from_utf8(raw).map_err(|_| "invalid \\u escape".to_owned())?;
        u16::from_str_radix(text, 16).map_err(|_| "invalid \\u escape".to_owned())
    }

    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(count)?;
        let slice = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(slice)
    }

    fn object(&mut self, depth: usize) -> Result<Json, String> {
        self.expect(b'{')?;
        let mut map = BTreeMap::new();
        self.skip_space();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Json::Object(map));
        }
        loop {
            self.skip_space();
            let key = self.string()?;
            self.skip_space();
            self.expect(b':')?;
            self.skip_space();
            let value = self.value(depth + 1)?;
            if map.insert(key.clone(), value).is_some() {
                return Err(format!("duplicate key {key:?}"));
            }
            self.skip_space();
            match self.bump() {
                Some(b',') => {}
                Some(b'}') => break,
                _ => return Err(format!("expected ',' or '}}' at byte {}", self.at)),
            }
        }
        Ok(Json::Object(map))
    }

    fn array(&mut self, depth: usize) -> Result<Json, String> {
        self.expect(b'[')?;
        let mut values = Vec::new();
        self.skip_space();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(Json::Array(values));
        }
        loop {
            self.skip_space();
            values.push(self.value(depth + 1)?);
            self.skip_space();
            match self.bump() {
                Some(b',') => {}
                Some(b']') => break,
                _ => return Err(format!("expected ',' or ']' at byte {}", self.at)),
            }
        }
        Ok(Json::Array(values))
    }
}

pub(crate) fn read_json(path: &Path) -> Result<Json, CliFailure> {
    let bytes = fs::read(path)
        .map_err(|error| CliFailure::Usage(format!("cannot read {}: {error}", path.display())))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| CliFailure::Usage(format!("{} is not UTF-8", path.display())))?;
    Json::parse(text).map_err(|message| CliFailure::Usage(format!("{}: {message}", path.display())))
}

pub(crate) fn expect_object<'a>(
    json: &'a Json,
    file: &str,
) -> Result<&'a BTreeMap<String, Json>, CliFailure> {
    json.as_object()
        .ok_or_else(|| CliFailure::Usage(format!("{file}: top level must be an object")))
}

pub(crate) fn string_field(
    object: &BTreeMap<String, Json>,
    key: &str,
    file: &str,
) -> Result<String, CliFailure> {
    let value = object
        .get(key)
        .ok_or_else(|| CliFailure::Usage(format!("{file}: missing {key:?}")))?;
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| CliFailure::Usage(format!("{file}: {key:?} must be a string")))
}

pub(crate) fn integer_field<T>(
    object: &BTreeMap<String, Json>,
    key: &str,
    file: &str,
) -> Result<T, CliFailure>
where
    T: std::str::FromStr,
{
    let value = object
        .get(key)
        .ok_or_else(|| CliFailure::Usage(format!("{file}: missing {key:?}")))?;
    let Json::Number(raw) = value else {
        return Err(CliFailure::Usage(format!(
            "{file}: {key:?} must be an integer"
        )));
    };
    raw.parse::<T>()
        .map_err(|_| CliFailure::Usage(format!("{file}: invalid {key:?} {raw:?}")))
}
