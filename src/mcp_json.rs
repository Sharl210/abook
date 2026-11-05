//! Minimal dependency-free JSON value/parser used by the embedded MCP transport.
//!
//! This is deliberately small and strict: MCP messages are JSON-RPC documents,
//! not an excuse to pass a shell command through a string parser. Strings are
//! kept as UTF-8 Rust strings and are never reinterpreted as argv or shell code.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

impl Value {
    pub fn object() -> BTreeMap<String, Value> { BTreeMap::new() }
    pub fn s(v: impl Into<String>) -> Value { Value::String(v.into()) }
    pub fn b(v: bool) -> Value { Value::Bool(v) }
    pub fn n(v: impl Into<String>) -> Value { Value::Number(v.into()) }
    pub fn arr(v: Vec<Value>) -> Value { Value::Array(v) }
    pub fn obj(v: BTreeMap<String, Value>) -> Value { Value::Object(v) }

    pub fn get(&self, key: &str) -> Option<&Value> {
        match self { Value::Object(m) => m.get(key), _ => None }
    }
    pub fn str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(|v| match v { Value::String(s) => Some(s.as_str()), _ => None })
    }
    pub fn bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(|v| match v { Value::Bool(b) => Some(*b), _ => None })
    }
    pub fn array(&self, key: &str) -> Option<&[Value]> {
        self.get(key).and_then(|v| match v { Value::Array(a) => Some(a.as_slice()), _ => None })
    }
}

pub fn parse(input: &str) -> Result<Value, String> {
    let mut p = Parser { bytes: input.as_bytes(), at: 0, depth: 0 };
    let v = p.value()?;
    p.ws();
    if p.at != p.bytes.len() { return Err(format!("JSON 尾部有未解析内容（字节 {}）", p.at)); }
    Ok(v)
}

pub fn stringify(v: &Value) -> String {
    let mut out = String::new();
    write_value(v, &mut out);
    out
}

