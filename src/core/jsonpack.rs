//! Lossless JSON packing for structured command output (`gh … --json`, `gh api`).
//!
//! JSON arrays of uniform objects waste most of their bytes repeating the same
//! field names on every row. This module re-encodes them so field names appear
//! once, without dropping a single value:
//!
//! - **Top-level array of objects** → a CSV+schema table:
//!   ```text
//!   [3]{id:int,name:string,tag:string?}
//!   1,alice,x
//!   2,bob,
//!   3,"c,d",null
//!   ```
//!   `[N]` declares the row count (the model can detect truncation), `?` marks
//!   nullable columns, an empty cell means the key was absent, a bare `null`
//!   means the key was present and null.
//!
//! - **Objects (envelopes, e.g. `gh api` responses)** → same JSON shape, but
//!   every dense inner array of objects is replaced in place by
//!   `{"_cols":[…],"_rows":[[…],…]}` and the document is minified. Output
//!   stays valid JSON.
//!
//! # Losslessness is verified, not assumed
//!
//! [`pack`] never trusts its own encoder: it decodes the packed text with
//! [`unpack`] and compares the result against the parsed input. Any mismatch —
//! including adversarial inputs that collide with the packed notation itself —
//! returns `None`, and callers fall back to the raw output (Never Block).
//! The only semantic normalization is serde_json's number parsing (`1.10` →
//! `1.1`), which applies to every JSON path in rtk alike.
//!
//! # Attribution
//!
//! The tabular compaction approach (schema declaration + CSV rows, frequency
//! ordering, nested-uniform flattening) is ported from the lossless compaction
//! stage of `headroom-core` (<https://github.com/headroomlabs-ai/headroom>,
//! Apache-2.0), with its lossy paths removed: no retrieval pointers, no row
//! dropping, no stringified-JSON rewriting. Cells here are only ever rendered
//! verbatim, and the round-trip check above makes that a runtime guarantee.

use serde_json::Value;

/// Minimum array length worth packing. Below this the declaration overhead
/// beats the savings.
const MIN_ITEMS: usize = 2;

/// Cap on inner-key count for nested-uniform flattening (headroom parity).
/// Wider inner objects stay as JSON cells rather than exploding the table.
const MAX_FLATTEN_INNER_KEYS: usize = 6;

/// Keys of the in-place table notation used inside JSON envelopes.
const COLS_KEY: &str = "_cols";
const ROWS_KEY: &str = "_rows";

/// Type tags a column may carry in the CSV declaration.
const TYPE_TAGS: [&str; 6] = ["int", "float", "bool", "string", "null", "json"];

/// Pack a JSON document losslessly. Returns `None` when the input is not
/// JSON, would not shrink, or fails the round-trip verification — callers
/// must then emit the raw output unchanged.
pub fn pack(raw: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(raw).ok()?;
    let shrinks = |p: String| if p.len() < raw.len() { Some(p) } else { None };
    // CSV+schema is the preferred encoding (models read it best); the JSON
    // `_cols` notation is the fallback when CSV declines or cannot shrink
    // (e.g. wide unflattened objects, whose JSON cells pay the CSV
    // quote-doubling tax).
    let packed = match &parsed {
        Value::Array(items) => csv_table(items)
            .and_then(shrinks)
            .or_else(|| walk_pack(&parsed).and_then(shrinks))?,
        Value::Object(_) => walk_pack(&parsed).and_then(shrinks)?,
        _ => return None,
    };
    // The round-trip proof: decode our own output and require exact value
    // equality with the input. Catches every encoder bug and every
    // notation collision at runtime, before a single byte is emitted.
    if unpack(&packed)? != parsed {
        return None;
    }
    Some(packed)
}

/// Decode a packed document back to the JSON value it encodes. Inverse of
/// [`pack`]; used by the round-trip verification and by tests.
pub fn unpack(packed: &str) -> Option<Value> {
    let first_line = packed.lines().next().unwrap_or("");
    if parse_declaration(first_line).is_some() {
        return unpack_table(packed);
    }
    let v: Value = serde_json::from_str(packed).ok()?;
    Some(decode_walk(v))
}

// ─────────────────────────── CSV+schema encoder ───────────────────────────

/// One table cell: the field's value, or `None` when the key was absent.
type Cell = Option<Value>;

