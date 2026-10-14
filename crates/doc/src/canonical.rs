//! Canonical JSON writer (§4.3).
//!
//! Serialization runs through `serde_json` with a custom [`Formatter`] that,
//! instead of writing bytes as they come, records the value as a small tree
//! of already-formatted scalars. The tree is then laid out:
//!
//! - 2-space indentation, LF line endings, trailing newline;
//! - the root is expanded; any other container goes on one line when it nests at most one level of
//!   containers (e.g. `[{ "type": "solid", "color": "#fff" }]`), does not
//!   carry an `"id"` (nodes, pages, styles and components are always
//!   expanded), and its line fits in 100 columns;
//! - in an expanded object, a run of scalar geometry keys
//!   (`x`, `y`, `width`, `height`, `rotation`) shares one line;
//! - `f32` values are rounded to 2 decimals and written in shortest form
//!   (`16`, `0.5`, never `-0`). Other floats (only found in unknown keys,
//!   kept verbatim) are written in shortest form without rounding.
//!
//! Key order comes from struct field order and `BTreeMap`s, so the writer
//! itself never reorders anything.

use std::io;

use serde::Serialize;
use serde_json::ser::{CharEscape, CompactFormatter, Formatter};

/// Maximum rendered line length for one-line containers.
pub const MAX_LINE: usize = 100;
const INDENT: &str = "  ";
const GEOMETRY_KEYS: [&str; 5] = ["\"x\"", "\"y\"", "\"width\"", "\"height\"", "\"rotation\""];

/// Serialize `value` in canonical form, with a trailing newline.
pub fn to_canonical_string<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<String> {
    let mut rec = Recorder::default();
    {
        let mut ser = serde_json::Serializer::with_formatter(io::sink(), &mut rec);
        value.serialize(&mut ser)?;
    }
    let tree = rec.finish();
    let mut out = String::new();
    render(&tree, 0, 0, &mut out);
    out.push('\n');
    Ok(out)
}

/// Round to 2 decimals and write in shortest form.
pub fn format_f32(v: f32) -> String {
    let r = (v as f64 * 100.0).round() / 100.0;
    format_f64(r)
}

/// Shortest round-trip form; integral values without `.0`; `-0` as `0`.
pub fn format_f64(v: f64) -> String {
    if v == 0.0 {
        return "0".to_string();
    }
    // `Display` for f64 is the shortest representation that round-trips and
    // never uses exponent notation.
    format!("{v}")
}

// ---------------------------------------------------------------------------
// Recording formatter

#[derive(Debug)]
enum Tree {
    /// Already-formatted JSON scalar (number, string, bool, null).
    Scalar(String),
    Array(Vec<Tree>),
    /// Keys are already-formatted JSON strings, quotes included.
    Object(Vec<(String, Tree)>),
}

enum Open {
    Array(Vec<Tree>),
    Object(Vec<(String, Tree)>, Option<String>),
}

#[derive(Default)]
struct Recorder {
    stack: Vec<Open>,
    /// Bytes of the scalar or key being written.
    buf: Vec<u8>,
    /// The last completed container, waiting to be attached to its parent.
    done: Option<Tree>,
}

impl Recorder {
    fn take_value(&mut self) -> Tree {
        self.done
            .take()
            .unwrap_or_else(|| Tree::Scalar(self.take_buf()))
    }

    fn take_buf(&mut self) -> String {
        String::from_utf8(std::mem::take(&mut self.buf)).expect("serde_json writes UTF-8")
    }

    fn finish(mut self) -> Tree {
        assert!(self.stack.is_empty(), "unbalanced serialization");
        self.take_value()
    }

    fn scalar(&mut self, s: &str) {
        self.buf.extend_from_slice(s.as_bytes());
    }
}

macro_rules! ints {
    ($($name:ident: $t:ty),*) => {
        $(fn $name<W: ?Sized + io::Write>(&mut self, _: &mut W, v: $t) -> io::Result<()> {
            self.scalar(&v.to_string());
            Ok(())
        })*
    };
}

