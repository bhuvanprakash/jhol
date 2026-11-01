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
    BigInt(i128),
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
    ">>>=", "...", "===", "!==", "**=", "<<=", ">>=", ">>>", "&&=", "||=", "??=", "?.", "??", "=>",
    "==", "!=", "<=", ">=", "&&", "||", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=",
    "**", "<<", ">>", "{", "}", "(", ")", "[", "]", ";", ",", "<", ">", "+", "-", "*", "/", "%",
    "&", "|", "^", "!", "~", "?", ":", "=", ".",
];

const KEYWORDS: &[&str] = &[
    "var",
    "let",
    "const",
    "function",
    "return",
    "if",
    "else",
    "for",
    "while",
    "do",
    "break",
    "continue",
    "new",
    "delete",
    "typeof",
    "instanceof",
    "in",
    "of",
    "void",
    "throw",
    "try",
    "catch",
    "finally",
    "switch",
    "case",
    "default",
    "class",
    "extends",
    "super",
    "this",
    "yield",
    "with",
    "await",
    "async",
    "get",
    "set",
    "static",
    "null",
    "true",
    "false",
    "undefined",
    "import",
    "export",
    "as",
    "from",
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
    /// Byte offset where each token in `toks` starts (parallel vec).
    pub starts: Vec<u32>,
    pub comments: Vec<LexComment>,
    prev_significant: Option<Tok>,
    /// One entry per open `(`: true when it opened a control header
    /// (`for`, `if`, `while`, `with`).
    ctrl_paren: Vec<bool>,
    /// One entry per open `{`: true when it opens a statement body rather
    /// than an object literal.
    block_brace: Vec<bool>,
    pending_ctrl: bool,
    /// The previous token just closed a control header or statement block, so
    /// a following `/` starts a regex (the body of the statement).
    regex_after: bool,
    /// The previous significant token is a property name (it followed `.`).
    /// Keywords are valid property names (`a.else`, `x.in`), and a `/` after
    /// one is always division, never a regex start.
    prev_was_prop: bool,
    tok_start: usize,
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
        let start = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
            3
        } else {
            0
        };
        Lexer {
            src: &bytes[start..],
            pos: 0,
            line: 1,
            toks: Vec::new(),
            starts: Vec::new(),
            comments: Vec::new(),
            prev_significant: None,
            ctrl_paren: Vec::new(),
            block_brace: Vec::new(),
            pending_ctrl: false,
            regex_after: false,
            prev_was_prop: false,
            tok_start: 0,
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
        if self.regex_after {
            return true;
        }
        if self.prev_was_prop {
            return false;
        }
        match &self.prev_significant {
            None => true,
            Some(Tok::Num(_)) | Some(Tok::BigInt(_)) | Some(Tok::Str(_)) | Some(Tok::Str16(_)) => {
                false
            }
            Some(Tok::Template(_)) | Some(Tok::Regex(_, _)) => false,
            Some(Tok::Private(_)) => false,
            Some(Tok::Punct(p)) => !matches!(*p, ")" | "]" | "}"),
            // `as`/`from` only appear in import/export clauses, where a
            // following `/` is never a regex — and both are valid identifier
            // names in bundles (`M + as / 500`).
            Some(Tok::Keyword(k)) => !matches!(
                *k,
                "this" | "super" | "true" | "false" | "null" | "undefined" | "as" | "from"
            ),
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
            let start_line = self.line;
            self.tok_start = self.pos;
            let t = self.next_token()?;
            self.push_at(t, start_line);
        }
        Ok(())
    }

    fn push(&mut self, t: Tok) {
        let line = self.line;
        self.push_at(t, line);
    }

    fn push_at(&mut self, t: Tok, line: u32) {
        self.starts.push(self.tok_start as u32);
        let significant = !matches!(t, Tok::Eof);
        if significant {
            self.regex_after = false;
            match &t {
                Tok::Keyword(k) => match *k {
                    "for" | "if" | "while" | "with" => self.pending_ctrl = true,
                    "await" if self.pending_ctrl => {}
                    _ => self.pending_ctrl = false,
                },
                Tok::Punct("(") => {
                    self.ctrl_paren.push(self.pending_ctrl);
                    self.pending_ctrl = false;
                }
                Tok::Punct(")") => {
                    if self.ctrl_paren.pop().unwrap_or(false) {
                        self.regex_after = true;
                    }
                }
                Tok::Punct("{") => {
                    let block = match &self.prev_significant {
                        None => true,
                        Some(Tok::Punct(p)) => matches!(*p, ")" | "=>" | ";" | "{" | "}"),
                        Some(Tok::Keyword(k)) => {
                            matches!(*k, "else" | "try" | "finally" | "do" | "class" | "static")
                        }
                        _ => false,
                    };
                    self.block_brace.push(block);
                }
                Tok::Punct("}") => {
                    if self.block_brace.pop().unwrap_or(false) {
                        self.regex_after = true;
                    }
                }
                _ => self.pending_ctrl = false,
            }
            self.prev_was_prop = matches!(&self.prev_significant, Some(Tok::Punct(".")));
            self.prev_significant = Some(t.clone());
        }
        // Token line is where the token *starts*: a multiline template must
        // not look like it began after its `return`.
        self.toks.push((t, line));
    }

    /// Decode exactly one scalar. A fixed four-byte window can end halfway
    /// through the following scalar and make valid UTF-8 appear invalid.
    fn peek_char(&self) -> Option<char> {
        let input = self.src.get(self.pos..)?;
        let first = *input.first()?;
        let width = if first < 0x80 {
            1
        } else if first < 0xe0 {
            2
        } else if first < 0xf0 {
            3
        } else {
            4
        };
        std::str::from_utf8(input.get(..width)?)
            .ok()?
            .chars()
            .next()
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
                let text = String::from_utf8_lossy(&self.src[start..self.pos]).to_string();
                self.comments.push(LexComment {
                    text,
                    next_tok: self.toks.len(),
                    line,
                });
            } else if matches!(c, b'\t'..=b'\r' | b' ') {
                self.bump();
            } else if c == b'#' && self.pos == 0 && self.src.starts_with(b"#!") {
                while self.pos < self.src.len() && self.peek() != b'\n' {
                    self.pos += 1;
                }
            } else if c >= 0x80 {
                // ECMAScript whitespace includes Unicode spaces such as NBSP,
                // which real bundles use (often invisibly) between tokens.
                let ch = self.peek_char();
                match ch {
                    Some(ch) if is_js_whitespace(ch) => {
                        self.pos += ch.len_utf8();
                        if matches!(ch, '\u{2028}' | '\u{2029}') {
                            self.line += 1;
                        }
                    }
                    _ => break,
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
        if c == b'#'
            && (is_ident_start(self.peek_at(1))
                || (self.peek_at(1) == b'\\' && self.peek_at(2) == b'u'))
        {
            self.pos += 1;
            let s = self.read_ident_text();
            if s.is_empty() {
                return Err(self.err("invalid private identifier"));
            }
            return Ok(Tok::Private(s));
        }
        if is_ident_start(c) || (c == b'\\' && self.peek_at(1) == b'u') {
            let start = self.pos;
            let s = self.read_ident_text();
            if self.pos == start {
                return Err(self.err("invalid identifier escape"));
            }
            let escaped = self.src[start..self.pos].contains(&b'\\');
            if !escaped {
                if let Some(kw) = KEYWORDS.iter().find(|k| **k == s.as_str()) {
                    return Ok(Tok::Keyword(kw));
                }
            }
            return Ok(Tok::Ident(s));
        }
        if c.is_ascii_digit() || (c == b'.' && self.peek_at(1).is_ascii_digit()) {
            return self.read_number();
        }
        self.read_punct()
    }

    fn read_ident_text(&mut self) -> String {
        let mut out = String::new();
        while self.pos < self.src.len() {
            let c = self.src[self.pos];
            if c == b'\\' && self.peek_at(1) == b'u' {
                let start = self.pos;
                self.pos += 2;
                let code = if self.peek() == b'{' {
                    self.pos += 1;
                    let begin = self.pos;
                    while self.pos < self.src.len() && self.peek() != b'}' {
                        self.pos += 1;
                    }
                    let digits = String::from_utf8_lossy(&self.src[begin..self.pos]).into_owned();
                    if self.peek() == b'}' {
                        self.pos += 1;
                    }
                    u32::from_str_radix(&digits, 16).ok()
                } else if self.pos + 4 <= self.src.len() {
                    let digits =
                        String::from_utf8_lossy(&self.src[self.pos..self.pos + 4]).into_owned();
                    self.pos += 4;
                    u32::from_str_radix(&digits, 16).ok()
                } else {
                    None
                };
                match code.and_then(char::from_u32) {
                    Some(ch) => out.push(ch),
                    None => {
                        self.pos = start;
                        break;
                    }
                }
            } else if is_ident_part(c) {
                // Decode only the character at the cursor, never the rest of
                // the source: validating a whole huge file per ident char is
                // quadratic.
                let ch = self.peek_char();
                match ch {
                    Some(ch) => {
                        if is_js_whitespace(ch) {
                            break;
                        }
                        out.push(ch);
                        self.pos += ch.len_utf8();
                    }
                    None => break,
                }
            } else {
                break;
            }
        }
        out
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
        let ctx: Vec<String> = self
            .toks
            .iter()
            .enumerate()
            .rev()
            .take(6)
            .map(|(n, _)| {
                let a = self.starts.get(n).copied().unwrap_or(0) as usize;
                let b = self.starts.get(n + 1).copied().unwrap_or(self.pos as u32) as usize;
                let slice = String::from_utf8_lossy(
                    &self.src[a.min(self.src.len())..b.min(self.src.len())],
                );
                let text: String = slice.chars().take(40).collect();
                format!("@{}={:?}", a, text)
            })
            .collect();
        Err(self.err(&format!(
            "unexpected character `{}` at byte {} [{}]",
            c as char,
            self.pos - 1,
            ctx.join(" | ")
        )))
    }

    /// Decode a verbatim byte run as UTF-8 and append it.
    ///
    /// String and template contents are accumulated as byte runs rather than
    /// one `char` per byte: `b as char` maps each byte through Latin-1, which
    /// turned `"héllo"` into `"hÃ©llo"` and corrupted every non-ASCII literal
    /// in both `jhol run` and `jhol fmt`. The whole source is validated as
    /// UTF-8 before it reaches the lexer, so this decodes exactly; lossy keeps
    /// a stray invalid byte from panicking.
    fn flush_run(&self, cooked: &mut String, raw: &mut String, from: usize, to: usize) {
        if from >= to || to > self.src.len() {
            return;
        }
        let text = String::from_utf8_lossy(&self.src[from..to]);
        cooked.push_str(&text);
        raw.push_str(&text);
    }

    fn read_string(&mut self, quote: u8) -> Result<Tok, String> {
        self.pos += 1;
        let mut units = Vec::new();
        let mut run = self.pos;
        loop {
            if self.pos >= self.src.len() {
                return Err(self.err("unterminated string literal"));
            }
            let c = self.bump();
            if c == quote || c == b'\\' {
                units.extend(String::from_utf8_lossy(&self.src[run..self.pos - 1]).encode_utf16());
                if c == quote {
                    break;
                }
                if self.peek() == b'u' {
                    self.bump();
                    let n = if self.peek() == b'{' {
                        self.bump();
                        let start = self.pos;
                        while self.pos < self.src.len() && self.peek() != b'}' {
                            self.bump();
                        }
                        if self.pos == self.src.len() {
                            return Err(self.err("unterminated Unicode escape"));
                        }
                        let n = u32::from_str_radix(
                            std::str::from_utf8(&self.src[start..self.pos]).unwrap_or(""),
                            16,
                        )
                        .map_err(|_| self.err("bad Unicode escape"))?;
                        self.bump();
                        n
                    } else {
                        self.take_hex(4)?
                    };
                    if n > 0x10ffff {
                        return Err(self.err("invalid Unicode code point"));
                    }
                    if n <= 65535 {
                        units.push(n as u16);
                    } else {
                        let n = n - 65536;
                        units.extend([0xd800 + (n >> 10) as u16, 0xdc00 + (n & 1023) as u16]);
                    }
                } else {
                    let mut escaped = String::new();
                    self.read_escape(&mut escaped)?;
                    units.extend(escaped.encode_utf16());
                }
                run = self.pos;
            } else if c == b'\n' {
                return Err(self.err("unterminated string literal"));
            }
        }
        Ok(match String::from_utf16(&units) {
            Ok(s) => Tok::Str(s),
            Err(_) => Tok::Str16(units),
        })
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
                    let n =
                        u32::from_str_radix(&txt, 16).map_err(|_| self.err("bad \\u{} escape"))?;
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
        let mut cooked = String::new();
        let mut raw_text = String::new();
        let mut run = self.pos;
        loop {
            if self.pos >= self.src.len() {
                return Err(self.err("unterminated template literal"));
            }
            let c = self.bump();
            if c == b'`' {
                self.flush_run(&mut cooked, &mut raw_text, run, self.pos - 1);
                break;
            }
            if c == b'\\' {
                self.flush_run(&mut cooked, &mut raw_text, run, self.pos - 1);
                let esc_start = self.pos - 1;
                self.read_escape(&mut cooked)?;
                // Keep the escape exactly as written for `strings.raw`.
                raw_text.push_str(&String::from_utf8_lossy(&self.src[esc_start..self.pos]));
                run = self.pos;
                continue;
            }
            if c == b'$' && self.peek() == b'{' {
                self.flush_run(&mut cooked, &mut raw_text, run, self.pos - 1);
                self.pos += 1;
                let hole = self.read_balanced_until_close_brace()?;
                parts.push(TplPart::Str {
                    cooked: std::mem::take(&mut cooked),
                    raw: std::mem::take(&mut raw_text),
                });
                parts.push(TplPart::Raw(hole));
                run = self.pos;
                continue;
            }
        }
        parts.push(TplPart::Str {
            cooked,
            raw: raw_text,
        });
        Ok(Tok::Template(parts))
    }

    /// Collect source up to the matching `}` of a `${` interpolation, handling
    /// Collect source up to the matching `}` of a `${` interpolation.
    ///
    /// This has to understand every construct that can contain a `}`:
    /// strings, nested template literals (with their own interpolations),
    /// line/block comments and object literals. The earlier version only
    /// counted `${` pairs and got the nesting wrong on an object literal
    /// inside a nested `${...}`, which then swallowed the rest of the file.
    fn read_balanced_until_close_brace(&mut self) -> Result<String, String> {
        let start = self.pos;
        let mut depth = 0usize;
        while self.pos < self.src.len() {
            match self.peek() {
                b'{' => {
                    self.bump();
                    depth += 1;
                }
                b'}' => {
                    if depth == 0 {
                        let out = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
                        self.pos += 1;
                        return Ok(out);
                    }
                    self.bump();
                    depth -= 1;
                }
                b'"' | b'\'' => {
                    let q = self.peek();
                    self.bump();
                    loop {
                        if self.pos >= self.src.len() {
                            return Err(self.err("unterminated string in template interpolation"));
                        }
                        let c = self.bump();
                        if c == q {
                            break;
                        }
                        if c == b'\\' {
                            self.bump();
                        }
                    }
                }
                b'`' => {
                    // Parse it here (and discard the token) so nested
                    // interpolations and line numbers stay exact.
                    self.read_template()?;
                }
                b'/' if self.peek_at(1) == b'/' && !self.escaped_slash() => {
                    while self.pos < self.src.len() && self.peek() != b'\n' {
                        self.bump();
                    }
                }
                b'/' if self.peek_at(1) == b'*' && !self.escaped_slash() => {
                    self.bump();
                    self.bump();
                    loop {
                        if self.pos >= self.src.len() {
                            return Err(self.err("unterminated comment in template interpolation"));
                        }
                        if self.peek() == b'*' && self.peek_at(1) == b'/' {
                            self.bump();
                            self.bump();
                            break;
                        }
                        self.bump();
                    }
                }
                b'/' if self.slash_starts_regex() => {
                    // A regex can contain `//`, `/*` and `}`; skipping it as
                    // plain text would let one of those end the interpolation.
                    self.bump();
                    let mut in_class = false;
                    loop {
                        if self.pos >= self.src.len() {
                            break;
                        }
                        let c = self.bump();
                        if c == b'\\' {
                            self.bump();
                        } else if c == b'[' {
                            in_class = true;
                        } else if c == b']' {
                            in_class = false;
                        } else if c == b'/' && !in_class {
                            while self.pos < self.src.len() && is_ident_part(self.src[self.pos]) {
                                self.bump();
                            }
                            break;
                        } else if c == b'\n' {
                            break;
                        }
                    }
                }
                _ => {
                    self.bump();
                }
            }
        }
        Err(self.err("unterminated template interpolation"))
    }

    fn escaped_slash(&self) -> bool {
        self.pos > 0 && self.src[self.pos - 1] == b'\\'
    }

    /// True when a `/` at the current position looks like a regex start rather
    /// than division, based on the previous non-space byte.
    fn slash_starts_regex(&self) -> bool {
        let mut i = self.pos;
        while i > 0 {
            i -= 1;
            let c = self.src[i];
            if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                continue;
            }
            return matches!(
                c,
                b'(' | b'['
                    | b'{'
                    | b','
                    | b';'
                    | b':'
                    | b'='
                    | b'!'
                    | b'&'
                    | b'|'
                    | b'?'
                    | b'*'
                    | b'+'
                    | b'-'
                    | b'%'
                    | b'^'
                    | b'~'
            );
        }
        true
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
            let digits: String = String::from_utf8_lossy(&self.src[start + 2..self.pos])
                .chars()
                .filter(|c| *c != '_')
                .collect();
            if self.peek() == b'n' {
                self.pos += 1;
                let v =
                    i128::from_str_radix(&digits, 16).map_err(|_| self.err("bad hex literal"))?;
                return Ok(Tok::BigInt(v));
            }
            // JS numbers are doubles. Hex literals like 0x8000000000000000
            // exceed i64 but are perfectly valid (used as bit patterns).
            let v = digits.chars().fold(0f64, |acc, c| {
                acc * 16.0 + c.to_digit(16).unwrap_or(0) as f64
            });
            if digits.is_empty() {
                return Err(self.err("bad hex literal"));
            }
            return Ok(Tok::Num(v));
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
                && (self.src[self.pos] == b'0'
                    || self.src[self.pos] == b'1'
                    || self.src[self.pos] == b'_')
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
            let v: i128 = txt.parse().map_err(|_| self.err("bad bigint literal"))?;
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

// ECMAScript WhiteSpace + LineTerminator, rather than Rust's broader Unicode
// White_Space property. FEFF may also occur between concatenated source files.
fn is_js_whitespace(c: char) -> bool {
    matches!(c, '\t'..='\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}'
        | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

fn is_ident_part(c: u8) -> bool {
    is_ident_start(c) || c.is_ascii_digit()
}