fn csv_table(items: &[Value]) -> Option<String> {
    if items.len() < MIN_ITEMS {
        return None;
    }
    let objs: Vec<&serde_json::Map<String, Value>> =
        items.iter().map(|v| v.as_object()).collect::<Option<_>>()?;

    // Column order: frequency descending, then alphabetical (headroom
    // parity) — stable and puts core fields first.
    let mut freq: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for obj in &objs {
        for k in obj.keys() {
            *freq.entry(k.as_str()).or_insert(0) += 1;
        }
    }
    if freq.is_empty() {
        return None;
    }
    // Keys must survive the declaration line verbatim, and must not collide
    // with the dotted flatten notation.
    if freq.keys().any(|k| !csv_safe_name(k) || k.contains('.')) {
        return None;
    }
    let mut ordered: Vec<(&str, usize)> = freq.iter().map(|(k, f)| (*k, *f)).collect();
    ordered.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));

    let mut names: Vec<String> = ordered.iter().map(|(k, _)| k.to_string()).collect();
    let mut rows: Vec<Vec<Cell>> = objs
        .iter()
        .map(|obj| names.iter().map(|k| obj.get(k).cloned()).collect())
        .collect();

    flatten_uniform_nested(&mut names, &mut rows);

    // Infer a type tag + nullability per column, from the flattened cells.
    let col_types: Vec<(String, bool)> = (0..names.len()).map(|c| infer_column(&rows, c)).collect();

    let mut out = String::new();
    out.push('[');
    out.push_str(&rows.len().to_string());
    out.push_str("]{");
    let decl: Vec<String> = names
        .iter()
        .zip(&col_types)
        .map(|(n, (ty, nullable))| {
            if *nullable {
                format!("{n}:{ty}?")
            } else {
                format!("{n}:{ty}")
            }
        })
        .collect();
    out.push_str(&decl.join(","));
    out.push_str("}\n");
    for row in &rows {
        let cells: Vec<String> = row
            .iter()
            .zip(&col_types)
            .map(|(cell, (ty, _))| render_cell(cell, ty))
            .collect();
        out.push_str(&cells.join(","));
        out.push('\n');
    }
    Some(out)
}

/// Promote columns whose every present cell is a non-empty object with one
/// identical key list into dotted columns (`author.login`). One level only;
/// null parents, mixed inner keys, wide inner objects, dotted inner keys,
/// and name collisions all block the promotion for that column.
fn flatten_uniform_nested(names: &mut Vec<String>, rows: &mut [Vec<Cell>]) {
    let mut i = 0;
    while i < names.len() {
        if names[i].contains('.') {
            i += 1;
            continue;
        }
        let Some(inner_keys) = uniform_inner_keys(rows, i) else {
            i += 1;
            continue;
        };
        if inner_keys.is_empty()
            || inner_keys.len() > MAX_FLATTEN_INNER_KEYS
            || inner_keys
                .iter()
                .any(|k| !csv_safe_name(k) || k.contains('.'))
        {
            i += 1;
            continue;
        }
        let parent = names[i].clone();
        let new_names: Vec<String> = inner_keys.iter().map(|k| format!("{parent}.{k}")).collect();
        if new_names
            .iter()
            .any(|n| names.iter().enumerate().any(|(j, e)| j != i && e == n))
        {
            i += 1;
            continue;
        }

        names.splice(i..i + 1, new_names);
        for row in rows.iter_mut() {
            let original = row.remove(i);
            let inner_obj = match original {
                Some(Value::Object(map)) => Some(map),
                None => None,
                _ => unreachable!("uniform_inner_keys guarantees object-or-missing"),
            };
            let expanded = inner_keys.iter().map(|k| match &inner_obj {
                None => None,
                Some(map) => map.get(k).cloned(),
            });
            for (offset, cell) in expanded.enumerate() {
                row.insert(i + offset, cell);
            }
        }
        i += inner_keys.len();
    }
}

/// `Some(keys)` when every present cell in column `col` is a non-empty
/// object with exactly this key list (same order). `None` otherwise, or
/// when the column has no objects at all.
fn uniform_inner_keys(rows: &[Vec<Cell>], col: usize) -> Option<Vec<String>> {
    let mut canonical: Option<Vec<String>> = None;
    for row in rows {
        match &row[col] {
            None => continue,
            Some(Value::Object(map)) if !map.is_empty() => {
                let keys: Vec<String> = map.keys().cloned().collect();
                match &canonical {
                    None => canonical = Some(keys),
                    Some(existing) if *existing != keys => return None,
                    Some(_) => {}
                }
            }
            Some(_) => return None,
        }
    }
    canonical
}

fn infer_column(rows: &[Vec<Cell>], col: usize) -> (String, bool) {
    let mut nullable = false;
    let mut tag: Option<&'static str> = None;
    for row in rows {
        match &row[col] {
            None | Some(Value::Null) => nullable = true,
            Some(v) => {
                let t = type_tag_for(v);
                tag = Some(match tag {
                    None => t,
                    Some(prev) if prev == t => prev,
                    // int + float widen to float; any other mix is json.
                    Some("int") if t == "float" => "float",
                    Some("float") if t == "int" => "float",
                    Some(_) => "json",
                });
            }
        }
    }
    (tag.unwrap_or("null").to_string(), nullable)
}

fn type_tag_for(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_i64() || n.is_u64() => "int",
        Value::Number(_) => "float",
        Value::String(_) => "string",
        Value::Object(_) | Value::Array(_) => "json",
    }
}

