//! Bounded event JSON reader and a Json.NET-compatible `SelectToken` subset.
use super::{Fail, R, limits};

/// Ordered JSON value; numbers keep their source text.
#[derive(Clone, Debug)]
pub(super) enum J {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}
impl J {
    pub(super) fn get(&self, key: &str) -> Option<&J> {
        match self {
            J::Obj(fields) => fields.iter().find(|(name, _)| name == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub(super) fn is_integer(raw: &str) -> bool {
        !raw.contains(['.', 'e', 'E'])
    }
}

pub(super) enum Ev<'a> {
    StartObj,
    EndObj,
    StartArr,
    EndArr,
    Key(String),
    Str(String),
    Num(&'a str),
    Bool(bool),
    Null,
}

/// A syntax error is distinct from a policy failure raised by a sink.
pub(super) enum JErr {
    Syntax,
    Depth,
    /// Json.NET `new Name(...)` constructor syntax.
    Constructor,
    Fail(Fail),
}
impl From<Fail> for JErr {
    fn from(value: Fail) -> Self {
        JErr::Fail(value)
    }
}

/// `lenient` follows Json.NET's reader: comments, unquoted or single-quoted names
/// and single-quoted strings are accepted; constructors are reported.
struct Reader<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
    lenient: bool,
}
impl<'a> Reader<'a> {
    fn ws(&mut self) -> Result<(), JErr> {
        loop {
            while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\r' | b'\n') {
                self.i += 1;
            }
            if !self.lenient || self.peek() != Some(b'/') {
                return Ok(());
            }
            match self.s.get(self.i + 1) {
                Some(b'/') => {
                    while self.i < self.s.len() && !matches!(self.s[self.i], b'\r' | b'\n') {
                        self.i += 1;
                    }
                }
                Some(b'*') => {
                    let end = self.s[self.i + 2..].windows(2).position(|w| w == b"*/").ok_or(JErr::Syntax)?;
                    self.i += end + 4;
                }
                _ => return Err(JErr::Syntax),
            }
        }
    }
    fn key(&mut self) -> Result<String, JErr> {
        match self.peek() {
            Some(b'"') => self.string(b'"'),
            Some(b'\'') if self.lenient => self.string(b'\''),
            Some(byte) if self.lenient && (byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$')) => {
                let start = self.i;
                while self.peek().is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'$')) {
                    self.i += 1;
                }
                Ok(String::from_utf8_lossy(&self.s[start..self.i]).into_owned())
            }
            _ => Err(JErr::Syntax),
        }
    }
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn value(&mut self, sink: &mut dyn FnMut(Ev) -> Result<(), JErr>) -> Result<(), JErr> {
        self.ws()?;
        match self.peek().ok_or(JErr::Syntax)? {
            b'{' => {
                self.enter()?;
                self.i += 1;
                sink(Ev::StartObj)?;
                self.ws()?;
                if self.peek() == Some(b'}') {
                    self.i += 1;
                } else {
                    loop {
                        self.ws()?;
                        let key = self.key()?;
                        sink(Ev::Key(key))?;
                        self.ws()?;
                        if self.peek() != Some(b':') {
                            return Err(JErr::Syntax);
                        }
                        self.i += 1;
                        self.value(sink)?;
                        self.ws()?;
                        match self.peek() {
                            Some(b',') => self.i += 1,
                            Some(b'}') => {
                                self.i += 1;
                                break;
                            }
                            _ => return Err(JErr::Syntax),
                        }
                    }
                }
                self.depth -= 1;
                sink(Ev::EndObj)
            }
            b'[' => {
                self.enter()?;
                self.i += 1;
                sink(Ev::StartArr)?;
                self.ws()?;
                if self.peek() == Some(b']') {
                    self.i += 1;
                } else {
                    loop {
                        self.value(sink)?;
                        self.ws()?;
                        match self.peek() {
                            Some(b',') => self.i += 1,
                            Some(b']') => {
                                self.i += 1;
                                break;
                            }
                            _ => return Err(JErr::Syntax),
                        }
                    }
                }
                self.depth -= 1;
                sink(Ev::EndArr)
            }
            quote @ (b'"' | b'\'') => {
                if quote == b'\'' && !self.lenient {
                    return Err(JErr::Syntax);
                }
                let text = self.string(quote)?;
                sink(Ev::Str(text))
            }
            b't' => self.word(b"true").and_then(|_| sink(Ev::Bool(true))),
            b'f' => self.word(b"false").and_then(|_| sink(Ev::Bool(false))),
            b'n' if self.lenient
                && self.s[self.i..].starts_with(b"new")
                && self.s.get(self.i + 3).is_some_and(|b| b.is_ascii_whitespace()) =>
            {
                Err(JErr::Constructor)
            }
            b'n' => self.word(b"null").and_then(|_| sink(Ev::Null)),
            b'-' | b'0'..=b'9' => {
                let start = self.i;
                if self.peek() == Some(b'-') {
                    self.i += 1;
                }
                let digits = self.digits();
                if digits == 0 {
                    return Err(JErr::Syntax);
                }
                if self.peek() == Some(b'.') {
                    self.i += 1;
                    if self.digits() == 0 {
                        return Err(JErr::Syntax);
                    }
                }
                if matches!(self.peek(), Some(b'e' | b'E')) {
                    self.i += 1;
                    if matches!(self.peek(), Some(b'+' | b'-')) {
                        self.i += 1;
                    }
                    if self.digits() == 0 {
                        return Err(JErr::Syntax);
                    }
                }
                let raw = std::str::from_utf8(&self.s[start..self.i]).map_err(|_| JErr::Syntax)?;
                sink(Ev::Num(raw))
            }
            _ => Err(JErr::Syntax),
        }
    }
    fn enter(&mut self) -> Result<(), JErr> {
        self.depth += 1;
        if self.depth > 64 {
            return Err(JErr::Depth);
        }
        Ok(())
    }
    fn digits(&mut self) -> usize {
        let start = self.i;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.i += 1;
        }
        self.i - start
    }
    fn word(&mut self, word: &[u8]) -> Result<(), JErr> {
        if self.s[self.i..].starts_with(word) {
            self.i += word.len();
            Ok(())
        } else {
            Err(JErr::Syntax)
        }
    }
    fn hex4(&mut self) -> Result<u16, JErr> {
        let text = self.s.get(self.i..self.i + 4).ok_or(JErr::Syntax)?;
        let text = std::str::from_utf8(text).map_err(|_| JErr::Syntax)?;
        self.i += 4;
        u16::from_str_radix(text, 16).map_err(|_| JErr::Syntax)
    }
    fn string(&mut self, quote: u8) -> Result<String, JErr> {
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let byte = self.peek().ok_or(JErr::Syntax)?;
            self.i += 1;
            match byte {
                _ if byte == quote => break,
                b'\\' => {
                    let escape = self.peek().ok_or(JErr::Syntax)?;
                    self.i += 1;
                    let ch = match escape {
                        b'"' => '"',
                        b'\'' => '\'',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let high = self.hex4()?;
                            if (0xD800..0xDC00).contains(&high) && self.s[self.i..].starts_with(b"\\u") {
                                let save = self.i;
                                self.i += 2;
                                let low = self.hex4()?;
                                if (0xDC00..0xE000).contains(&low) {
                                    let code = 0x10000 + (((high as u32) - 0xD800) << 10) + (low as u32 - 0xDC00);
                                    char::from_u32(code).unwrap_or('\u{FFFD}')
                                } else {
                                    self.i = save;
                                    '\u{FFFD}'
                                }
                            } else {
                                char::from_u32(high as u32).unwrap_or('\u{FFFD}')
                            }
                        }
                        _ => return Err(JErr::Syntax),
                    };
                    let mut buffer = [0; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
                }
                0..=0x1f => return Err(JErr::Syntax),
                other => out.push(other),
            }
        }
        String::from_utf8(out).map_err(|_| JErr::Syntax)
    }
}