fn write_value(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(n),
        Value::String(s) => write_string(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() { if i > 0 { out.push(','); } write_value(x, out); }
            out.push(']');
        }
        Value::Object(m) => {
            out.push('{');
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 { out.push(','); }
                write_string(k, out); out.push(':'); write_value(x, out);
            }
            out.push('}');
        }
    }
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Parser<'a> { bytes: &'a [u8], at: usize, depth: usize }
impl<'a> Parser<'a> {
    fn ws(&mut self) { while self.at < self.bytes.len() && self.bytes[self.at].is_ascii_whitespace() { self.at += 1; } }
    fn value(&mut self) -> Result<Value, String> {
        self.ws();
        let b = *self.bytes.get(self.at).ok_or_else(|| "JSON 缺少值".to_string())?;
        match b {
            b'n' => self.word(b"null", Value::Null),
            b't' => self.word(b"true", Value::Bool(true)),
            b'f' => self.word(b"false", Value::Bool(false)),
            b'"' => Ok(Value::String(self.string()?)),
            b'[' => self.array(),
            b'{' => self.object(),
            b'-' | b'0'..=b'9' => self.number(),
            _ => Err(format!("JSON 在字节 {} 遇到非法值", self.at)),
        }
    }
    fn enter(&mut self) -> Result<(), String> {
        if self.depth >= 128 { return Err("JSON 嵌套层级超过 128".into()); }
        self.depth += 1; Ok(())
    }
    fn leave(&mut self) { self.depth = self.depth.saturating_sub(1); }
    fn word(&mut self, want: &[u8], v: Value) -> Result<Value, String> {
        if self.bytes.get(self.at..self.at + want.len()) == Some(want) { self.at += want.len(); Ok(v) }
        else { Err(format!("JSON 在字节 {} 遇到非法字面量", self.at)) }
    }
    fn string(&mut self) -> Result<String, String> {
        if self.bytes.get(self.at) != Some(&b'"') { return Err("JSON 字符串缺少开头引号".into()); }
        self.at += 1; let mut out = String::new();
        while self.at < self.bytes.len() {
            let b = self.bytes[self.at]; self.at += 1;
            match b {
                b'"' => return Ok(out),
                b'\\' => {
                    let e = *self.bytes.get(self.at).ok_or_else(|| "JSON 字符串转义不完整".to_string())?; self.at += 1;
                    match e {
                        b'"' => out.push('"'), b'\\' => out.push('\\'), b'/' => out.push('/'),
                        b'b' => out.push('\u{08}'), b'f' => out.push('\u{0c}'), b'n' => out.push('\n'),
                        b'r' => out.push('\r'), b't' => out.push('\t'),
                        b'u' => {
                            let first = self.hex4()?;
                            let scalar = if (0xd800..=0xdbff).contains(&first) {
                                if self.bytes.get(self.at..self.at + 2) != Some(b"\\u") {
                                    return Err("JSON 高位代理项后缺少低位代理项".into());
                                }
                                self.at += 2;
                                let second = self.hex4()?;
                                if !(0xdc00..=0xdfff).contains(&second) {
                                    return Err("JSON Unicode 代理对低位无效".into());
                                }
                                0x10000 + (((first as u32 - 0xd800) << 10) | (second as u32 - 0xdc00))
                            } else if (0xdc00..=0xdfff).contains(&first) {
                                return Err("JSON 出现孤立低位代理项".into());
                            } else {
                                first as u32
                            };
                            out.push(char::from_u32(scalar).ok_or_else(|| "JSON 字符串包含无效 Unicode".to_string())?);
                        }
                        _ => return Err(format!("JSON 字符串有未知转义 \\{}", e as char)),
                    }
                }
                0..=0x1f => return Err("JSON 字符串包含未转义控制字符".into()),
                _ => {
                    let start = self.at - 1;
                    let w = utf8_width(b).ok_or_else(|| "JSON 字符串 UTF-8 起始字节非法".to_string())?;
                    if w > 1 { while self.at < start + w { self.at += 1; } }
                    let s = std::str::from_utf8(&self.bytes[start..self.at]).map_err(|_| "JSON 字符串 UTF-8 非法".to_string())?;
                    out.push_str(s);
                }
            }
        }
        Err("JSON 字符串没有闭合引号".into())
    }
    fn hex4(&mut self) -> Result<u16, String> {
        if self.at + 4 > self.bytes.len() { return Err("JSON Unicode 转义不完整".into()); }
        let mut n = 0u16;
        for b in &self.bytes[self.at..self.at + 4] { n = (n << 4) | hex(*b).ok_or_else(|| "JSON Unicode 转义非法".to_string())? as u16; }
        self.at += 4; Ok(n)
    }
    fn array(&mut self) -> Result<Value, String> {
        self.enter()?;
        self.at += 1; let mut out = Vec::new(); self.ws();
        if self.bytes.get(self.at) == Some(&b']') { self.at += 1; self.leave(); return Ok(Value::Array(out)); }
        loop {
            out.push(self.value()?); self.ws();
            match self.bytes.get(self.at) {
                Some(b',') => { self.at += 1; }
                Some(b']') => { self.at += 1; self.leave(); return Ok(Value::Array(out)); }
                _ => { self.leave(); return Err("JSON 数组缺少逗号或闭合括号".into()); }
            }
        }
    }
    fn object(&mut self) -> Result<Value, String> {
        self.enter()?;
        self.at += 1; let mut out = BTreeMap::new(); self.ws();
        if self.bytes.get(self.at) == Some(&b'}') { self.at += 1; self.leave(); return Ok(Value::Object(out)); }
        loop {
            self.ws(); let k = self.string()?; self.ws();
            if self.bytes.get(self.at) != Some(&b':') { self.leave(); return Err("JSON 对象键后缺少冒号".into()); }
            self.at += 1; let v = self.value()?;
            if out.insert(k, v).is_some() { self.leave(); return Err("JSON 对象存在重复键".into()); }
            self.ws();
            match self.bytes.get(self.at) {
                Some(b',') => { self.at += 1; }
                Some(b'}') => { self.at += 1; self.leave(); return Ok(Value::Object(out)); }
                _ => { self.leave(); return Err("JSON 对象缺少逗号或闭合括号".into()); }
            }
        }
    }
    fn number(&mut self) -> Result<Value, String> {
        let start = self.at;
        if self.bytes.get(self.at) == Some(&b'-') { self.at += 1; }
        match self.bytes.get(self.at) {
            Some(b'0') => {
                self.at += 1;
                if self.bytes.get(self.at).map(|b| b.is_ascii_digit()).unwrap_or(false) {
                    return Err("JSON 数字不能有前导零".into());
                }
            }
            Some(b'1'..=b'9') => {
                self.at += 1;
                while self.bytes.get(self.at).map(|b| b.is_ascii_digit()).unwrap_or(false) { self.at += 1; }
            }
            _ => return Err("JSON 数字整数部分非法".into()),
        }
        if self.bytes.get(self.at) == Some(&b'.') {
            self.at += 1;
            let fraction_start = self.at;
            while self.bytes.get(self.at).map(|b| b.is_ascii_digit()).unwrap_or(false) { self.at += 1; }
            if self.at == fraction_start { return Err("JSON 小数点后必须有数字".into()); }
        }
        if matches!(self.bytes.get(self.at), Some(b'e') | Some(b'E')) {
            self.at += 1;
            if matches!(self.bytes.get(self.at), Some(b'+') | Some(b'-')) { self.at += 1; }
            let exp_start = self.at;
            while self.bytes.get(self.at).map(|b| b.is_ascii_digit()).unwrap_or(false) { self.at += 1; }
            if self.at == exp_start { return Err("JSON 指数部分必须有数字".into()); }
        }
        Ok(Value::Number(String::from_utf8(self.bytes[start..self.at].to_vec()).unwrap_or_default()))
    }
}
fn hex(b: u8) -> Option<u8> { match b { b'0'..=b'9' => Some(b - b'0'), b'a'..=b'f' => Some(b - b'a' + 10), b'A'..=b'F' => Some(b - b'A' + 10), _ => None } }
fn utf8_width(b: u8) -> Option<usize> { match b { 0..=0x7f => Some(1), 0xc2..=0xdf => Some(2), 0xe0..=0xef => Some(3), 0xf0..=0xf4 => Some(4), _ => None } }