fn render_cell(cell: &Cell, ty: &str) -> String {
    let v = match cell {
        None => return String::new(),
        Some(v) => v,
    };
    if v.is_null() {
        return "null".to_string();
    }
    match ty {
        // Mixed columns carry every value as a JSON literal, so
        // 42-the-int and "42"-the-string can never blur together.
        "json" => csv_maybe_quote(&serde_json::to_string(v).unwrap_or_default()),
        "string" => {
            let s = v.as_str().unwrap_or_default();
            if s.is_empty() || s == "null" || needs_csv_quote(s) {
                csv_quote(s)
            } else {
                s.to_string()
            }
        }
        // int / float / bool / null literals never contain CSV specials.
        _ => v.to_string(),
    }
}

fn csv_safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.trim() == name
        && !name
            .chars()
            .any(|c| matches!(c, ',' | ':' | '?' | '{' | '}' | '"' | '\n' | '\r'))
}

fn needs_csv_quote(s: &str) -> bool {
    s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r')
}

fn csv_maybe_quote(s: &str) -> String {
    if needs_csv_quote(s) {
        csv_quote(s)
    } else {
        s.to_string()
    }
}

fn csv_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '"' {
            out.push('"');
        }
        out.push(c);
    }
    out.push('"');
    out
}

// ─────────────────────────── CSV+schema decoder ───────────────────────────

struct ColSpec {
    name: String,
    ty: String,
}

fn parse_declaration(line: &str) -> Option<(usize, Vec<ColSpec>)> {
    let rest = line.strip_prefix('[')?;
    let (count_str, rest) = rest.split_once(']')?;
    if count_str.is_empty() || !count_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: usize = count_str.parse().ok()?;
    let inner = rest.strip_prefix('{')?.strip_suffix('}')?;
    if inner.is_empty() {
        return None;
    }
    let mut cols = Vec::new();
    for part in inner.split(',') {
        let (name, ty_raw) = part.split_once(':')?;
        let ty = ty_raw.strip_suffix('?').unwrap_or(ty_raw);
        if name.is_empty() || !TYPE_TAGS.contains(&ty) {
            return None;
        }
        cols.push(ColSpec {
            name: name.to_string(),
            ty: ty.to_string(),
        });
    }
    Some((n, cols))
}

fn unpack_table(text: &str) -> Option<Value> {
    let (decl_line, body) = text.split_once('\n')?;
    let (n, cols) = parse_declaration(decl_line)?;
    let records = parse_csv_records(body, n, cols.len())?;
    let items: Vec<Value> = records
        .iter()
        .map(|record| record_to_object(record, &cols))
        .collect::<Option<_>>()?;
    Some(Value::Array(items))
}

/// A parsed CSV field: its content and whether it was quoted (the decoder
/// needs the distinction to tell `""` the empty string from a missing key).
struct Field {
    content: String,
    quoted: bool,
}

/// Strict CSV reader: every record is newline-terminated, quotes follow
/// RFC 4180 (`""` escapes a quote, quoted fields may span lines). Exactly
/// `n_records` × `n_fields` or `None`.
fn parse_csv_records(body: &str, n_records: usize, n_fields: usize) -> Option<Vec<Vec<Field>>> {
    let mut records: Vec<Vec<Field>> = Vec::new();
    let mut fields: Vec<Field> = Vec::new();
    let mut cur = String::new();
    let mut cur_quoted = false;
    let mut in_quotes = false;
    let mut after_close = false; // a quoted field just closed; only , or \n may follow

    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    cur.push('"');
                } else {
                    in_quotes = false;
                    after_close = true;
                }
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            '"' if cur.is_empty() && !cur_quoted => {
                in_quotes = true;
                cur_quoted = true;
            }
            ',' => {
                fields.push(Field {
                    content: std::mem::take(&mut cur),
                    quoted: cur_quoted,
                });
                cur_quoted = false;
                after_close = false;
            }
            '\n' => {
                fields.push(Field {
                    content: std::mem::take(&mut cur),
                    quoted: cur_quoted,
                });
                cur_quoted = false;
                after_close = false;
                if fields.len() != n_fields {
                    return None;
                }
                records.push(std::mem::take(&mut fields));
            }
            _ if after_close => return None, // junk after a closing quote
            _ if cur_quoted => return None,  // junk after a quoted field
            _ => cur.push(c),
        }
    }
    // Unterminated quote or a record without its closing newline.
    if in_quotes || !cur.is_empty() || !fields.is_empty() || cur_quoted {
        return None;
    }
    if records.len() != n_records {
        return None;
    }
    Some(records)
}

fn record_to_object(record: &[Field], cols: &[ColSpec]) -> Option<Value> {
    // Cells first (None = key absent in this row).
    let cells: Vec<Option<Value>> = record
        .iter()
        .zip(cols)
        .map(|(f, c)| field_to_value(f, &c.ty))
        .collect::<Option<_>>()?;

    // Rebuild objects, folding dotted columns back into their parent.
    let mut obj = serde_json::Map::new();
    let mut done_parents: Vec<&str> = Vec::new();
    for (idx, col) in cols.iter().enumerate() {
        match col.name.split_once('.') {
            None => {
                if let Some(v) = cells[idx].clone() {
                    obj.insert(col.name.clone(), v);
                }
            }
            Some((parent, _)) => {
                if done_parents.contains(&parent) {
                    continue;
                }
                done_parents.push(parent);
                let mut inner = serde_json::Map::new();
                let mut any_present = false;
                for (j, other) in cols.iter().enumerate() {
                    if let Some((p, key)) = other.name.split_once('.') {
                        if p == parent {
                            if let Some(v) = cells[j].clone() {
                                inner.insert(key.to_string(), v);
                                any_present = true;
                            }
                        }
                    }
                }
                if any_present {
                    obj.insert(parent.to_string(), Value::Object(inner));
                }
            }
        }
    }
    Some(Value::Object(obj))
}