/// Stream events for a complete document; trailing non-whitespace is a syntax error.
pub(super) fn events(
    text: &str,
    lenient: bool,
    sink: &mut dyn FnMut(Ev) -> Result<(), JErr>,
) -> Result<(), JErr> {
    let mut reader = Reader { s: text.as_bytes(), i: 0, depth: 0, lenient };
    reader.value(sink)?;
    reader.ws()?;
    if reader.i != reader.s.len() {
        return Err(JErr::Syntax);
    }
    Ok(())
}

/// Parse into an ordered tree. `strict` is the request/profile reader: standard JSON
/// with duplicate properties rejected. Otherwise Json.NET leniency applies and the last
/// duplicate wins, as `JToken.ReadFrom` does.
pub(super) fn parse(text: &str, strict: bool) -> R<J> {
    let mut stack: Vec<(J, Option<String>)> = Vec::new();
    let mut root = None;
    let mut key: Option<String> = None;
    let result = events(text, !strict, &mut |event| {
        let value = match event {
            Ev::StartObj => {
                stack.push((J::Obj(Vec::new()), key.take()));
                return Ok(());
            }
            Ev::StartArr => {
                stack.push((J::Arr(Vec::new()), key.take()));
                return Ok(());
            }
            Ev::Key(name) => {
                key = Some(name);
                return Ok(());
            }
            Ev::EndObj | Ev::EndArr => {
                let (value, name) = stack.pop().ok_or(JErr::Syntax)?;
                key = name;
                value
            }
            Ev::Str(text) => J::Str(text),
            Ev::Num(raw) => J::Num(raw.to_owned()),
            Ev::Bool(value) => J::Bool(value),
            Ev::Null => J::Null,
        };
        match stack.last_mut() {
            None => root = Some(value),
            Some((J::Arr(items), _)) => items.push(value),
            Some((J::Obj(fields), _)) => {
                let name = key.take().ok_or(JErr::Syntax)?;
                if let Some(slot) = fields.iter_mut().find(|(existing, _)| *existing == name) {
                    if strict {
                        return Err(JErr::Fail(Fail::Code("invalid_expression_or_request")));
                    }
                    slot.1 = value;
                } else {
                    fields.push((name, value));
                }
            }
            Some(_) => return Err(JErr::Syntax),
        }
        Ok(())
    });
    match result {
        Ok(()) => root.ok_or(Fail::Code("invalid_expression_or_request")),
        Err(JErr::Fail(fail)) => Err(fail),
        Err(_) => Err(Fail::Code("invalid_expression_or_request")),
    }
}