#[cfg(test)]
mod tests {
    use super::{parse, stringify, Value};
    #[test]
    fn round_trip_multiline_shell_and_unicode() {
        let s = "第一行\n第二行 \" ' ` $() | <tag>\\\nemoji 😀";
        let input = format!("{{\"body\":{}}}", stringify(&Value::s(s)));
        let v = parse(&input).unwrap();
        assert_eq!(v.str("body"), Some(s));
        assert_eq!(parse(&stringify(&v)).unwrap(), v);
    }
    #[test]
    fn decodes_surrogate_pairs_and_rejects_lone_surrogates() {
        assert_eq!(parse(r#""\uD83D\uDE00""#).unwrap(), Value::s("😀"));
        assert!(parse(r#""\uD83D""#).is_err());
        assert!(parse(r#""\uDE00""#).is_err());
    }
    #[test]
    fn rejects_non_json_numbers_and_duplicate_keys() {
        for bad in ["01", "-01", "1.", ".1", "1e", "1e+", "--1"] { assert!(parse(bad).is_err(), "{bad}"); }
        assert!(parse(r#"{"a":1,"a":2}"#).is_err());
    }
    #[test]
    fn rejects_excessive_nesting_and_trailing_bytes() {
        let deep = format!("{}0{}", "[".repeat(130), "]".repeat(130));
        assert!(parse(&deep).is_err());
        assert!(parse("true false").is_err());
    }
}