/// Decode one field under its column type. `Some(None)` means the key was
/// absent; outer `None` means the text is not a valid encoding.
#[allow(clippy::option_option)]
fn field_to_value(f: &Field, ty: &str) -> Option<Option<Value>> {
    if !f.quoted && f.content.is_empty() {
        return Some(None);
    }
    if !f.quoted && f.content == "null" {
        return Some(Some(Value::Null));
    }
    let v = match ty {
        "string" => Value::String(f.content.clone()),
        "json" => serde_json::from_str(&f.content).ok()?,
        "int" => {
            let v: Value = serde_json::from_str(&f.content).ok()?;
            match &v {
                Value::Number(n) if n.is_i64() || n.is_u64() => v,
                _ => return None,
            }
        }
        "float" => {
            let v: Value = serde_json::from_str(&f.content).ok()?;
            if !v.is_number() {
                return None;
            }
            v
        }
        "bool" => match f.content.as_str() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => return None,
        },
        "null" => return None, // only "null" or empty are valid, handled above
        _ => return None,
    };
    if f.quoted && ty != "string" && ty != "json" {
        return None; // scalar columns are never quoted by the encoder
    }
    Some(Some(v))
}

// ─────────────── envelope mode: in-place `_cols`/`_rows` tables ───────────────

fn walk_pack(parsed: &Value) -> Option<String> {
    let walked = walk_replace(parsed.clone());
    serde_json::to_string(&walked).ok()
}

fn walk_replace(v: Value) -> Value {
    match v {
        Value::Array(items) => {
            // Recurse first so inner tables are in place before the outer
            // array is considered (headroom's walker order).
            let items: Vec<Value> = items.into_iter().map(walk_replace).collect();
            match dense_table(&items) {
                Some(table) => table,
                None => Value::Array(items),
            }
        }
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, walk_replace(v))).collect())
        }
        other => other,
    }
}

/// Dense arrays only: every item an object with the same key SET. Sparse
/// arrays stay verbatim — JSON rows have no way to say "absent" that is
/// distinct from null, and inventing one would break citation fidelity.
fn dense_table(items: &[Value]) -> Option<Value> {
    if items.len() < MIN_ITEMS {
        return None;
    }
    let objs: Vec<&serde_json::Map<String, Value>> =
        items.iter().map(|v| v.as_object()).collect::<Option<_>>()?;
    let cols: Vec<&String> = objs[0].keys().collect();
    if cols.is_empty() {
        return None;
    }
    for obj in &objs[1..] {
        if obj.len() != cols.len() || !cols.iter().all(|k| obj.contains_key(*k)) {
            return None;
        }
    }
    let mut table = serde_json::Map::new();
    table.insert(
        COLS_KEY.to_string(),
        Value::Array(cols.iter().map(|k| Value::String((*k).clone())).collect()),
    );
    table.insert(
        ROWS_KEY.to_string(),
        Value::Array(
            objs.iter()
                .map(|obj| Value::Array(cols.iter().map(|k| obj[*k].clone()).collect()))
                .collect(),
        ),
    );
    Some(Value::Object(table))
}