/// .NET `double.ToString(InvariantCulture)`: shortest round-trip, E notation outside 1e-5..1e15.
pub(super) fn dotnet_double(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "∞".into() } else { "-∞".into() };
    }
    let magnitude = value.abs();
    if magnitude != 0.0 && !(1e-5..1e15).contains(&magnitude) {
        let text = format!("{value:E}");
        let (mantissa, exponent) = text.split_once('E').unwrap_or((&text, "0"));
        let (sign, digits) = exponent.strip_prefix('-').map_or(("+", exponent), |rest| ("-", rest));
        return format!("{mantissa}E{sign}{digits:0>2}");
    }
    let text = format!("{value}");
    if text == "-0" { "-0".into() } else { text }
}

/// Json.NET explicit `(string)` conversion of a selected value.
pub(super) fn token_string(value: &J) -> R<String> {
    Ok(match value {
        J::Null => String::new(),
        J::Str(text) => text.clone(),
        J::Bool(true) => "True".into(),
        J::Bool(false) => "False".into(),
        J::Num(raw) if J::is_integer(raw) => {
            let trimmed = raw.trim_start_matches('-').trim_start_matches('0');
            if trimmed.is_empty() {
                "0".into()
            } else if raw.starts_with('-') {
                format!("-{trimmed}")
            } else {
                trimmed.to_owned()
            }
        }
        J::Num(raw) => dotnet_double(raw.parse().map_err(|_| Fail::invalid())?),
        J::Arr(_) | J::Obj(_) => return Err(Fail::invalid()),
    })
}

