//! Hand-written lexer.
//!
//! Tokens are produced in one left-to-right pass over the source bytes. Two
//! things are worth calling out because they drive the rest of the engine:
//!
//! * **Template raw fragments.** `` `a${x}b` `` is lexed into a single
//!   `Tok::Template` whose interpolations are *raw source slices*, which the
//!   parser re-enters recursively. That avoids a template-aware parser/lexer
//!   feedback loop (the usual reason hand-written ES lexers either duplicate the
//!   parser or resort to push-back hacks).
//! * **Regex/division disambiguation** is decided from the previous significant
//!   token, which is exactly what the ECMAScript grammar's lexical goal symbol
//!   encodes.

use crate::ast::TplPart;

#[derive(Clone, Debug)]
pub enum Tok {
    Num(f64),
    BigInt(i64),
    Str(String),
    Str16(Vec<u16>),
    Template(Vec<TplPart>),
    Ident(String),
    Private(String),
    Regex(String, String),
    Punct(&'static str),
    Keyword(&'static str),
    Eof,
}

impl Tok {
    pub fn describe(&self) -> String {
        match self {
            Tok::Num(n) => format!("number {}", n),
            Tok::BigInt(n) => format!("bigint {}n", n),
            Tok::Str(s) => format!("string {:?}", s),
            Tok::Str16(s) => format!("UTF-16 string {:?}", s),
            Tok::Template(_) => "template literal".into(),
            Tok::Ident(s) => format!("identifier `{}`", s),
            Tok::Private(s) => format!("private name `#{}`", s),
            Tok::Regex(a, b) => format!("regex /{}/{}", a, b),
            Tok::Punct(p) => format!("`{}`", p),
            Tok::Keyword(k) => format!("keyword `{}`", k),
            Tok::Eof => "end of input".into(),
        }
    }
}

const PUNCTS: &[&str] = &[
    ">>>=", "...", "===", "!==", "**=", "<<=", ">>=", ">>>", "&&=", "||=", "??=", "?.", "??",
    "=>", "==", "!=", "<=", ">=", "&&", "||", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=",
    "|=", "^=", "**", "<<", ">>", "{", "}", "(", ")", "[", "]", ";", ",", "<", ">", "+", "-",
    "*", "/", "%", "&", "|", "^", "!", "~", "?", ":", "=", ".",
];

const KEYWORDS: &[&str] = &[
    "var", "let", "const", "function", "return", "if", "else", "for", "while", "do", "break",
    "continue", "new", "delete", "typeof", "instanceof", "in", "of", "void", "throw", "try",
    "catch", "finally", "switch", "case", "default", "class", "extends", "super", "this", "yield",
    "await", "async", "get", "set", "static", "null", "true", "false", "undefined",
    "import", "export", "as", "from",
];

/// A comment, positioned by the token index it precedes so the parser can
/// splice it into the statement stream exactly where it appeared.
#[derive(Clone, Debug)]
pub struct LexComment {
    pub text: String,
    /// Index of the next token after this comment.
    pub next_tok: usize,
    /// Source line the comment started on.
    pub line: u32,
}

pub struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    line: u32,
    pub toks: Vec<(Tok, u32)>,
    pub comments: Vec<LexComment>,
    prev_significant: Option<Tok>,
}

pub fn tokenize(src: &str) -> Result<Vec<(Tok, u32)>, String> {
    tokenize_with_comments(src).map(|(t, _)| t)
}

/// Tokenize, keeping comments so they can be preserved through the AST.
pub fn tokenize_with_comments(src: &str) -> Result<(Vec<(Tok, u32)>, Vec<LexComment>), String> {
    let mut lx = Lexer::new(src);
    lx.run()?;
    Ok((lx.toks, lx.comments))
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        let bytes = src.as_bytes();
        // Strip a UTF-8 BOM before lexing. Windows-authored files routinely
        // carry one, and without this the very first token becomes a syntax
        // error at position 0 — in `jhol run` *and* `jhol fmt` — while node and
        // every other runtime accept the same file.
        let start = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) { 3 } else { 0 };
        Lexer {
            src: &bytes[start..],
            pos: 0,
            line: 1,
            toks: Vec::new(),
            comments: Vec::new(),
            prev_significant: None,
        }
    }

    fn err(&self, msg: &str) -> String {
        format!("line {}: {}", self.line, msg)
    }

    fn peek(&self) -> u8 {
        if self.pos < self.src.len() {
            self.src[self.pos]
        } else {
            0
        }
    }
    fn peek_at(&self, n: usize) -> u8 {
        if self.pos + n < self.src.len() {
            self.src[self.pos + n]
        } else {
            0
        }
    }

    fn bump(&mut self) -> u8 {
        let c = self.peek();
        self.pos += 1;
        if c == b'\n' {
            self.line += 1;
        }
        c
    }

    /// True when `/` should be read as a division operator.
    fn regex_allowed(&self) -> bool {
        match &self.prev_significant {
            None => true,
            Some(Tok::Num(_)) | Some(Tok::BigInt(_)) | Some(Tok::Str(_)) | Some(Tok::Str16(_)) => false,
            Some(Tok::Template(_)) | Some(Tok::Regex(_, _)) => false,
            Some(Tok::Private(_)) => false,
            Some(Tok::Punct(p)) => !matches!(*p, ")" | "]" | "}"),
            Some(Tok::Keyword(k)) => !matches!(*k, "this" | "super" | "true" | "false" | "null" | "undefined"),
            Some(Tok::Ident(_)) => false,
            Some(Tok::Eof) => true,
        }
    }

    pub fn run(&mut self) -> Result<(), String> {
        loop {
            self.skip_trivia()?;
            if self.pos >= self.src.len() {
                self.push(Tok::Eof);
                break;
            }
            let t = self.next_token()?;
            self.push(t);
        }
        Ok(())
    }

    fn push(&mut self, t: Tok) {
        let significant = !matches!(t, Tok::Eof);
        if significant {
            self.prev_significant = Some(t.clone());
        }
        self.toks.push((t, self.line));
    }

    fn skip_trivia(&mut self) -> Result<(), String> {
        loop {
            let c = self.peek();
            if c == b'/' && self.peek_at(1) == b'/' {
                let (start, line) = (self.pos, self.line);
                while self.pos < self.src.len() && self.peek() != b'\n' {
                    self.pos += 1;
                }
                let text = String::from_utf8_lossy(&self.src[start..self.pos])
                    .trim_end()
                    .to_string();
                self.comments.push(LexComment {
                    text,
                    next_tok: self.toks.len(),
                    line,
                });
            } else if c == b'/' && self.peek_at(1) == b'*' {
                let (start, line) = (self.pos, self.line);
                self.pos += 2;
                let mut closed = false;
                while self.pos < self.src.len() {
                    if self.peek() == b'*' && self.peek_at(1) == b'/' {
                        self.pos += 2;
                        closed = true;
                        break;
                    }
                    self.bump();
                }
                if !closed {
                    return Err(self.err("unterminated block comment"));
                }
                let text = String::from_utf8_lossy(&self.src[start..self.pos])
                    .to_string();
                self.comments.push(LexComment {
                    text,
                    next_tok: self.toks.len(),
                    line,
                });
            } else if c.is_ascii_whitespace() {
                self.bump();
            } else if c == b'#' && self.pos == 0 && self.src.starts_with(b"#!") {
                while self.pos < self.src.len() && self.peek() != b'\n' {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
        Ok(())
    }

    fn next_token(&mut self) -> Result<Tok, String> {
        let c = self.peek();

        if c == b'"' || c == b'\'' {
            return self.read_string(c);
        }
        if c == b'`' {
            return self.read_template();
        }
        if c == b'/' && self.regex_allowed() {
            return self.read_regex();
        }
        if c == b'#' && is_ident_start(self.peek_at(1)) {
            self.pos += 1;
            let s = self.read_ident_text();
            return Ok(Tok::Private(s));
        }
        if is_ident_start(c) {
            let s = self.read_ident_text();
            if let Some(kw) = KEYWORDS.iter().find(|k| **k == s.as_str()) {
                return Ok(Tok::Keyword(kw));
            }
            return Ok(Tok::Ident(s));
        }
        if c.is_ascii_digit() || (c == b'.' && self.peek_at(1).is_ascii_digit()) {
            return self.read_number();
        }
        self.read_punct()
    }

    fn read_ident_text(&mut self) -> String {
        let start = self.pos;
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            if is_ident_part(c) {
                self.pos += 1;
            } else {
                break;
            }
        }
        String::from_utf8_lossy(&self.src[start..self.pos]).into_owned()
    }

    fn read_punct(&mut self) -> Result<Tok, String> {
        for p in PUNCTS {
            let n = p.len();
            if self.pos + n <= self.src.len() && &self.src[self.pos..self.pos + n] == p.as_bytes() {
                if *p == "?." && self.peek_at(2).is_ascii_digit() {
                    // `a?.3: b` is a ternary, not optional chaining.
                    continue;
                }
                self.pos += n;
                return Ok(Tok::Punct(p));
            }
        }
        let c = self.peek();
        self.pos += 1;
        Err(self.err(&format!("unexpected character `{}`", c as char)))
    }

    /// Decode a verbatim byte run as UTF-8 and append it.
    ///
    /// String and template contents are accumulated as byte runs rather than
    /// one `char` per byte: `b as char` maps each byte through Latin-1, which
    /// turned `"héllo"` into `"hÃ©llo"` and corrupted every non-ASCII literal
    /// in both `jhol run` and `jhol fmt`. The whole source is validated as
    /// UTF-8 before it reaches the lexer, so this decodes exactly; lossy keeps
    /// a stray invalid byte from panicking.
    fn flush_run(&self, out: &mut String, from: usize, to: usize) {
        if from >= to || to > self.src.len() {
            return;
        }
        out.push_str(&String::from_utf8_lossy(&self.src[from..to]));
    }

    fn read_string(&mut self, quote: u8) -> Result<Tok, String> {
        self.pos += 1;
        let mut units = Vec::new();
        let mut run = self.pos;
        loop {
            if self.pos >= self.src.len() { return Err(self.err("unterminated string literal")); }
            let c = self.bump();
            if c == quote || c == b'\\' {
                units.extend(String::from_utf8_lossy(&self.src[run..self.pos-1]).encode_utf16());
                if c == quote { break; }
                if self.peek() == b'u' {
                    self.bump();
                    let n = if self.peek() == b'{' {
                        self.bump(); let start=self.pos;
                        while self.pos < self.src.len() && self.peek()!=b'}' { self.bump(); }
                        if self.pos == self.src.len() { return Err(self.err("unterminated Unicode escape")); }
                        let n=u32::from_str_radix(std::str::from_utf8(&self.src[start..self.pos]).unwrap_or(""),16)
                            .map_err(|_|self.err("bad Unicode escape"))?;
                        self.bump(); n
                    } else { self.take_hex(4)? };
                    if n > 0x10ffff { return Err(self.err("invalid Unicode code point")); }
                    if n <= 65535 { units.push(n as u16); }
                    else { let n=n-65536; units.extend([0xd800+(n>>10) as u16,0xdc00+(n&1023) as u16]); }
                } else {
                    let mut escaped=String::new(); self.read_escape(&mut escaped)?;
                    units.extend(escaped.encode_utf16());
                }
                run=self.pos;
            } else if c == b'\n' { return Err(self.err("unterminated string literal")); }
        }
        Ok(match String::from_utf16(&units) { Ok(s)=>Tok::Str(s),Err(_)=>Tok::Str16(units) })
    }

    fn read_escape(&mut self, out: &mut String) -> Result<(), String> {
        let e = self.bump();
        match e {
            b'n' => out.push('\n'),
            b't' => out.push('\t'),
            b'r' => out.push('\r'),
            b'b' => out.push('\u{8}'),
            b'f' => out.push('\u{c}'),
            b'v' => out.push('\u{b}'),
            b'0' if !self.peek().is_ascii_digit() => out.push('\0'),
            b'\n' => {}
            b'x' => {
                let h = self.take_hex(2)?;
                out.push(char::from_u32(h).unwrap_or(char::REPLACEMENT_CHARACTER));
            }
            b'u' => {
                if self.peek() == b'{' {
                    self.pos += 1;
                    let start = self.pos;
                    while self.pos < self.src.len() && self.peek() != b'}' {
                        self.pos += 1;
                    }
                    let txt = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
                    self.pos += 1;
                    let n = u32::from_str_radix(&txt, 16)
                        .map_err(|_| self.err("bad \\u{} escape"))?;
                    match char::from_u32(n) {
                        Some(ch) => out.push(ch),
                        None => out.push(char::REPLACEMENT_CHARACTER),
                    }
                } else {
                    let h = self.take_hex(4)?;
                    out.push(char::from_u32(h).unwrap_or(char::REPLACEMENT_CHARACTER));
                }
            }
            other => out.push(other as char),
        }
        Ok(())
    }

    fn take_hex(&mut self, n: usize) -> Result<u32, String> {
        let start = self.pos;
        self.pos += n;
        if self.pos > self.src.len() {
            return Err(self.err("bad hex escape"));
        }
        let s = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
        u32::from_str_radix(&s, 16).map_err(|_| self.err("bad hex escape"))
    }

    fn read_template(&mut self) -> Result<Tok, String> {
        self.pos += 1; // `
        let mut parts: Vec<TplPart> = Vec::new();
        let mut chunk = String::new();
        let mut run = self.pos;
        loop {
            if self.pos >= self.src.len() {
                return Err(self.err("unterminated template literal"));
            }
            let c = self.bump();
            if c == b'`' {
                self.flush_run(&mut chunk, run, self.pos - 1);
                break;
            }
            if c == b'\\' {
                self.flush_run(&mut chunk, run, self.pos - 1);
                self.read_escape(&mut chunk)?;
                run = self.pos;
                continue;
            }
            if c == b'$' && self.peek() == b'{' {
                self.flush_run(&mut chunk, run, self.pos - 1);
                self.pos += 1;
                let raw = self.read_balanced_until_close_brace()?;
                parts.push(TplPart::Str(std::mem::take(&mut chunk)));
                parts.push(TplPart::Raw(raw));
                run = self.pos;
                continue;
            }
        }
        if parts.is_empty() {
            parts.push(TplPart::Str(chunk));
        } else {
            parts.push(TplPart::Str(chunk));
        }
        Ok(Tok::Template(parts))
    }

    /// Collect source up to the matching `}` of a `${` interpolation, handling
    /// nested braces, strings and template literals.
    fn read_balanced_until_close_brace(&mut self) -> Result<String, String> {
        let start = self.pos;
        let mut depth = 0usize;
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            match c {
                b'{' => {
                    depth += 1;
                    self.pos += 1;
                }
                b'}' => {
                    if depth == 0 {
                        let out = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
                        self.pos += 1;
                        return Ok(out);
                    }
                    depth -= 1;
                    self.pos += 1;
                }
                b'"' | b'\'' => {
                    let q = c;
                    self.pos += 1;
                    while self.pos < self.src.len() && self.src[self.pos] != q {
                        if self.src[self.pos] == b'\\' {
                            self.pos += 1;
                        }
                        self.pos += 1;
                    }
                    self.pos += 1;
                }
                b'`' => {
                    self.pos += 1;
                    let mut d = 0usize;
                    while self.pos < self.src.len() {
                        if self.src[self.pos] == b'\\' {
                            self.pos += 2;
                            continue;
                        }
                        if self.src[self.pos] == b'`' && d == 0 {
                            break;
                        }
                        if self.src[self.pos] == b'$' && self.src[self.pos + 1] == b'{' {
                            d += 1;
                            self.pos += 2;
                            continue;
                        }
                        if self.src[self.pos] == b'}' && d > 0 {
                            d -= 1;
                        }
                        self.pos += 1;
                    }
                    self.pos += 1;
                }
                _ => {
                    self.pos += 1;
                }
            }
        }
        Err(self.err("unterminated template interpolation"))
    }

    fn read_regex(&mut self) -> Result<Tok, String> {
        self.pos += 1; // /
        let start = self.pos;
        let mut in_class = false;
        loop {
            if self.pos >= self.src.len() {
                return Err(self.err("unterminated regex literal"));
            }
            let c = self.bump();
            match c {
                b'\\' => {
                    self.pos += 1;
                }
                b'[' => in_class = true,
                b']' => in_class = false,
                b'/' if !in_class => break,
                b'\n' => return Err(self.err("unterminated regex literal")),
                _ => {}
            }
        }
        let body = String::from_utf8_lossy(&self.src[start..self.pos - 1]).into_owned();
        let fstart = self.pos;
        while self.pos < self.src.len() && is_ident_part(self.src[self.pos]) {
            self.pos += 1;
        }
        let flags = String::from_utf8_lossy(&self.src[fstart..self.pos]).into_owned();
        Ok(Tok::Regex(body, flags))
    }

    fn read_number(&mut self) -> Result<Tok, String> {
        let start = self.pos;
        let mut is_float = false;
        if self.peek() == b'0' && (self.peek_at(1) == b'x' || self.peek_at(1) == b'X') {
            self.pos += 2;
            while self.pos < self.src.len()
                && (self.src[self.pos].is_ascii_hexdigit() || self.src[self.pos] == b'_')
            {
                self.pos += 1;
            }
            let txt: String = String::from_utf8_lossy(&self.src[start + 2..self.pos])
                .chars()
                .filter(|c| *c != '_')
                .collect();
            let v = i64::from_str_radix(&txt, 16).map_err(|_| self.err("bad hex literal"))?;
            if self.peek() == b'n' {
                self.pos += 1;
                return Ok(Tok::BigInt(v));
            }
            return Ok(Tok::Num(v as f64));
        }
        if self.peek() == b'0' && (self.peek_at(1) == b'o' || self.peek_at(1) == b'O') {
            self.pos += 2;
            while self.pos < self.src.len()
                && ((b'0'..=b'7').contains(&self.src[self.pos]) || self.src[self.pos] == b'_')
            {
                self.pos += 1;
            }
            let txt: String = String::from_utf8_lossy(&self.src[start + 2..self.pos])
                .chars()
                .filter(|c| *c != '_')
                .collect();
            let v = i64::from_str_radix(&txt, 8).map_err(|_| self.err("bad octal literal"))?;
            return Ok(Tok::Num(v as f64));
        }
        if self.peek() == b'0' && (self.peek_at(1) == b'b' || self.peek_at(1) == b'B') {
            self.pos += 2;
            while self.pos < self.src.len()
                && (self.src[self.pos] == b'0' || self.src[self.pos] == b'1' || self.src[self.pos] == b'_')
            {
                self.pos += 1;
            }
            let txt: String = String::from_utf8_lossy(&self.src[start + 2..self.pos])
                .chars()
                .filter(|c| *c != '_')
                .collect();
            let v = i64::from_str_radix(&txt, 2).map_err(|_| self.err("bad binary literal"))?;
            return Ok(Tok::Num(v as f64));
        }
        while self.pos < self.src.len()
            && (self.src[self.pos].is_ascii_digit() || self.src[self.pos] == b'_')
        {
            self.pos += 1;
        }
        if self.peek() == b'.' {
            is_float = true;
            self.pos += 1;
            while self.pos < self.src.len()
                && (self.src[self.pos].is_ascii_digit() || self.src[self.pos] == b'_')
            {
                self.pos += 1;
            }
        }
        if self.peek() == b'e' || self.peek() == b'E' {
            let save = self.pos;
            self.pos += 1;
            if self.peek() == b'+' || self.peek() == b'-' {
                self.pos += 1;
            }
            if self.peek().is_ascii_digit() {
                is_float = true;
                while self.pos < self.src.len() && self.src[self.pos].is_ascii_digit() {
                    self.pos += 1;
                }
            } else {
                self.pos = save;
            }
        }
        if self.peek() == b'n' {
            self.pos += 1;
            let txt: String = String::from_utf8_lossy(&self.src[start..self.pos - 1])
                .chars()
                .filter(|c| *c != '_')
                .collect();
            let v: i64 = txt.parse().map_err(|_| self.err("bad bigint literal"))?;
            return Ok(Tok::BigInt(v));
        }
        let txt: String = String::from_utf8_lossy(&self.src[start..self.pos])
            .chars()
            .filter(|c| *c != '_')
            .collect();
        let v: f64 = txt.parse().map_err(|_| self.err("bad number literal"))?;
        let _ = is_float;
        Ok(Tok::Num(v))
    }
}

fn is_ident_start(c: u8) -> bool {
    c == b'_' || c == b'$' || c.is_ascii_alphabetic() || c >= 0x80
}

fn is_ident_part(c: u8) -> bool {
    is_ident_start(c) || c.is_ascii_digit()
}