fn decode_walk(v: Value) -> Value {
    match v {
        Value::Object(map) => {
            if let Some(items) = decode_table_obj(&map) {
                return Value::Array(items);
            }
            Value::Object(map.into_iter().map(|(k, v)| (k, decode_walk(v))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(decode_walk).collect()),
        other => other,
    }
}

fn decode_table_obj(map: &serde_json::Map<String, Value>) -> Option<Vec<Value>> {
    if map.len() != 2 {
        return None;
    }
    let cols = map.get(COLS_KEY)?.as_array()?;
    let rows = map.get(ROWS_KEY)?.as_array()?;
    let names: Vec<&str> = cols.iter().map(|c| c.as_str()).collect::<Option<_>>()?;
    if names.is_empty() {
        return None;
    }
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let cells = row.as_array()?;
        if cells.len() != names.len() {
            return None;
        }
        let mut obj = serde_json::Map::new();
        for (name, cell) in names.iter().zip(cells) {
            obj.insert(name.to_string(), decode_walk(cell.clone()));
        }
        items.push(Value::Object(obj));
    }
    Some(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Pack, assert success, verify the round trip explicitly, return packed.
    fn pack_ok(raw: &str) -> String {
        let packed = pack(raw).unwrap_or_else(|| panic!("pack declined for: {raw}"));
        let original: Value = serde_json::from_str(raw).unwrap();
        let decoded =
            unpack(&packed).unwrap_or_else(|| panic!("unpack failed for packed:\n{packed}"));
        assert_eq!(
            decoded, original,
            "round trip mismatch for packed:\n{packed}"
        );
        packed
    }

    // ── top-level arrays → CSV+schema ──

    #[test]
    fn packs_basic_table_with_declaration() {
        let raw = r#"[{"id":1,"name":"alice"},{"id":2,"name":"bob"},{"id":3,"name":"carol-with-a-long-name"}]"#;
        let packed = pack_ok(raw);
        let mut lines = packed.lines();
        let decl = lines.next().unwrap();
        assert_eq!(decl, "[3]{id:int,name:string}", "got decl: {decl}");
        assert_eq!(lines.next().unwrap(), "1,alice");
        assert_eq!(lines.next().unwrap(), "2,bob");
        assert_eq!(lines.next().unwrap(), "3,carol-with-a-long-name");
        assert_eq!(lines.next(), None);
    }

    #[test]
    fn nasty_strings_round_trip() {
        let items = json!([
            {"k": "with,comma", "pad": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
            {"k": "with \"quotes\" inside", "pad": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
            {"k": "line one\nline two", "pad": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
            {"k": "crlf\r\nhere", "pad": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
            {"k": "emoji 🚀 and ünïcode", "pad": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
            {"k": "  leading and trailing  ", "pad": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
            {"k": "tab\there", "pad": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
        ]);
        pack_ok(&serde_json::to_string(&items).unwrap());
    }

    #[test]
    fn quoted_cell_with_newline_spans_lines_and_decodes() {
        let items = json!([
            {"id": 1, "msg": "first line\nsecond line", "pad": "xxxxxxxxxxxxxxxxxxxx"},
            {"id": 2, "msg": "plain", "pad": "xxxxxxxxxxxxxxxxxxxx"}
        ]);
        let packed = pack_ok(&serde_json::to_string(&items).unwrap());
        // The embedded newline makes the physical line count exceed the row
        // count; the decoder must still see exactly 2 records.
        assert!(packed.lines().count() > 3, "got:\n{packed}");
    }

    #[test]
    fn null_vs_missing_vs_empty_vs_lookalikes_round_trip() {
        // Column `v` exercises every ambiguity class at once: present string,
        // empty string, JSON null, absent key, and strings that mimic other
        // scalars ("null", "true", "42"). All must survive the round trip
        // (asserted by pack_ok) — this is the citation-fidelity contract.
        let raw = concat!(
            r#"[{"id":1,"v":"plain","pad":"pppppppppppppppppppp"},"#,
            r#"{"id":2,"v":"","pad":"pppppppppppppppppppp"},"#,
            r#"{"id":3,"v":null,"pad":"pppppppppppppppppppp"},"#,
            r#"{"id":4,"pad":"pppppppppppppppppppp"},"#,
            r#"{"id":5,"v":"null","pad":"pppppppppppppppppppp"},"#,
            r#"{"id":6,"v":"true","pad":"pppppppppppppppppppp"},"#,
            r#"{"id":7,"v":"42","pad":"pppppppppppppppppppp"}]"#
        );
        let packed = pack_ok(raw);
        let decl = packed.lines().next().unwrap();
        assert!(
            decl.contains("v:string?"),
            "nullable string expected, got: {decl}"
        );
    }

    #[test]
    fn bool_and_all_null_columns_round_trip() {
        let raw = concat!(
            r#"[{"ok":true,"gone":null,"pad":"pppppppppppppppppppp"},"#,
            r#"{"ok":false,"gone":null,"pad":"pppppppppppppppppppp"}]"#
        );
        let packed = pack_ok(raw);
        let decl = packed.lines().next().unwrap();
        assert!(decl.contains("ok:bool"), "got: {decl}");
        assert!(decl.contains("gone:null?"), "got: {decl}");
    }

    #[test]
    fn extreme_numbers_round_trip() {
        let raw = format!(
            concat!(
                r#"[{{"a":{},"b":{},"c":1.5,"d":1e300,"e":0.0000001,"pad":"pppppppppppppppppppp"}},"#,
                r#"{{"a":0,"b":7,"c":-2.25,"d":-1e300,"e":123456789.5,"pad":"pppppppppppppppppppp"}}]"#
            ),
            i64::MIN,
            u64::MAX
        );
        pack_ok(&raw);
    }

    #[test]
    fn mixed_int_and_float_column_round_trips() {
        let raw = concat!(
            r#"[{"n":1,"pad":"pppppppppppppppppppppppppppppp"},"#,
            r#"{"n":2.5,"pad":"pppppppppppppppppppppppppppppp"}]"#
        );
        let packed = pack_ok(raw);
        assert!(
            packed.lines().next().unwrap().contains("n:float"),
            "int+float should widen to float, got: {packed}"
        );
    }

    #[test]
    fn mixed_type_column_renders_json_cells_and_round_trips() {
        // int + string in one column → every cell in that column is a JSON
        // literal, so "42"-the-string and 42-the-int stay distinct.
        let raw = concat!(
            r#"[{"v":42,"pad":"pppppppppppppppppppppppppppppp"},"#,
            r#"{"v":"42","pad":"pppppppppppppppppppppppppppppp"},"#,
            r#"{"v":{"deep":[1,2]},"pad":"pppppppppppppppppppppppppppppp"}]"#
        );
        let packed = pack_ok(raw);
        assert!(
            packed.lines().next().unwrap().contains("v:json"),
            "got: {packed}"
        );
    }

    #[test]
    fn sparse_columns_keep_missing_distinct() {
        // `extra` exists only in one row; the decl must mark it nullable and
        // the round trip (pack_ok) must restore rows without inventing keys.
        let raw = concat!(
            r#"[{"id":1,"extra":"x","pad":"pppppppppppppppppppp"},"#,
            r#"{"id":2,"pad":"pppppppppppppppppppp"},"#,
            r#"{"id":3,"pad":"pppppppppppppppppppp"}]"#
        );
        let packed = pack_ok(raw);
        assert!(packed.lines().next().unwrap().contains("extra:string?"));
    }

    // ── nested-uniform flattening ──

    #[test]
    fn uniform_nested_object_flattens_to_dotted_columns() {
        let raw = concat!(
            r#"[{"id":1,"author":{"login":"alice","bot":false}},"#,
            r#"{"id":2,"author":{"login":"bob","bot":true}},"#,
            r#"{"id":3,"author":{"login":"carol","bot":false}}]"#
        );
        let packed = pack_ok(raw);
        let decl = packed.lines().next().unwrap();
        assert!(decl.contains("author.login:string"), "got: {decl}");
        assert!(decl.contains("author.bot:bool"), "got: {decl}");
        assert!(
            !decl.contains("author:"),
            "parent column must be gone: {decl}"
        );
    }

    #[test]
    fn flatten_with_missing_parent_round_trips() {
        // Row 2 has no `author` at all — after flattening, both dotted cells
        // are empty and decode must restore "no author key", not
        // `author: {}` and not `author: null`.
        let raw = concat!(
            r#"[{"id":1,"author":{"login":"alice","bot":false}},"#,
            r#"{"id":2},"#,
            r#"{"id":3,"author":{"login":"carol","bot":true}}]"#
        );
        let packed = pack_ok(raw);
        assert!(packed.lines().next().unwrap().contains("author.login"));
    }

    #[test]
    fn non_uniform_nested_object_stays_json_column() {
        // Paired control for the flatten tests: inner key sets differ, so
        // the parent must stay a single `json` column and still round-trip.
        let raw = concat!(
            r#"[{"id":1,"author":{"login":"alice"}},"#,
            r#"{"id":2,"author":{"login":"bob","bot":true}},"#,
            r#"{"id":3,"author":{"login":"carol"}}]"#
        );
        let packed = pack_ok(raw);
        let decl = packed.lines().next().unwrap();
        assert!(decl.contains("author:json"), "got: {decl}");
        assert!(!decl.contains("author.login"), "must not flatten: {decl}");
    }

    #[test]
    fn null_parent_blocks_flattening() {
        let raw = concat!(
            r#"[{"id":1,"author":{"login":"alice","bot":false}},"#,
            r#"{"id":2,"author":null},"#,
            r#"{"id":3,"author":{"login":"carol","bot":true}}]"#
        );
        let packed = pack_ok(raw);
        assert!(
            !packed.lines().next().unwrap().contains("author.login"),
            "null parent cannot flatten: {packed}"
        );
    }

    #[test]
    fn wide_inner_objects_do_not_flatten() {
        // 7 inner keys > MAX_FLATTEN_INNER_KEYS — stays a json column. The
        // flat sibling columns give CSV enough headroom to win overall even
        // though the json cells pay the quote-doubling tax.
        let inner: Value = json!({"a":1,"b":2,"c":3,"d":4,"e":5,"f":6,"g":7});
        let items: Vec<Value> = (0..5)
            .map(|i| {
                json!({
                    "id": i,
                    "alpha": format!("alpha-value-{i:03}"),
                    "beta": format!("beta-value-{i:03}"),
                    "gamma": format!("gamma-value-{i:03}"),
                    "m": inner
                })
            })
            .collect();
        let packed = pack_ok(&serde_json::to_string(&Value::Array(items)).unwrap());
        let decl = packed.lines().next().unwrap();
        assert!(decl.contains("m:json"), "got: {decl}");
        assert!(
            !decl.contains("m.a"),
            "must not flatten 7 inner keys: {decl}"
        );
    }

    // ── envelopes (gh api style) → in-place _cols/_rows, valid JSON out ──

    #[test]
    fn envelope_dense_array_becomes_cols_rows() {
        let raw = concat!(
            "{\n  \"total_count\": 2,\n  \"workflow_runs\": [\n",
            "    {\"id\": 11, \"status\": \"completed\", \"conclusion\": \"success\"},\n",
            "    {\"id\": 12, \"status\": \"completed\", \"conclusion\": \"failure\"}\n",
            "  ]\n}\n"
        );
        let packed = pack_ok(raw);
        assert!(packed.starts_with('{'), "envelope must stay JSON: {packed}");
        assert!(packed.contains("\"_cols\""), "got: {packed}");
        assert!(packed.contains("\"_rows\""), "got: {packed}");
        assert!(
            packed.contains("\"total_count\":2"),
            "envelope fields kept: {packed}"
        );
    }

    #[test]
    fn envelope_nested_deep_arrays_replaced() {
        let raw = serde_json::to_string_pretty(&json!({
            "data": {
                "repository": {
                    "issues": [
                        {"n": 1, "t": "first issue title here"},
                        {"n": 2, "t": "second issue title here"},
                        {"n": 3, "t": "third issue title here"}
                    ]
                }
            }
        }))
        .unwrap();
        let packed = pack_ok(&raw);
        assert!(packed.contains("\"_cols\""), "got: {packed}");
    }

    #[test]
    fn envelope_sparse_inner_array_left_verbatim_but_still_minifies() {
        // The inner array is NOT dense (differing key sets) so it must stay
        // as-is; a pretty-printed input still shrinks via minification.
        let raw = serde_json::to_string_pretty(&json!({
            "items": [
                {"a": 1},
                {"a": 2, "b": 3}
            ],
            "note": "sparse arrays must never be tabled in envelope mode"
        }))
        .unwrap();
        let packed = pack_ok(&raw);
        assert!(
            !packed.contains("_cols"),
            "sparse must not be tabled: {packed}"
        );
        assert!(packed.len() < raw.len());
    }

    #[test]
    fn already_minified_envelope_with_nothing_to_do_declines() {
        let raw = r#"{"a":1,"b":"two"}"#;
        assert_eq!(pack(raw), None, "no shrink possible — must decline");
    }

    #[test]
    fn pretty_single_object_minifies_losslessly() {
        // `gh api` pretty-prints single objects; minification alone is a real,
        // trivially lossless saving.
        let raw = serde_json::to_string_pretty(&json!({
            "name": "rtk",
            "full_name": "rtk-ai/rtk",
            "private": false,
            "topics": ["cli", "llm", "tokens"]
        }))
        .unwrap();
        let packed = pack_ok(&raw);
        assert!(packed.len() < raw.len());
        assert!(!packed.contains('\n'), "minified output expected: {packed}");
    }

    // ── self-defense: data colliding with the packed notation ──

    #[test]
    fn data_containing_fake_cols_rows_object_is_refused() {
        // The envelope contains a REAL field shaped exactly like our table
        // notation. Decoding the packed form would turn it into an array the
        // input never had — the round-trip verifier must catch that and
        // decline the whole pack.
        let raw = serde_json::to_string_pretty(&json!({
            "legit": {"_cols": ["a"], "_rows": [[1]]},
            "runs": [
                {"id": 1, "s": "ok"},
                {"id": 2, "s": "ok"}
            ]
        }))
        .unwrap();
        assert_eq!(
            pack(&raw),
            None,
            "colliding notation in data must refuse to pack"
        );

        // Paired control: the same envelope without the collision packs fine.
        let control = serde_json::to_string_pretty(&json!({
            "legit": {"other": 1},
            "runs": [
                {"id": 1, "s": "ok"},
                {"id": 2, "s": "ok"}
            ]
        }))
        .unwrap();
        let packed = pack_ok(&control);
        assert!(packed.contains("_cols"));
    }

    // ── decline paths (each paired with a qualifying control) ──

    #[test]
    fn non_json_and_scalars_decline() {
        assert_eq!(pack("plain text, not json"), None);
        assert_eq!(pack(""), None);
        assert_eq!(pack("42"), None);
        assert_eq!(pack("\"a string\""), None);
    }

    #[test]
    fn single_item_array_declines_but_two_pack() {
        let one = r#"[{"id":1,"name":"only-one-row-here"}]"#;
        assert_eq!(pack(one), None, "1 row is below MIN_ITEMS");
        let two = r#"[{"id":1,"name":"first-row-here"},{"id":2,"name":"second-row-here"}]"#;
        pack_ok(two);
    }

    #[test]
    fn scalar_array_declines() {
        assert_eq!(pack("[1,2,3,4,5]"), None);
        assert_eq!(pack(r#"["a","b","c"]"#), None);
    }

    #[test]
    fn mixed_object_and_scalar_array_declines() {
        // Not all objects → no CSV table. Minified input leaves nothing to
        // shrink → full decline.
        let raw = r#"[{"a":1},2,{"a":3}]"#;
        assert_eq!(pack(raw), None);
    }

    #[test]
    fn dotted_original_keys_never_emit_csv() {
        // A literal "a.b" key would collide with flatten notation on decode;
        // CSV mode must refuse. The JSON `_cols` notation carries any key
        // safely, so the array may still pack as an envelope-style table.
        let raw = concat!(
            r#"[{"a.b":1,"pad":"pppppppppppppppppppppppppppppp"},"#,
            r#"{"a.b":2,"pad":"pppppppppppppppppppppppppppppp"}]"#
        );
        if let Some(packed) = pack(raw) {
            assert!(
                !packed.starts_with("[2]{"),
                "dotted keys must not use CSV declaration: {packed}"
            );
            let original: Value = serde_json::from_str(raw).unwrap();
            assert_eq!(unpack(&packed).unwrap(), original);
        }
    }

    #[test]
    fn unsafe_column_names_never_emit_csv() {
        // Commas/colons/braces/quotes in keys would corrupt the declaration
        // line. JSON `_cols` mode may still take them losslessly.
        for bad in [
            "with,comma",
            "with:colon",
            "with}brace",
            "with\"quote",
            "with\nnewline",
        ] {
            let raw = serde_json::to_string(&json!([
                {bad: 1, "pad": "pppppppppppppppppppppppppppppp"},
                {bad: 2, "pad": "pppppppppppppppppppppppppppppp"}
            ]))
            .unwrap();
            if let Some(packed) = pack(&raw) {
                assert!(
                    !packed.starts_with("[2]{"),
                    "unsafe key {bad:?} must not use CSV declaration: {packed}"
                );
                let original: Value = serde_json::from_str(&raw).unwrap();
                assert_eq!(unpack(&packed).unwrap(), original, "key {bad:?}");
            }
        }
    }

    #[test]
    fn empty_and_trivial_arrays_decline() {
        assert_eq!(pack("[]"), None);
        assert_eq!(pack("[{},{}]"), None, "zero columns is not a table");
    }

    #[test]
    fn packed_output_must_be_smaller_than_raw() {
        // Dotted key rules out CSV; the JSON `_cols` notation would be LARGER
        // than this tiny minified input, so pack must decline, not inflate.
        let raw = r#"[{"a.b":1},{"a.b":2}]"#;
        assert_eq!(pack(raw), None, "must never emit a larger encoding");
    }

    // ── realistic fixture: the actual target workload ──

    #[test]
    fn gh_pr_list_shape_saves_at_least_30_percent() {
        let items: Vec<Value> = (0..12)
            .map(|i| {
                json!({
                    "number": 4200 + i,
                    "title": format!("fix(scope-{i}): make the thing work properly again"),
                    "state": if i % 3 == 0 { "MERGED" } else { "OPEN" },
                    "author": {"is_bot": i % 4 == 0, "login": format!("user-{i}"), "name": format!("User Number {i}")},
                    "updatedAt": format!("2026-08-{:02}T10:0{}:00Z", 10 + i, i % 10),
                    "url": format!("https://github.com/rtk-ai/rtk/pull/{}", 4200 + i)
                })
            })
            .collect();
        let raw = serde_json::to_string(&Value::Array(items)).unwrap();
        let packed = pack_ok(&raw);
        let saved = 1.0 - (packed.len() as f64 / raw.len() as f64);
        assert!(
            saved >= 0.30,
            "expected ≥30% byte savings on gh pr list shape, got {:.1}% \nraw {} bytes → packed {} bytes\n{packed}",
            saved * 100.0,
            raw.len(),
            packed.len()
        );
        // The flattened author columns are where much of the win comes from.
        assert!(packed.lines().next().unwrap().contains("author.login"));
    }

    #[test]
    fn gh_api_commits_shape_round_trips() {
        // gh api repos/x/commits: deep objects, mixed nesting, dense arrays.
        let items: Vec<Value> = (0..6)
            .map(|i| {
                json!({
                    "sha": format!("{:040x}", 0xabcd00 + i),
                    "commit": {
                        "message": format!("commit message number {i}\n\nwith a body line"),
                        "author": {"name": "R B", "email": "r@example.com", "date": "2026-08-14T10:00:00Z"}
                    },
                    "parents": [{"sha": format!("{:040x}", 0xdead00 + i)}]
                })
            })
            .collect();
        let raw = serde_json::to_string_pretty(&Value::Array(items)).unwrap();
        pack_ok(&raw);
    }

    // ── unpack robustness (decoder alone, no encoder involved) ──

    #[test]
    fn unpack_rejects_garbage() {
        assert_eq!(unpack("not a packed document"), None);
        assert_eq!(unpack("[3]{broken"), None);
        assert_eq!(unpack("[2]{a:int}\n1"), None, "row count mismatch");
        assert_eq!(unpack("[1]{a:int}\n1\n2"), None, "extra rows");
        assert_eq!(unpack("[1]{a:int}\n1,2"), None, "extra cells");
        assert_eq!(unpack("[1]{a:unknowntype}\n1"), None, "bad type tag");
    }

    #[test]
    fn unpack_plain_json_passes_through() {
        // A packed envelope with zero replacements is just minified JSON.
        assert_eq!(unpack(r#"{"a":1}"#), Some(json!({"a": 1})));
    }
}