// ---- JSONPath (Json.NET JPath semantics, SelectToken single-result) ----

#[derive(Debug)]
enum Seg {
    Name(String),
    Wildcard,
    Indexes(Vec<i64>),
    Names(Vec<String>),
    Slice(Option<i64>, Option<i64>, Option<i64>),
    Filter(Box<Expr>),
    Scan(Box<Seg>),
}
#[derive(Debug)]
enum Operand {
    Path(bool, Vec<Seg>),
    Value(J),
    Pattern(String, String),
}
#[derive(Debug)]
enum Expr {
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Exists(Operand),
    Compare(Operand, &'static str, Operand),
}

struct PathParser<'a> {
    s: &'a [char],
    i: usize,
}
impl PathParser<'_> {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }
    fn ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.i += 1;
        }
    }
    fn eat(&mut self, ch: char) -> bool {
        if self.peek() == Some(ch) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn starts(&self, text: &str) -> bool {
        text.chars().enumerate().all(|(n, ch)| self.s.get(self.i + n) == Some(&ch))
    }
    /// Segments until end, or until a filter terminator when `nested`.
    fn segments(&mut self, nested: bool) -> R<Vec<Seg>> {
        let mut out = Vec::new();
        let mut first = true;
        while let Some(ch) = self.peek() {
            if nested && (ch.is_whitespace() || matches!(ch, ')' | '=' | '!' | '<' | '>' | '&' | '|')) {
                break;
            }
            match ch {
                '.' => {
                    self.i += 1;
                    let scan = self.eat('.');
                    let seg = if self.peek() == Some('[') {
                        self.bracket()?
                    } else if self.eat('*') {
                        Seg::Wildcard
                    } else {
                        Seg::Name(self.name(nested)?)
                    };
                    out.push(if scan { Seg::Scan(Box::new(seg)) } else { seg });
                }
                '[' => out.push(self.bracket()?),
                _ if first && !nested => out.push(Seg::Name(self.name(nested)?)),
                _ => return Err(Fail::invalid()),
            }
            first = false;
        }
        Ok(out)
    }
    fn name(&mut self, nested: bool) -> R<String> {
        let start = self.i;
        while let Some(ch) = self.peek() {
            if ch == '.' || ch == '[' || ch == ']' || ch == '(' || ch == ')' {
                break;
            }
            if nested && (ch.is_whitespace() || matches!(ch, '=' | '!' | '<' | '>' | '&' | '|')) {
                break;
            }
            self.i += 1;
        }
        if self.i == start {
            return Err(Fail::invalid());
        }
        Ok(self.s[start..self.i].iter().collect())
    }
    fn quoted(&mut self) -> R<String> {
        let quote = self.peek().ok_or_else(Fail::invalid)?;
        self.i += 1;
        let mut out = String::new();
        loop {
            let ch = self.peek().ok_or_else(Fail::invalid)?;
            self.i += 1;
            if ch == quote {
                return Ok(out);
            }
            if ch == '\\' {
                let escaped = self.peek().ok_or_else(Fail::invalid)?;
                self.i += 1;
                out.push(match escaped {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    other => other,
                });
            } else {
                out.push(ch);
            }
        }
    }
    fn integer(&mut self) -> R<Option<i64>> {
        self.ws();
        let start = self.i;
        if self.peek() == Some('-') {
            self.i += 1;
        }
        while self.peek().is_some_and(|ch| ch.is_ascii_digit()) {
            self.i += 1;
        }
        if self.i == start {
            return Ok(None);
        }
        let text: String = self.s[start..self.i].iter().collect();
        self.ws();
        text.parse().map(Some).map_err(|_| Fail::invalid())
    }
    fn bracket(&mut self) -> R<Seg> {
        self.i += 1;
        self.ws();
        let seg = match self.peek() {
            Some('*') => {
                self.i += 1;
                self.ws();
                Seg::Wildcard
            }
            Some('\'' | '"') => {
                let mut names = vec![self.quoted()?];
                self.ws();
                while self.eat(',') {
                    self.ws();
                    names.push(self.quoted()?);
                    self.ws();
                }
                if names.len() == 1 { Seg::Name(names.pop().unwrap()) } else { Seg::Names(names) }
            }
            Some('?') => {
                self.i += 1;
                self.ws();
                if !self.eat('(') {
                    return Err(Fail::invalid());
                }
                let expr = self.or()?;
                self.ws();
                if !self.eat(')') {
                    return Err(Fail::invalid());
                }
                self.ws();
                Seg::Filter(Box::new(expr))
            }
            _ => {
                let first = self.integer()?;
                if self.eat(':') {
                    let end = self.integer()?;
                    let step = if self.eat(':') { self.integer()? } else { None };
                    Seg::Slice(first, end, step)
                } else {
                    let mut items = vec![first.ok_or_else(Fail::invalid)?];
                    while self.eat(',') {
                        items.push(self.integer()?.ok_or_else(Fail::invalid)?);
                    }
                    Seg::Indexes(items)
                }
            }
        };
        if !self.eat(']') {
            return Err(Fail::invalid());
        }
        Ok(seg)
    }
    fn or(&mut self) -> R<Expr> {
        let mut left = self.and()?;
        loop {
            self.ws();
            if self.starts("||") {
                self.i += 2;
                left = Expr::Or(Box::new(left), Box::new(self.and()?));
            } else {
                return Ok(left);
            }
        }
    }
    fn and(&mut self) -> R<Expr> {
        let mut left = self.atom()?;
        loop {
            self.ws();
            if self.starts("&&") {
                self.i += 2;
                left = Expr::And(Box::new(left), Box::new(self.atom()?));
            } else {
                return Ok(left);
            }
        }
    }
    fn atom(&mut self) -> R<Expr> {
        self.ws();
        if self.eat('(') {
            let inner = self.or()?;
            self.ws();
            if !self.eat(')') {
                return Err(Fail::invalid());
            }
            return Ok(inner);
        }
        let left = self.operand()?;
        self.ws();
        for op in ["==", "!=", "<=", ">=", "=~", "<", ">"] {
            if self.starts(op) {
                self.i += op.len();
                let right = self.operand()?;
                return Ok(Expr::Compare(left, op, right));
            }
        }
        Ok(Expr::Exists(left))
    }
    fn operand(&mut self) -> R<Operand> {
        self.ws();
        match self.peek().ok_or_else(Fail::invalid)? {
            '@' | '$' => {
                let root = self.peek() == Some('$');
                self.i += 1;
                Ok(Operand::Path(root, self.segments(true)?))
            }
            '\'' | '"' => Ok(Operand::Value(J::Str(self.quoted()?))),
            '/' => {
                self.i += 1;
                let mut pattern = String::new();
                loop {
                    let ch = self.peek().ok_or_else(Fail::invalid)?;
                    self.i += 1;
                    if ch == '/' {
                        break;
                    }
                    if ch == '\\' && self.peek() == Some('/') {
                        self.i += 1;
                        pattern.push('/');
                        continue;
                    }
                    pattern.push(ch);
                }
                let mut flags = String::new();
                while let Some(ch) = self.peek().filter(char::is_ascii_alphabetic) {
                    flags.push(ch);
                    self.i += 1;
                }
                Ok(Operand::Pattern(pattern, flags))
            }
            _ if self.starts("true") => {
                self.i += 4;
                Ok(Operand::Value(J::Bool(true)))
            }
            _ if self.starts("false") => {
                self.i += 5;
                Ok(Operand::Value(J::Bool(false)))
            }
            _ if self.starts("null") => {
                self.i += 4;
                Ok(Operand::Value(J::Null))
            }
            _ => {
                let start = self.i;
                while self.peek().is_some_and(|ch| ch.is_ascii_digit() || matches!(ch, '-' | '+' | '.' | 'e' | 'E')) {
                    self.i += 1;
                }
                let raw: String = self.s[start..self.i].iter().collect();
                raw.parse::<f64>().map_err(|_| Fail::invalid())?;
                Ok(Operand::Value(J::Num(raw)))
            }
        }
    }
}