impl Formatter for &mut Recorder {
    fn write_null<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        self.scalar("null");
        Ok(())
    }
    fn write_bool<W: ?Sized + io::Write>(&mut self, _: &mut W, v: bool) -> io::Result<()> {
        self.scalar(if v { "true" } else { "false" });
        Ok(())
    }
    ints!(write_i8: i8, write_i16: i16, write_i32: i32, write_i64: i64, write_i128: i128,
          write_u8: u8, write_u16: u16, write_u32: u32, write_u64: u64, write_u128: u128);

    fn write_f32<W: ?Sized + io::Write>(&mut self, _: &mut W, v: f32) -> io::Result<()> {
        self.scalar(&format_f32(v));
        Ok(())
    }
    fn write_f64<W: ?Sized + io::Write>(&mut self, _: &mut W, v: f64) -> io::Result<()> {
        self.scalar(&format_f64(v));
        Ok(())
    }
    fn write_number_str<W: ?Sized + io::Write>(&mut self, _: &mut W, v: &str) -> io::Result<()> {
        self.scalar(v);
        Ok(())
    }
    fn begin_string<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        self.scalar("\"");
        Ok(())
    }
    fn end_string<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        self.scalar("\"");
        Ok(())
    }
    fn write_string_fragment<W: ?Sized + io::Write>(
        &mut self,
        _: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        self.scalar(fragment);
        Ok(())
    }
    fn write_char_escape<W: ?Sized + io::Write>(
        &mut self,
        _: &mut W,
        escape: CharEscape,
    ) -> io::Result<()> {
        CompactFormatter.write_char_escape(&mut self.buf, escape)
    }
    fn write_raw_fragment<W: ?Sized + io::Write>(&mut self, _: &mut W, v: &str) -> io::Result<()> {
        self.scalar(v);
        Ok(())
    }

    fn begin_array<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        self.stack.push(Open::Array(Vec::new()));
        Ok(())
    }
    fn begin_array_value<W: ?Sized + io::Write>(&mut self, _: &mut W, _: bool) -> io::Result<()> {
        Ok(())
    }
    fn end_array_value<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        let v = self.take_value();
        match self.stack.last_mut() {
            Some(Open::Array(items)) => items.push(v),
            _ => unreachable!("array value outside an array"),
        }
        Ok(())
    }
    fn end_array<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        match self.stack.pop() {
            Some(Open::Array(items)) => self.done = Some(Tree::Array(items)),
            _ => unreachable!("end_array outside an array"),
        }
        Ok(())
    }

    fn begin_object<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        self.stack.push(Open::Object(Vec::new(), None));
        Ok(())
    }
    fn begin_object_key<W: ?Sized + io::Write>(&mut self, _: &mut W, _: bool) -> io::Result<()> {
        self.buf.clear();
        Ok(())
    }
    fn end_object_key<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        let key = self.take_buf();
        match self.stack.last_mut() {
            Some(Open::Object(_, pending)) => *pending = Some(key),
            _ => unreachable!("object key outside an object"),
        }
        Ok(())
    }
    fn begin_object_value<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        Ok(())
    }
    fn end_object_value<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        let v = self.take_value();
        match self.stack.last_mut() {
            Some(Open::Object(entries, pending)) => {
                entries.push((pending.take().expect("value without key"), v))
            }
            _ => unreachable!("object value outside an object"),
        }
        Ok(())
    }
    fn end_object<W: ?Sized + io::Write>(&mut self, _: &mut W) -> io::Result<()> {
        match self.stack.pop() {
            Some(Open::Object(entries, _)) => self.done = Some(Tree::Object(entries)),
            _ => unreachable!("end_object outside an object"),
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Layout

/// Nesting depth: scalars 0, a container 1 + its deepest child.
fn depth(t: &Tree) -> usize {
    match t {
        Tree::Scalar(_) => 0,
        Tree::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
        Tree::Object(entries) => 1 + entries.iter().map(|(_, v)| depth(v)).max().unwrap_or(0),
    }
}

fn has_id(t: &Tree) -> bool {
    matches!(t, Tree::Object(entries) if entries.iter().any(|(k, _)| k == "\"id\""))
}

fn one_line(t: &Tree, out: &mut String) {
    match t {
        Tree::Scalar(s) => out.push_str(s),
        Tree::Array(items) if items.is_empty() => out.push_str("[]"),
        Tree::Object(entries) if entries.is_empty() => out.push_str("{}"),
        Tree::Array(items) => {
            out.push('[');
            for (i, v) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                one_line(v, out);
            }
            out.push(']');
        }
        Tree::Object(entries) => {
            out.push_str("{ ");
            for (i, (k, v)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(k);
                out.push_str(": ");
                one_line(v, out);
            }
            out.push_str(" }");
        }
    }
}

/// Render `t` at `level`; `used` is how many columns precede it on its line
/// (indentation plus `"key": `), and one more column is reserved for a comma.
fn render(t: &Tree, level: usize, used: usize, out: &mut String) {
    // The root is always expanded so files read the same however small.
    if level > 0 && depth(t) <= 2 && !has_id(t) {
        let mut line = String::new();
        one_line(t, &mut line);
        if used + line.chars().count() < MAX_LINE {
            out.push_str(&line);
            return;
        }
    }
    let inner = INDENT.repeat(level + 1);
    match t {
        Tree::Scalar(s) => out.push_str(s),
        Tree::Array(items) => {
            out.push_str("[\n");
            for (i, v) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&inner);
                render(v, level + 1, inner.len(), out);
            }
            out.push('\n');
            out.push_str(&INDENT.repeat(level));
            out.push(']');
        }
        Tree::Object(entries) => {
            out.push_str("{\n");
            let mut i = 0;
            while i < entries.len() {
                if i > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&inner);
                // A run of scalar geometry keys shares one line.
                let is_geom = |(k, v): &(String, Tree)| {
                    GEOMETRY_KEYS.contains(&k.as_str()) && matches!(v, Tree::Scalar(_))
                };
                if is_geom(&entries[i]) {
                    let mut first = true;
                    while i < entries.len() && is_geom(&entries[i]) {
                        if !first {
                            out.push_str(", ");
                        }
                        first = false;
                        let (k, v) = &entries[i];
                        out.push_str(k);
                        out.push_str(": ");
                        one_line(v, out);
                        i += 1;
                    }
                    continue;
                }
                let (k, v) = &entries[i];
                out.push_str(k);
                out.push_str(": ");
                render(v, level + 1, inner.len() + k.chars().count() + 2, out);
                i += 1;
            }
            out.push('\n');
            out.push_str(&INDENT.repeat(level));
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_are_rounded_and_shortest() {
        assert_eq!(format_f32(16.0), "16");
        assert_eq!(format_f32(0.5), "0.5");
        assert_eq!(format_f32(0.1), "0.1");
        assert_eq!(format_f32(1.005), "1"); // 1.005f32 is just below 1.005
        assert_eq!(format_f32(1.239), "1.24");
        assert_eq!(format_f32(-0.001), "0");
        assert_eq!(format_f32(-0.0), "0");
        assert_eq!(format_f32(-12.5), "-12.5");
        assert_eq!(format_f32(1440.0), "1440");
        assert_eq!(format_f32(123456.78), "123456.78");
        assert_eq!(format_f64(16.0), "16");
        assert_eq!(format_f64(1e-7), "0.0000001");
    }

    #[test]
    fn rounding_is_idempotent_through_f32() {
        let mut x = -5000.0f32;
        while x < 5000.0 {
            let once = format_f32(x);
            let back: f32 = once.parse().unwrap();
            assert_eq!(format_f32(back), once, "{x}");
            x += 0.7313;
        }
    }

    #[test]
    fn short_leaf_containers_go_on_one_line() {
        let v = json!({
            "a": [1, 2, 3],
            "b": { "c": "d", "e": [{ "f": 1 }] },
            "fills": [{ "type": "solid", "color": "#fff" }],
            "nested": { "deeper": { "deepest": { "x": 1 } } },
            "empty": [],
            "obj": {},
        });
        let s = to_canonical_string(&v).unwrap();
        assert_eq!(
            s,
            r##"{
  "a": [1, 2, 3],
  "b": {
    "c": "d",
    "e": [{ "f": 1 }]
  },
  "empty": [],
  "fills": [{ "color": "#fff", "type": "solid" }],
  "nested": {
    "deeper": { "deepest": { "x": 1 } }
  },
  "obj": {}
}
"##
        );
    }

    #[test]
    fn long_containers_are_expanded() {
        let long = "x".repeat(90);
        let v = json!({ "k": [long, "y"] });
        let s = to_canonical_string(&v).unwrap();
        assert_eq!(
            s,
            format!("{{\n  \"k\": [\n    \"{long}\",\n    \"y\"\n  ]\n}}\n")
        );
    }

    #[test]
    fn line_limit_is_exact() {
        // `  "a": { "k": "…" },` must be at most 100 columns including the
        // comma: 7 columns before the value, 11 around the string, 1 comma.
        for (len, inline) in [(81, true), (82, false)] {
            let v = json!({ "a": { "k": "x".repeat(len) }, "b": 1 });
            let s = to_canonical_string(&v).unwrap();
            let first_value_line = s.lines().nth(1).unwrap();
            assert_eq!(first_value_line.starts_with("  \"a\": { "), inline, "{s}");
            for line in s.lines() {
                assert!(line.chars().count() <= MAX_LINE, "{line}");
            }
        }
    }

    #[test]
    fn objects_with_ids_are_expanded_and_geometry_shares_a_line() {
        let v = json!({ "id": "n_0000000000", "x": 1, "y": 2, "name": "a" });
        // json! sorts keys: id, name, x, y.
        let s = to_canonical_string(&v).unwrap();
        assert_eq!(
            s,
            "{\n  \"id\": \"n_0000000000\",\n  \"name\": \"a\",\n  \"x\": 1, \"y\": 2\n}\n"
        );
    }

    #[test]
    fn strings_are_escaped() {
        let s = to_canonical_string(&json!(["a\"b\\c\nd\u{1}é"])).unwrap();
        assert_eq!(s, "[\n  \"a\\\"b\\\\c\\nd\\u0001é\"\n]\n");
    }
}