fn apply<'a>(seg: &Seg, current: &'a J, root: &'a J, out: &mut Vec<&'a J>) -> R<()> {
    match seg {
        Seg::Name(name) => out.extend(current.get(name)),
        Seg::Names(names) => {
            for name in names {
                out.extend(current.get(name));
            }
        }
        Seg::Wildcard => match current {
            J::Arr(items) => out.extend(items),
            J::Obj(fields) => out.extend(fields.iter().map(|(_, v)| v)),
            _ => {}
        },
        Seg::Indexes(indexes) => {
            if let J::Arr(items) = current {
                for index in indexes {
                    let at = if *index < 0 { items.len() as i64 + index } else { *index };
                    if at >= 0 {
                        out.extend(items.get(at as usize));
                    }
                }
            }
        }
        Seg::Slice(start, end, step) => {
            if let J::Arr(items) = current {
                let len = items.len() as i64;
                let step = step.unwrap_or(1);
                if step == 0 {
                    return Err(Fail::invalid());
                }
                let norm = |v: i64| if v < 0 { (len + v).max(0) } else { v.min(len) };
                if step > 0 {
                    let (mut i, stop) = (norm(start.unwrap_or(0)), norm(end.unwrap_or(len)));
                    while i < stop {
                        out.push(&items[i as usize]);
                        i += step;
                    }
                } else {
                    let mut i = start.map_or(len - 1, |v| if v < 0 { len + v } else { v.min(len - 1) });
                    let stop = end.map_or(-1, |v| if v < 0 { (len + v).max(-1) } else { v });
                    while i > stop && i >= 0 {
                        out.push(&items[i as usize]);
                        i += step;
                    }
                }
            }
        }
        Seg::Filter(expr) => {
            let candidates: Vec<&J> = match current {
                J::Arr(items) => items.iter().collect(),
                J::Obj(fields) => fields.iter().map(|(_, v)| v).collect(),
                _ => Vec::new(),
            };
            for candidate in candidates {
                if eval(expr, candidate, root)? {
                    out.push(candidate);
                }
            }
        }
        Seg::Scan(inner) => {
            fn walk<'a>(value: &'a J, all: &mut Vec<&'a J>) {
                match value {
                    J::Arr(items) => items.iter().for_each(|item| {
                        all.push(item);
                        walk(item, all)
                    }),
                    J::Obj(fields) => fields.iter().for_each(|(_, item)| {
                        all.push(item);
                        walk(item, all)
                    }),
                    _ => {}
                }
            }
            match inner.as_ref() {
                Seg::Wildcard => walk(current, out),
                Seg::Name(name) => {
                    fn scan<'a>(value: &'a J, name: &str, out: &mut Vec<&'a J>) {
                        match value {
                            J::Arr(items) => items.iter().for_each(|item| scan(item, name, out)),
                            J::Obj(fields) => {
                                for (key, item) in fields {
                                    if key == name {
                                        out.push(item);
                                    }
                                    scan(item, name, out);
                                }
                            }
                            _ => {}
                        }
                    }
                    scan(current, name, out);
                }
                other => {
                    let mut all = vec![current];
                    walk(current, &mut all);
                    for value in all {
                        apply(other, value, root, out)?;
                    }
                }
            }
        }
    }
    limits::require(out.len() <= 65536, "json_path_limit")
}

fn select<'a>(segs: &[Seg], start: &'a J, root: &'a J) -> R<Vec<&'a J>> {
    let mut current = vec![start];
    for seg in segs {
        let mut next = Vec::new();
        for value in current {
            apply(seg, value, root, &mut next)?;
        }
        current = next;
    }
    Ok(current)
}

fn number(value: &J) -> Option<f64> {
    match value {
        J::Num(raw) => raw.parse().ok(),
        J::Str(text) => text.trim().parse().ok(),
        _ => None,
    }
}
fn compare(left: &J, op: &str, right: &J) -> bool {
    use std::cmp::Ordering;
    let ordering = match (left, right) {
        (J::Null, J::Null) => Some(Ordering::Equal),
        (J::Bool(a), J::Bool(b)) => Some(a.cmp(b)),
        (J::Str(a), J::Str(b)) => Some(a.cmp(b)),
        (J::Num(_), _) | (_, J::Num(_)) => match (number(left), number(right)) {
            (Some(a), Some(b)) => a.partial_cmp(&b),
            _ => None,
        },
        _ => None,
    };
    match (op, ordering) {
        ("==", Some(Ordering::Equal)) => true,
        ("!=", ordering) => ordering != Some(Ordering::Equal),
        ("<", Some(Ordering::Less)) => true,
        ("<=", Some(Ordering::Less | Ordering::Equal)) => true,
        (">", Some(Ordering::Greater)) => true,
        (">=", Some(Ordering::Greater | Ordering::Equal)) => true,
        _ => false,
    }
}
fn eval(expr: &Expr, current: &J, root: &J) -> R<bool> {
    let resolve = |operand: &Operand| -> R<Vec<J>> {
        Ok(match operand {
            Operand::Path(is_root, segs) => {
                select(segs, if *is_root { root } else { current }, root)?.into_iter().cloned().collect()
            }
            Operand::Value(value) => vec![value.clone()],
            Operand::Pattern(..) => Vec::new(),
        })
    };
    Ok(match expr {
        Expr::And(a, b) => eval(a, current, root)? && eval(b, current, root)?,
        Expr::Or(a, b) => eval(a, current, root)? || eval(b, current, root)?,
        Expr::Exists(operand) => !resolve(operand)?.is_empty(),
        Expr::Compare(left, "=~", Operand::Pattern(pattern, flags)) => {
            let mut prefix = String::new();
            for flag in flags.chars() {
                match flag {
                    'i' | 'm' | 's' | 'x' => prefix.push(flag),
                    _ => return Err(Fail::invalid()),
                }
            }
            let source = if prefix.is_empty() { pattern.clone() } else { format!("(?{prefix}){pattern}") };
            let regex = super::regex(&source)?;
            let mut matched = false;
            for value in resolve(left)? {
                if let J::Str(text) = value
                    && super::regex_find(&regex, &text)?.is_some()
                {
                    matched = true;
                    break;
                }
            }
            matched
        }
        Expr::Compare(left, op, right) => {
            let (left, right) = (resolve(left)?, resolve(right)?);
            left.iter().any(|a| right.iter().any(|b| compare(a, op, b)))
        }
    })
}

/// Json.NET `SelectToken(path)` with errorWhenNoMatch=false; several results are an error.
pub(super) fn select_token(document: &J, path: &str) -> R<Option<J>> {
    let chars: Vec<char> = path.chars().collect();
    let mut parser = PathParser { s: &chars, i: 0 };
    if parser.peek() == Some('$') {
        parser.i += 1;
    }
    let segs = parser.segments(false)?;
    let found = select(&segs, document, document)?;
    limits::require(found.len() <= 1, "invalid_expression_or_request")?;
    Ok(found.first().map(|value| (*value).clone()))
}
