//! Pratt parser.
//!
//! Two details are worth calling out because they are where hand-written ES
//! parsers usually go wrong:
//!
//! * **Template literals** arrive from the lexer already split into literal
//!   chunks and *raw source* holes (`TplPart::Raw`). Each hole is re-parsed with
//!   a nested `Parser`, which is what keeps template interpolation out of the
//!   lexer's brace scanner.
//! * **ASI** is driven by comparing token line numbers, plus an explicit
//!   restricted-production list for `return`/`throw`/`break`/`continue`.

use crate::ast::*;
use crate::lexer::{self, Tok};
use crate::objects::PropKey;

pub fn parse_program(src: &str) -> Result<Func, String> {
    let (toks, comments) = lexer::tokenize_with_comments(src)?;
    let mut p = Parser::from_tokens_with_comments(toks, comments);
    let body = p.program()?;
    Ok(Func {
        name: None,
        params: Vec::new(),
        body,
        is_arrow: false,
        is_async: false,
        is_generator: false,
        expr_body: None,
    })
}

pub struct Parser {
    toks: Vec<(Tok, u32)>,
    i: usize,
    no_in: bool,
    /// Comments in source order, waiting to be spliced into the statement
    /// stream at `next_tok`.
    comments: Vec<lexer::LexComment>,
    ci: usize,
}

impl Parser {
    pub fn from_tokens(toks: Vec<(Tok, u32)>) -> Self {
        Parser {
            toks,
            i: 0,
            no_in: false,
            comments: Vec::new(),
            ci: 0,
        }
    }

    pub fn from_tokens_with_comments(
        toks: Vec<(Tok, u32)>,
        comments: Vec<lexer::LexComment>,
    ) -> Self {
        Parser {
            toks,
            i: 0,
            no_in: false,
            comments,
            ci: 0,
        }
    }

    /// Emit the next comment as a statement if it precedes the current token.
    /// Returns `false` when there is nothing to emit.
    fn take_comment(&mut self) -> Option<Stmt> {
        let c = self.comments.get(self.ci)?;
        if c.next_tok > self.i {
            return None;
        }
        let c = c.clone();
        self.ci += 1;
        // Same line as the token before it means it trailed a statement.
        let prev_line = if self.i == 0 { 0 } else { self.toks[self.i - 1].1 };
        let trailing = c.line == prev_line && prev_line != 0;
        // A comment from inside an expression (`const x = /** doc */
        // (async function () {}.x)`) is flushed at the next `statement()`
        // call — inside the function body being parsed, the wrong scope.
        // Emitting its text there corrupts the output, so a displaced comment
        // is dropped. Statement-position comments — the overwhelming
        // majority — are kept byte-exact.
        if c.next_tok < self.i {
            return None;
        }
        Some(Stmt::Comment { text: c.text, trailing })
    }

    /// Flush every comment that has not been consumed yet — called where a
    /// construct ends, so a comment just before a `}` is not dropped.
    /// Flush every pending comment. Unlike `take_comment` this ignores the
    /// position test: a block that is ending is the last place any of these
    /// can go, and losing them is worse than placing them imprecisely.
    fn drain_comments(&mut self, out: &mut Vec<Stmt>) {
        while let Some(c) = self.comments.get(self.ci).cloned() {
            // Only comments preceding this point belong to this block; one
            // from a later construct stays queued for its own block.
            if c.next_tok > self.i {
                break;
            }
            self.ci += 1;
            out.push(Stmt::Comment {
                text: c.text,
                trailing: false,
            });
        }
    }

    pub fn new(src: &str) -> Result<Self, String> {
        let toks = lexer::tokenize(src)?;
        Ok(Parser {
            toks,
            i: 0,
            no_in: false,
            comments: Vec::new(),
            ci: 0,
        })
    }

    /// Parse a single expression (used for raw template holes).
    #[allow(clippy::too_many_arguments)]
    pub fn parse_expression_only(&mut self) -> Result<Expr, String> {
        self.assignment_expr()
    }

    fn line(&self, idx: usize) -> u32 {
        self.toks[idx].1
    }

    fn cur(&self) -> &Tok {
        &self.toks[self.i.min(self.toks.len() - 1)].0
    }

    fn nth(&self, n: usize) -> &Tok {
        &self.toks[(self.i + n).min(self.toks.len() - 1)].0
    }

    fn cur_line(&self) -> u32 {
        self.toks[self.i.min(self.toks.len() - 1)].1
    }

    /// True when a newline separates the previous token from the current one.
    fn newline_before(&self) -> bool {
        if self.i == 0 {
            return false;
        }
        self.line(self.i) > self.line(self.i - 1)
    }

    fn at_end(&self) -> bool {
        matches!(self.cur(), Tok::Eof)
    }

    fn bump(&mut self) -> Tok {
        let t = self.toks[self.i.min(self.toks.len() - 1)].0.clone();
        if self.i + 1 < self.toks.len() {
            self.i += 1;
        }
        t
    }

    fn at_punct(&self, p: &str) -> bool {
        matches!(self.cur(), Tok::Punct(x) if *x == p)
    }
    fn at_punct2(&self, p: &str) -> bool {
        matches!(self.nth(1), Tok::Punct(x) if *x == p)
    }
    fn at_kw(&self, k: &str) -> bool {
        matches!(self.cur(), Tok::Keyword(x) if *x == k)
    }
    fn eat_punct(&mut self, p: &str) -> bool {
        if self.at_punct(p) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn eat_kw(&mut self, k: &str) -> bool {
        if self.at_kw(k) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn expect_punct(&mut self, p: &str) -> Result<(), String> {
        if self.eat_punct(p) {
            Ok(())
        } else {
            Err(self.err(&format!("expected `{}`, found {}", p, self.cur().describe())))
        }
    }
    fn expect_kw(&mut self, k: &str) -> Result<(), String> {
        if self.eat_kw(k) {
            Ok(())
        } else {
            Err(self.err(&format!("expected `{}`, found {}", k, self.cur().describe())))
        }
    }
    fn err(&self, msg: &str) -> String {
        format!("SyntaxError: line {}: {}", self.cur_line(), msg)
    }

    fn ident_name(&mut self) -> Result<String, String> {
        match self.cur().clone() {
            Tok::Ident(s) => {
                self.bump();
                Ok(s)
            }
            Tok::Keyword(k) if !matches!(k, "null" | "true" | "false" | "undefined") => {
                self.bump();
                Ok(k.to_string())
            }
            t => Err(self.err(&format!("expected an identifier, found {}", t.describe()))),
        }
    }

    fn is_ident(&self) -> bool {
        match self.cur() {
            Tok::Ident(_) => true,
            Tok::Keyword(k) => !matches!(*k, "null" | "true" | "false" | "undefined"),
            _ => false,
        }
    }

    /// Consume `;` if present, otherwise apply ASI.
    fn semi(&mut self) -> Result<(), String> {
        if self.eat_punct(";") {
            return Ok(());
        }
        if self.at_punct("}") || self.at_end() || self.newline_before() {
            return Ok(());
        }
        Err(self.err(&format!("expected `;`, found {}", self.cur().describe())))
    }

    // ------------------------------------------------------------------
    // program & statements
    // ------------------------------------------------------------------

    fn program(&mut self) -> Result<Vec<Stmt>, String> {
        let mut out = Vec::new();
        while !self.at_end() {
            out.push(self.statement()?);
        }
        self.drain_comments(&mut out);
        Ok(out)
    }

    fn statement(&mut self) -> Result<Stmt, String> {
        // A comment pending before the current token becomes a statement of
        // its own, so it travels through the AST instead of being discarded.
        if let Some(c) = self.take_comment() {
            return Ok(c);
        }
        // Labels: `name:` — but only when not a use of `default`/`case`.
        if let Tok::Ident(_) = self.cur() {
            if self.at_punct2(":") {
                let name = match self.bump() {
                    Tok::Ident(s) => s,
                    _ => unreachable!(),
                };
                self.bump(); // :
                let inner = self.statement()?;
                return Ok(Stmt::Labeled(name, Box::new(inner)));
            }
        }

        if self.eat_punct(";") {
            return Ok(Stmt::Empty);
        }
        if self.at_punct("{") {
            return self.block();
        }
        if let Tok::Keyword(k) = self.cur().clone() {
            return self.keyword_statement(k);
        }
        // Async function declaration.
        if self.is_async_fn() {
            self.bump(); // async
            let f = self.function_after_async()?;
            let name = f.name.clone().unwrap_or_default();
            return Ok(Stmt::Func(Box::new(f)));
        }
        let e = self.expression()?;
        self.semi()?;
        let _ = name_unused();
        Ok(Stmt::Expr(e))
    }

    fn block(&mut self) -> Result<Stmt, String> {
        self.expect_punct("{")?;
        let mut out = Vec::new();
        while !self.at_punct("}") && !self.at_end() {
            out.push(self.statement()?);
        }
        self.drain_comments(&mut out);
        self.expect_punct("}")?;
        Ok(Stmt::Block(out))
    }

    fn statement_body(&mut self) -> Result<Stmt, String> {
        let mut comments = Vec::new();
        while let Some(c) = self.take_comment() {
            comments.push(c);
        }
        let mut s = self.statement()?;
        if comments.is_empty() {
            Ok(s)
        } else {
            match &mut s {
                Stmt::Block(body) => {
                    comments.append(body);
                    *body = comments;
                    Ok(s)
                }
                _ => {
                    comments.push(s);
                    Ok(Stmt::Block(comments))
                }
            }
        }
    }

    fn keyword_statement(&mut self, k: &str) -> Result<Stmt, String> {
        match k {
            "var" => self.var_statement(VarKind::Var),
            "let" => {
                if self.is_let_decl() {
                    self.var_statement(VarKind::Let)
                } else {
                    let e = self.expression()?;
                    self.semi()?;
                    Ok(Stmt::Expr(e))
                }
            }
            "const" => self.var_statement(VarKind::Const),
            "function" => {
                let f = self.function(false, false)?;
                let name = f.name.clone().unwrap_or_default();
                let _ = name;
                Ok(Stmt::Func(Box::new(f)))
            }
            "async" => {
                if matches!(self.nth(1), Tok::Keyword("function")) && !self.newline_before() {
                    self.bump();
                    let f = self.function_after_async()?;
                    return Ok(Stmt::Func(Box::new(f)));
                }
                let e = self.expression()?;
                self.semi()?;
                Ok(Stmt::Expr(e))
            }
            "class" => {
                let c = self.class_decl()?;
                Ok(Stmt::Class(Box::new(c)))
            }
            "if" => {
                self.bump();
                self.expect_punct("(")?;
                let c = self.expression()?;
                self.expect_punct(")")?;
                let a = Box::new(self.statement_body()?);
                let b = if self.eat_kw("else") {
                    Some(Box::new(self.statement_body()?))
                } else {
                    None
                };
                Ok(Stmt::If(c, a, b))
            }
            "while" => {
                self.bump();
                self.expect_punct("(")?;
                let c = self.expression()?;
                self.expect_punct(")")?;
                Ok(Stmt::While(c, Box::new(self.statement_body()?)))
            }
            "do" => {
                self.bump();
                let body = Box::new(self.statement_body()?);
                self.expect_kw("while")?;
                self.expect_punct("(")?;
                let c = self.expression()?;
                self.expect_punct(")")?;
                self.eat_punct(";");
                Ok(Stmt::DoWhile(body, c))
            }
            "for" => self.for_statement(),
            "return" => {
                self.bump();
                if self.at_punct(";") || self.at_punct("}") || self.at_end() || self.newline_before() {
                    self.eat_punct(";");
                    return Ok(Stmt::Return(None));
                }
                let e = self.expression()?;
                self.semi()?;
                Ok(Stmt::Return(Some(e)))
            }
            "break" => {
                self.bump();
                let l = if !self.at_punct(";") && !self.newline_before() && !self.at_end() {
                    if let Tok::Ident(s) = self.cur().clone() {
                        self.bump();
                        Some(s)
                    } else {
                        None
                    }
                } else {
                    None
                };
                self.semi()?;
                Ok(Stmt::Break(l))
            }
            "continue" => {
                self.bump();
                let l = if !self.at_punct(";") && !self.newline_before() && !self.at_end() {
                    if let Tok::Ident(s) = self.cur().clone() {
                        self.bump();
                        Some(s)
                    } else {
                        None
                    }
                } else {
                    None
                };
                self.semi()?;
                Ok(Stmt::Continue(l))
            }
            "throw" => {
                self.bump();
                let e = self.expression()?;
                self.semi()?;
                Ok(Stmt::Throw(e))
            }
            "try" => self.try_statement(),
            "switch" => self.switch_statement(),
            "export" | "import" => {
                // Module syntax: consume to the terminating `;` (respecting
                // bracket depth so `export { a, b };` survives) and rebuild the
                // source so the formatter can round-trip it verbatim.
                let mut depth = 0i32;
                let mut text = String::new();
                let mut needs_gap = false;
                let mut last_kw = false;
                // Spacing: bare words and bracket-closers separate on their own;
                // `(`/`[` attach to a word (`f(x)`) but space after a keyword
                // (`return (x)`); `{` always separates (`export { a }`).
                macro_rules! push {
                    ($toks:expr, $s:expr, $is_kw:expr) => {{
                        let owned = $s;
                        let s: &str = owned.as_str();
                        let group = matches!(s, "(" | "[");
                        let brace = s == "{";
                        let closes = matches!(s, ")" | "]" | "}");
                        // Separators/operators that want a gap after them. Brackets,
                        // `.`, `?.`, `++` and `--` do not.
                        let sep = matches!(
                            s,
                            "," | ";" | ":" | "=>" | "..."
                                | "=" | "==" | "!=" | "==" | "!==" | "<" | ">" | "<=" | ">="
                                | "+" | "-" | "*" | "/" | "%" | "**"
                                | "&&" | "||" | "??" | "&" | "|" | "^" | "~" | "!" | "?"
                                | "<<" | ">>" | ">>>" | "instanceof" | "in"
                        );
                        let quoted = s.starts_with('"')
                            || s.starts_with('\'')
                            || s.starts_with('`')
                            || (s.starts_with('/') && s.len() > 1 && s.ends_with('/'));
                        let word = !s.is_empty()
                            && (quoted
                                || s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$'));
                        if word || closes {
                            if needs_gap || last_kw {
                                $toks.push(' ');
                            }
                            $toks.push_str(s);
                            needs_gap = true;
                            last_kw = $is_kw;
                        } else if group {
                            // `f(x)` attaches; `return (x)` / `new (x)` do not.
                            if last_kw {
                                $toks.push(' ');
                            }
                            $toks.push_str(s);
                            needs_gap = true;
                            last_kw = false;
                        } else if brace {
                            if last_kw || needs_gap {
                                $toks.push(' ');
                            }
                            $toks.push_str(s);
                            needs_gap = true;
                            last_kw = false;
                        } else {
                            $toks.push_str(s);
                            needs_gap = sep;
                            last_kw = false;
                        }
                    }};
                }
                loop {
                    let is_kw = matches!(self.cur(), Tok::Keyword(_));
                    let piece: String = match self.cur().clone() {
                        Tok::Eof => break,
                        Tok::Punct(";") if depth == 0 => {
                            push!(text, ";".to_string(), false);
                            self.bump();
                            break;
                        }
                        Tok::Punct(p) => {
                            match p {
                                "{" | "[" | "(" => depth += 1,
                                "}" | "]" | ")" => depth -= 1,
                                _ => {}
                            }
                            (*p).to_string()
                        }
                        Tok::Str(v) => crate::formatter::quote(&v),
                        Tok::Str16(v) => crate::formatter::quote_utf16(&v),
                        Tok::Template(parts) => {
                            let mut out = String::from("`");
                            for part in &parts {
                                match part {
                                    crate::ast::TplPart::Str(v) => out.push_str(v),
                                    crate::ast::TplPart::Expr(e) => out.push_str(&format!(
                                        "${{{}}}",
                                        crate::formatter::expr_source(e.as_ref())
                                    )),
                                    crate::ast::TplPart::Raw(raw) => {
                                        out.push_str(&format!("${{{}}}", raw.as_str()))
                                    }
                                }
                            }
                            out.push('`');
                            out
                        }
                        Tok::Num(n) => crate::value::f64_to_js_string(n),
                        Tok::BigInt(n) => format!("{}n", n),
                        Tok::Ident(n) | Tok::Private(n) => n.clone(),
                        Tok::Keyword(k) => k.to_string(),
                        Tok::Regex(b, f) => format!("/{}/{}", b, f),
                    };
                    if !piece.is_empty() {
                        push!(text, piece, is_kw);
                    }
                    self.bump();
                }
                Ok(Stmt::Raw(text.trim_end().to_string()))
            }
            "get" | "set" => {
                let e = self.expression()?;
                self.semi()?;
                Ok(Stmt::Expr(e))
            }
            "static" => {
                let e = self.expression()?;
                self.semi()?;
                Ok(Stmt::Expr(e))
            }
            _ => {
                let e = self.expression()?;
                self.semi()?;
                Ok(Stmt::Expr(e))
            }
        }
    }

    /// `let` is only a declaration when an identifier (or `[`/`{`) follows.
    fn is_let_decl(&self) -> bool {
        match self.nth(1) {
            Tok::Ident(_) => true,
            Tok::Keyword(k) => !matches!(*k, "in" | "instanceof" | "of"),
            Tok::Punct("[") | Tok::Punct("{") => true,
            _ => false,
        }
    }

    fn var_statement(&mut self, kind: VarKind) -> Result<Stmt, String> {
        self.bump();
        let mut decls = Vec::new();
        loop {
            let target = self.binding_target()?;
            let init = if self.eat_punct("=") {
                Some(self.assignment_expr()?)
            } else {
                None
            };
            decls.push((target, init));
            if !self.eat_punct(",") {
                break;
            }
        }
        self.semi()?;
        Ok(Stmt::Var(kind, decls))
    }

    fn for_statement(&mut self) -> Result<Stmt, String> {
        self.bump();
        let is_await = self.eat_kw("await");
        self.expect_punct("(")?;
        let mut head: Option<Box<Stmt>> = None;

        if self.at_punct(";") {
            self.bump();
        } else {
            let saved = self.no_in;
            if self.at_kw("var") || self.at_kw("const") || (self.at_kw("let") && self.is_let_decl()) {
                let kind = match self.cur() {
                    Tok::Keyword("var") => VarKind::Var,
                    Tok::Keyword("const") => VarKind::Const,
                    _ => VarKind::Let,
                };
                // Only an initializer restricted to `no_in` — that is what
                // keeps `for (let x = a in b; …)` from being read as a loop
                // head. Test and update are plain expressions, so the flag is
                // dropped again before them.
                self.no_in = true;
                self.bump();
                // Several declarators: `for (let i = 0, j = n; …)` is ordinary
                // real-world code and used to stop at the first comma.
                let mut decls = Vec::new();
                loop {
                    let target = self.binding_target()?;
                    let init = if self.eat_punct("=") {
                        Some(self.assignment_expr()?)
                    } else {
                        None
                    };
                    decls.push((target, init));
                    if !self.eat_punct(",") {
                        break;
                    }
                }
                self.no_in = saved;
                head = Some(Box::new(Stmt::Var(kind, decls)));
            } else {
                // Restricted too: the `in` must survive the parse so the
                // for-in detection below can see it.
                self.no_in = true;
                let e = self.expression()?;
                head = Some(Box::new(Stmt::Expr(e)));
            }
            self.no_in = saved;
            if self.at_punct(";") {
                self.bump();
            } else {
                // `for (T x of/in e)`
                let kind = self.peek_for_kind();
                if kind.is_some() {
                    self.bump();
                    let rhs = self.expression()?;
                    self.expect_punct(")")?;
                    let body = Box::new(self.statement_body()?);
                    let (vk, tg) = match head.map(|b| *b) {
                        Some(Stmt::Var(k, mut v)) => {
                            if k == VarKind::Const && false {
                                (k, v.remove(0).0)
                            } else {
                                (k, v.remove(0).0)
                            }
                        }
                        Some(Stmt::Expr(e)) => {
                            if let Some(t) = target_of(&e) {
                                (VarKind::Var, t)
                            } else {
                                return Err(self.err("invalid left-hand side in for-of"));
                            }
                        }
                        _ => return Err(self.err("invalid for head")),
                    };
                    let fk = match kind.unwrap() {
                        "in" => ForKind::In,
                        _ if is_await => ForKind::AwaitOf,
                        _ => ForKind::Of,
                    };
                    return Ok(Stmt::ForInOf {
                        kind: fk,
                        decl: Some((vk, tg)),
                        expr: rhs,
                        body,
                    });
                }
                self.no_in = saved;
            }
        }

        let test = if self.at_punct(";") {
            self.bump();
            None
        } else {
            let e = self.expression()?;
            if !self.eat_punct(";") {
                return Err(self.err(&format!("expected `;`, found {}", self.cur().describe())));
            }
            Some(e)
        };
        let update = if self.at_punct(")") {
            None
        } else {
            Some(self.expression()?)
        };
        self.expect_punct(")")?;
        let body = Box::new(self.statement_body()?);
        Ok(Stmt::For(head, test, update, body))
    }

    fn peek_for_kind(&self) -> Option<&'static str> {
        if self.at_kw("of") {
            return Some("of");
        }
        if self.at_kw("in") {
            return Some("in");
        }
        None
    }

    fn try_statement(&mut self) -> Result<Stmt, String> {
        self.bump();
        let body = Box::new(self.block()?);
        let mut catch = None;
        let mut finally = None;
        if self.eat_kw("catch") {
            let param = if self.eat_punct("(") {
                let t = self.binding_target()?;
                self.expect_punct(")")?;
                Some(t)
            } else {
                None
            };
            let cbody = Box::new(self.block()?);
            catch = Some((param.unwrap_or(Target::Simple(String::new())), cbody));
        }
        if self.eat_kw("finally") {
            finally = Some(Box::new(self.block()?));
        }
        Ok(Stmt::Try(body, catch, finally))
    }

    fn switch_statement(&mut self) -> Result<Stmt, String> {
        self.bump();
        self.expect_punct("(")?;
        let d = self.expression()?;
        self.expect_punct(")")?;
        self.expect_punct("{")?;
        let mut cases = Vec::new();
        while !self.at_punct("}") && !self.at_end() {
            if self.eat_kw("case") {
                let t = self.expression()?;
                self.expect_punct(":")?;
                let mut body = Vec::new();
                while !self.at_kw("case") && !self.at_kw("default") && !self.at_punct("}") {
                    body.push(self.statement()?);
                }
                cases.push((Some(t), body));
            } else if self.eat_kw("default") {
                self.expect_punct(":")?;
                let mut body = Vec::new();
                while !self.at_kw("case") && !self.at_kw("default") && !self.at_punct("}") {
                    body.push(self.statement()?);
                }
                cases.push((None, body));
            } else {
                return Err(self.err("expected `case` or `default`"));
            }
        }
        self.expect_punct("}")?;
        Ok(Stmt::Switch(d, cases))
    }
}

fn name_unused() {}

/// Convert an already-parsed expression into a destructuring target, when legal.
fn target_of(e: &Expr) -> Option<Target> {
    match e {
        Expr::Ident(s) => Some(Target::Simple(s.clone())),
        Expr::Member { .. } => Some(Target::Member(Box::new(e.clone()))),
        _ => None,
    }
}

/// Extract a `PropKey`-style plain name from a member expression.
pub fn member_name(e: &Expr) -> Option<String> {
    match e {
        Expr::Member {
            key: MemberKey::Dot(n),
            ..
        } => Some(n.clone()),
        _ => None,
    }
}

/// Placeholder so the module advertises a stable helper for later callers.
pub fn dummy_key() -> PropKey {
    PropKey::Sym(0)
}

// --------------------------------------------------------------------------
// functions, classes, patterns
// --------------------------------------------------------------------------

impl Parser {
    fn is_async_fn(&self) -> bool {
        self.at_kw("async") && matches!(self.nth(1), Tok::Keyword("function")) && !self.newline_before()
    }
    /// Is `async` an async-function *prefix*? True for `async function`,
    /// `async (` and `async ident =>`. A bare `async` used as an identifier
    /// (or followed by anything else) stays an identifier.
    fn is_async_prefix(&self) -> bool {
        if !self.at_kw("async") || self.newline_before() {
            return false;
        }
        match self.nth(1) {
            Tok::Keyword("function") | Tok::Punct("(") => true,
            _ => matches!(self.nth(2), Tok::Punct("=>")),
        }
    }

    fn function_after_async(&mut self) -> Result<Func, String> {
        // `function()` consumes the `function` keyword itself — bumping here
        // as well made it expect a second `function` and fail on the name.
        let mut f = self.function(false, true)?;
        f.is_async = true;
        Ok(f)
    }

    fn function(&mut self, _in_expr: bool, _async: bool) -> Result<Func, String> {
        self.expect_kw("function")?;
        let mut is_generator = false;
        if self.at_punct("*") {
            self.bump();
            is_generator = true;
        }
        let name = if self.is_ident() { Some(self.ident_name()?) } else { None };
        let params = self.params()?;
        let body = self.function_body()?;
        Ok(Func {
            name,
            params,
            body,
            is_arrow: false,
            is_async: false,
            is_generator,
            expr_body: None,
        })
    }

    fn params(&mut self) -> Result<Vec<Param>, String> {
        self.expect_punct("(")?;
        let mut out = Vec::new();
        while !self.at_punct(")") {
            let rest = self.eat_punct("...");
            let target = self.binding_target()?;
            let default = if self.eat_punct("=") {
                Some(Box::new(self.assignment_expr()?))
            } else {
                None
            };
            out.push(Param { target, default, rest });
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct(")")?;
        Ok(out)
    }

    fn function_body(&mut self) -> Result<Vec<Stmt>, String> {
        self.expect_punct("{")?;
        let mut out = Vec::new();
        while !self.at_punct("}") && !self.at_end() {
            out.push(self.statement()?);
        }
        // A comment sitting just before the `}` never reaches `statement()`.
        self.drain_comments(&mut out);
        self.expect_punct("}")?;
        Ok(out)
    }

    /// An assignment/destructuring target.
    fn binding_target(&mut self) -> Result<Target, String> {
        if self.at_punct("[") {
            return self.array_pattern();
        }
        if self.at_punct("{") {
            return self.object_pattern();
        }
        // `undefined` is a plain global, not a reserved word, so declaring one
        // is legal — and common in defensive libraries (`var undefined;` appears
        // in lodash, qs and get-intrinsic). Reading it as the value made the
        // declaration an invalid binding target.
        if self.at_kw("undefined") {
            self.bump();
            return Ok(Target::Simple("undefined".to_string()));
        }
        let t = self.assign_target_expr()?;
        match t {
            Expr::Ident(s) => Ok(Target::Simple(s)),
            Expr::Member { .. } => Ok(Target::Member(Box::new(t))),
            _ => Err(self.err("invalid binding target")),
        }
    }

    fn assign_target_expr(&mut self) -> Result<Expr, String> {
        self.parse_unary()
    }

    fn array_pattern(&mut self) -> Result<Target, String> {
        self.expect_punct("[")?;
        let mut items: Vec<Option<Target>> = Vec::new();
        while !self.at_punct("]") {
            if self.at_punct(",") {
                self.bump();
                items.push(None);
                continue;
            }
            if self.eat_punct("...") {
                let t = self.binding_target()?;
                items.push(Some(Target::Rest {
                    from: items.len() as u32,
                    target: Box::new(t),
                }));
                break;
            }
            let mut t = self.binding_target()?;
            if self.eat_punct("=") {
                let d = self.assignment_expr()?;
                t = Target::WithDefault(Box::new(t), Box::new(d));
            }
            items.push(Some(t));
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct("]")?;
        Ok(Target::Array(items))
    }

    fn object_pattern(&mut self) -> Result<Target, String> {
        self.expect_punct("{")?;
        let mut fields = Vec::new();
        while !self.at_punct("}") {
            if self.eat_punct("...") {
                let t = self.binding_target()?;
                fields.push(ObjPatField {
                    key: MemberKey::Dot(String::new()),
                    target: Some(Box::new(t)),
                    default: None,
                    rest: true,
                });
                break;
            }
            let key = if self.at_punct("[") {
                self.bump();
                let e = self.assignment_expr()?;
                self.expect_punct("]")?;
                MemberKey::Computed(Box::new(e))
            } else {
                let name = self.property_key_name()?;
                MemberKey::Dot(name)
            };
            let (target, default) = if self.eat_punct(":") {
                let t = self.binding_target()?;
                let d = if self.eat_punct("=") {
                    Some(Box::new(self.assignment_expr()?))
                } else {
                    None
                };
                (Some(Box::new(t)), d)
            } else if self.eat_punct("=") {
                let d = self.assignment_expr()?;
                if let MemberKey::Dot(n) = &key {
                    (
                        Some(Box::new(Target::Simple(n.clone()))),
                        Some(Box::new(d)),
                    )
                } else {
                    (None, Some(Box::new(d)))
                }
            } else {
                (None, None)
            };
            fields.push(ObjPatField {
                key,
                target,
                default,
                rest: false,
            });
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct("}")?;
        Ok(Target::Object(fields))
    }

    fn property_key_name(&mut self) -> Result<String, String> {
        match self.cur().clone() {
            Tok::Str(s) => {
                self.bump();
                Ok(s)
            }
            Tok::Num(n) => {
                self.bump();
                Ok(n.to_string())
            }
            Tok::Ident(s) => {
                self.bump();
                Ok(s)
            }
            Tok::Keyword(k) => {
                self.bump();
                Ok(k.to_string())
            }
            t => Err(self.err(&format!("expected a property name, found {}", t.describe()))),
        }
    }

    fn class_decl(&mut self) -> Result<ClassDef, String> {
        self.expect_kw("class")?;
        let name = if self.is_ident() && !self.at_kw("extends") && !self.at_punct("{") {
            Some(self.ident_name()?)
        } else {
            None
        };
        let superclass = if self.eat_kw("extends") {
            Some(Box::new(self.parse_unary()?))
        } else {
            None
        };
        self.expect_punct("{")?;
        let mut members = Vec::new();
        while !self.at_punct("}") && !self.at_end() {
            self.eat_punct(";");
            if self.at_punct("}") {
                break;
            }
            let mut is_static = false;
            if self.at_kw("static") && !self.at_punct2("(") && !self.at_punct2("=") {
                self.bump();
                if self.at_punct("{") {
                    // static initialisation block: not supported, skip it
                    let mut depth = 0;
                    loop {
                        if self.at_punct("{") {
                            depth += 1;
                        } else if self.at_punct("}") {
                            depth -= 1;
                            if depth == 0 {
                                self.bump();
                                break;
                            }
                        }
                        self.bump();
                    }
                    continue;
                }
                is_static = true;
            }
            let member = self.class_member(is_static)?;
            members.push(member);
        }
        self.expect_punct("}")?;
        Ok(ClassDef { name, superclass, members })
    }

    fn class_member(&mut self, is_static: bool) -> Result<ClassMember, String> {
        let mut is_get = false;
        let mut is_set = false;
        // `get`/`set` followed by something that is not `(`/`=`/`;`
        if self.at_kw("get") && !self.at_punct2("(") && !self.at_punct2("=") && !self.at_punct2(";") {
            self.bump();
            is_get = true;
        } else if self.at_kw("set") && !self.at_punct2("(") && !self.at_punct2("=") && !self.at_punct2(";") {
            self.bump();
            is_set = true;
        }
        let mut is_async_method = false;
        // `async` before a method name — including when the member starts a
        // fresh line, which is normal in class/object bodies. Guarded on what
        // follows so `{ async: 1 }` still parses as a plain keyword key.
        if self.at_kw("async")
            && !self.at_punct2(":")
            && !self.at_punct2(",")
            && !self.at_punct2("(")
            && !self.at_punct2("=")
        {
            self.bump();
            is_async_method = true;
        }
        let mut is_generator = false;
        if self.at_punct("*") {
            self.bump();
            is_generator = true;
        }

        let key = if self.at_punct("[") {
            self.bump();
            let e = self.assignment_expr()?;
            self.expect_punct("]")?;
            ClassKey::Computed(Box::new(e))
        } else if let Tok::Private(p) = self.cur().clone() {
            self.bump();
            ClassKey::Private(p)
        } else {
            ClassKey::Name(self.property_key_name()?)
        };

        if self.at_punct("(") {
            let params = self.params()?;
            let body = self.function_body()?;
            let f = Func {
                name: None,
                params,
                body,
                is_arrow: false,
                is_async: is_async_method,
                is_generator,
                expr_body: None,
            };
            let is_method = !is_get && !is_set;
            let _ = is_method;
            // Getters/setters and static/instance distinction are carried in
            // `ClassMemberKind`; a `get`/`set` marker rides along in the name.
            let name = match &key {
                ClassKey::Name(n) => n.clone(),
                ClassKey::Private(n) => format!("#{}", n),
                ClassKey::Computed(_) => String::new(),
            };
            let marker = if is_get {
                format!("\u{1}g:{}", name)
            } else if is_set {
                format!("\u{1}s:{}", name)
            } else {
                String::new()
            };
            let kind = ClassMemberKind::Method(
                Box::new(Func {
                    name: if marker.is_empty() { None } else { Some(marker) },
                    ..f
                }),
                is_static,
            );
            return Ok(ClassMember { key, kind });
        }

        // Field declaration.
        let init = if self.eat_punct("=") {
            self.assignment_expr()?
        } else {
            Expr::Undefined
        };
        self.semi()?;
        let kind = if is_static {
            ClassMemberKind::StaticField(Box::new(init))
        } else {
            ClassMemberKind::Field(Box::new(init))
        };
        Ok(ClassMember { key, kind })
    }
}

// --------------------------------------------------------------------------
// expressions (Pratt)
// --------------------------------------------------------------------------

impl Parser {
    /// `expr, expr, ...`
    pub fn expression(&mut self) -> Result<Expr, String> {
        let first = self.assignment_expr()?;
        if !self.at_punct(",") {
            return Ok(first);
        }
        let mut items = vec![first];
        while self.eat_punct(",") {
            items.push(self.assignment_expr()?);
        }
        Ok(Expr::Seq(items))
    }

    fn assignment_expr(&mut self) -> Result<Expr, String> {
        // Arrow functions are looked for first; they can start with `(`.
        if let Some(e) = self.try_arrow()? {
            return Ok(e);
        }
        let left = self.conditional()?;

        let op = match self.cur() {
            Tok::Punct(p) if is_assign_op(p) => p.clone(),
            _ => return Ok(left),
        };
        // `=` must follow directly (no newline) for an assignment expression.
        if op == "=" && self.newline_before() {
            return Ok(left);
        }
        let target = match to_target(&left) {
            Some(t) => t,
            None => return Err(self.err("invalid assignment target")),
        };
        self.bump();
        let value = self.assignment_expr()?;
        let aop = match op {
            "=" => AssignOp::Assign,
            "+=" => AssignOp::AddAssign,
            "-=" => AssignOp::SubAssign,
            "*=" => AssignOp::MulAssign,
            "/=" => AssignOp::DivAssign,
            "%=" => AssignOp::ModAssign,
            "**=" => AssignOp::PowAssign,
            "<<=" => AssignOp::ShlAssign,
            ">>=" => AssignOp::ShrAssign,
            ">>>=" => AssignOp::UShrAssign,
            "&=" => AssignOp::BitAndAssign,
            "|=" => AssignOp::BitOrAssign,
            "^=" => AssignOp::BitXorAssign,
            "&&=" => AssignOp::AndAssign,
            "||=" => AssignOp::OrAssign,
            "??=" => AssignOp::NullishAssign,
            _ => AssignOp::Assign,
        };
        Ok(Expr::Assign(aop, Box::new(target), Box::new(value)))
    }

    fn conditional(&mut self) -> Result<Expr, String> {
        let c = self.parse_binary(1)?;
        if !self.eat_punct("?") {
            return Ok(c);
        }
        let a = self.assignment_expr()?;
        self.expect_punct(":")?;
        let b = self.assignment_expr()?;
        Ok(Expr::Cond(Box::new(c), Box::new(a), Box::new(b)))
    }

    fn parse_binary(&mut self, min_bp: u8) -> Result<Expr, String> {
        let mut left = self.parse_unary()?;
        loop {
            let (op, lbp, rbp) = match self.binary_op() {
                Some(x) => x,
                None => break,
            };
            if lbp < min_bp {
                break;
            }
            self.bump();
            let right = self.parse_binary(rbp)?;
            left = make_binary(op, left, right);
        }
        Ok(left)
    }

    /// `(op, left_bp, right_bp)`, or `None` when the token cannot continue a
    /// binary expression.
    fn binary_op(&self) -> Option<(BinaryKind, u8, u8)> {
        let t = self.cur();
        let p = match t {
            Tok::Punct(p) => *p,
            _ => {
                if let Tok::Keyword(k) = t {
                    match *k {
                        "instanceof" => return Some((BinaryKind::Op(BinOp::InstanceOf), 15, 16)),
                        // The `no_in` flag was stored but never consulted, so
                        // `for (i in obj)` swallowed the `in` into a binary
                        // expression and every for-in loop without a `let`
                        // declaration failed to parse.
                        "in" if !self.no_in => {
                            return Some((BinaryKind::Op(BinOp::In), 15, 16))
                        }
                        "in" => return None,
                        _ => return None,
                    }
                }
                return None;
            }
        };
        // Binding powers: for a *left*-associative operator the right operand is
        // parsed with `bp + 1`, so an operator of the same precedence cannot be
        // consumed again on the right. `**` and assignment-style operators are
        // the right-associative exceptions.
        let pair: (BinaryKind, u8, u8) = match p {
            "??" => (BinaryKind::Nullish, 1, 2),
            "||" => (BinaryKind::Or, 3, 4),
            "&&" => (BinaryKind::And, 5, 6),
            "|" => (BinaryKind::Op(BinOp::BitOr), 7, 8),
            "^" => (BinaryKind::Op(BinOp::BitXor), 9, 10),
            "&" => (BinaryKind::Op(BinOp::BitAnd), 11, 12),
            "==" => (BinaryKind::Op(BinOp::EqEq), 13, 14),
            "!=" => (BinaryKind::Op(BinOp::NotEq), 13, 14),
            "===" => (BinaryKind::Op(BinOp::StrictEq), 13, 14),
            "!==" => (BinaryKind::Op(BinOp::StrictNotEq), 13, 14),
            "<" => (BinaryKind::Op(BinOp::Lt), 15, 16),
            ">" => (BinaryKind::Op(BinOp::Gt), 15, 16),
            "<=" => (BinaryKind::Op(BinOp::Le), 15, 16),
            ">=" => (BinaryKind::Op(BinOp::Ge), 15, 16),
            "<<" => (BinaryKind::Op(BinOp::Shl), 17, 18),
            ">>" => (BinaryKind::Op(BinOp::Shr), 17, 18),
            ">>>" => (BinaryKind::Op(BinOp::UShr), 17, 18),
            "+" => (BinaryKind::Op(BinOp::Add), 19, 20),
            "-" => (BinaryKind::Op(BinOp::Sub), 19, 20),
            "*" => (BinaryKind::Op(BinOp::Mul), 21, 22),
            "/" => (BinaryKind::Op(BinOp::Div), 21, 22),
            "%" => (BinaryKind::Op(BinOp::Mod), 21, 22),
            // Right-associative: the right operand keeps the same bp.
            "**" => (BinaryKind::Op(BinOp::Pow), 23, 23),
            _ => return None,
        };
        Some(pair)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        if let Tok::Punct(p) = self.cur() {
            match *p {
                "!" => {
                    self.bump();
                    return Ok(Expr::Unary(UnOp::Not, Box::new(self.parse_unary()?)));
                }
                "~" => {
                    self.bump();
                    return Ok(Expr::Unary(UnOp::BitNot, Box::new(self.parse_unary()?)));
                }
                "+" => {
                    self.bump();
                    return Ok(Expr::Unary(UnOp::Plus, Box::new(self.parse_unary()?)));
                }
                "-" => {
                    self.bump();
                    return Ok(Expr::Unary(UnOp::Neg, Box::new(self.parse_unary()?)));
                }
                "++" | "--" => {
                    let op = if *p == "--" { UpdOp::Dec } else { UpdOp::Inc };
                    self.bump();
                    return Ok(Expr::Update(true, op, Box::new(self.parse_unary()?)));
                }
                _ => {}
            }
        }
        if let Tok::Keyword(k) = self.cur() {
            match *k {
                "typeof" => {
                    self.bump();
                    return Ok(Expr::Unary(UnOp::TypeOf, Box::new(self.parse_unary()?)));
                }
                "void" => {
                    self.bump();
                    return Ok(Expr::Unary(UnOp::Void, Box::new(self.parse_unary()?)));
                }
                "delete" => {
                    self.bump();
                    return Ok(Expr::Unary(UnOp::Delete, Box::new(self.parse_unary()?)));
                }
                "await" => {
                    self.bump();
                    // Kept distinct from `void` so `jhol fmt` prints it back.
                    return Ok(Expr::Unary(UnOp::Await, Box::new(self.parse_unary()?)));
                }
                "yield" => {
                    self.bump();
                    let d = self.eat_punct("*");
                    if self.at_punct(")") || self.at_punct("]") || self.at_punct("}") || self.at_punct(";") || self.newline_before() {
                        return Ok(Expr::Yield(None, d));
                    }
                    let e = self.assignment_expr()?;
                    return Ok(Expr::Yield(Some(Box::new(e)), d));
                }
                _ => {}
            }
        }
        let mut e = self.parse_postfix()?;
        loop {
            if self.at_punct("++") && !self.newline_before() {
                self.bump();
                e = Expr::Update(false, UpdOp::Inc, Box::new(e));
                continue;
            }
            if self.at_punct("--") && !self.newline_before() {
                self.bump();
                e = Expr::Update(false, UpdOp::Dec, Box::new(e));
                continue;
            }
            break;
        }
        Ok(e)
    }

    fn parse_postfix(&mut self) -> Result<Expr, String> {
        let e = self.parse_primary()?;
        self.parse_member_tail(e)
    }

    /// Member / call / new tail, including optional chaining and tagged templates.
    fn parse_member_tail(&mut self, mut e: Expr) -> Result<Expr, String> {
        loop {
            if self.at_punct(".") {
                self.bump();
                let name = if let Tok::Private(p) = self.cur().clone() {
                    self.bump();
                    format!("#{}", p)
                } else {
                    self.property_key_name()?
                };
                e = Expr::Member {
                    obj: Box::new(e),
                    key: MemberKey::Dot(name),
                    optional: false,
                };
                continue;
            }
            if self.at_punct("?.") {
                self.bump();
                if self.at_punct("(") {
                    let args = self.arguments()?;
                    e = Expr::Call {
                        callee: Box::new(e),
                        args,
                        optional: true,
                    };
                    continue;
                }
                if self.at_punct("[") {
                    self.bump();
                    let k = self.expression()?;
                    self.expect_punct("]")?;
                    e = Expr::Member {
                        obj: Box::new(e),
                        key: MemberKey::Computed(Box::new(k)),
                        optional: true,
                    };
                    continue;
                }
                let name = self.property_key_name()?;
                e = Expr::Member {
                    obj: Box::new(e),
                    key: MemberKey::Dot(name),
                    optional: true,
                };
                continue;
            }
            if self.at_punct("[") {
                self.bump();
                let k = self.expression()?;
                self.expect_punct("]")?;
                e = Expr::Member {
                    obj: Box::new(e),
                    key: MemberKey::Computed(Box::new(k)),
                    optional: false,
                };
                continue;
            }
            if self.at_punct("(") {
                let args = self.arguments()?;
                e = Expr::Call {
                    callee: Box::new(e),
                    args,
                    optional: false,
                };
                continue;
            }
            // Tagged template: only when a template directly follows a member
            // expression or a bare identifier/`)`/`]`.
            if matches!(self.cur(), Tok::Template(_)) {
                // `take_template` already consumes the token, so pairing it with
                // `template_parts` (which bumps again) would swallow the token
                // after the template.
                let raw = self.take_template()?;
                let parts = self.template_parts_from(raw)?;
                return Ok(Expr::TaggedTemplate {
                    tag: Box::new(e),
                    parts,
                });
            }
            break;
        }
        Ok(e)
    }

    fn arguments(&mut self) -> Result<Vec<Expr>, String> {
        self.expect_punct("(")?;
        let mut out = Vec::new();
        while !self.at_punct(")") {
            if self.eat_punct("...") {
                out.push(Expr::Spread(Box::new(self.assignment_expr()?)));
            } else {
                out.push(self.assignment_expr()?);
            }
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct(")")?;
        Ok(out)
    }

    /// Consume the current template token and return its raw parts.
    fn take_template(&mut self) -> Result<Vec<TplPart>, String> {
        match self.bump() {
            Tok::Template(p) => Ok(p),
            _ => Err(self.err("expected a template literal")),
        }
    }

    /// Turn the lexer's `TplPart::Raw` holes into real expressions.
    pub fn template_parts(&mut self) -> Result<Vec<TplPart>, String> {
        let raw = self.take_template()?;
        self.template_parts_from(raw)
    }

    pub fn template_parts_from(&mut self, raw: Vec<TplPart>) -> Result<Vec<TplPart>, String> {
        let mut out = Vec::new();
        for part in raw {
            out.push(match part {
                TplPart::Str(s) => TplPart::Str(s),
                TplPart::Raw(src) => {
                    let mut sub = Parser::new(&src).map_err(|e| {
                        format!("SyntaxError: line {}: in template interpolation: {}", self.cur_line(), e)
                    })?;
                    let e = sub.parse_expression_only()?;
                    TplPart::Expr(Box::new(e))
                }
                TplPart::Expr(e) => TplPart::Expr(e),
            });
        }
        Ok(out)
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        let t = self.cur().clone();
        match t {
            Tok::Num(n) => {
                self.bump();
                Ok(Expr::Num(n))
            }
            Tok::BigInt(n) => {
                self.bump();
                Ok(Expr::BigInt(n))
            }
            Tok::Str(s) => {
                self.bump();
                Ok(Expr::Str(s))
            }
            Tok::Str16(s) => { self.bump(); Ok(Expr::Str16(s)) }
            Tok::Regex(b, f) => {
                self.bump();
                Ok(Expr::Regex(b, f))
            }
            Tok::Private(p) => {
                self.bump();
                Ok(Expr::Ident(format!("#{}", p)))
            }
            Tok::Template(_) => {
                let raw = self.take_template()?;
                let parts = self.template_parts_from(raw)?;
                Ok(Expr::Template(parts))
            }
            Tok::Ident(name) => {
                self.bump();
                match name.as_str() {
                    "function" if matches!(self.cur(), Tok::Keyword("function")) => {}
                    _ => {}
                }
                Ok(Expr::Ident(name))
            }
            Tok::Keyword(k) => match k {
                "true" => {
                    self.bump();
                    Ok(Expr::Bool(true))
                }
                "false" => {
                    self.bump();
                    Ok(Expr::Bool(false))
                }
                "null" => {
                    self.bump();
                    Ok(Expr::Null)
                }
                "undefined" => {
                    self.bump();
                    Ok(Expr::Undefined)
                }
                "this" => {
                    self.bump();
                    Ok(Expr::This)
                }
                "super" => {
                    self.bump();
                    Ok(Expr::Super)
                }
                "function" => {
                    self.bump();
                    self.function_expr()
                }
                "class" => {
                    self.bump();
                    let c = self.class_body_after_keyword()?;
                    Ok(Expr::Class(Box::new(c)))
                }
                "new" => {
                    self.bump();
                    if self.at_kw("new") {
                        // `new new A()()`
                    }
                    if self.at_punct(".") {
                        self.bump();
                        let _ = self.ident_name()?;
                        return Ok(Expr::NewTarget);
                    }
                    // The callee of `new` is a member expression *without* calls:
                    // `new A.B(1)` constructs `A.B`, while `new (A.B(1))`
                    // constructs the result of a call.
                    let mut callee = self.parse_primary()?;
                    loop {
                        if self.eat_punct(".") {
                            let name = self.property_key_name()?;
                            callee = Expr::Member {
                                obj: Box::new(callee),
                                key: MemberKey::Dot(name),
                                optional: false,
                            };
                            continue;
                        }
                        if self.at_punct("[") {
                            self.bump();
                            let k = self.expression()?;
                            self.expect_punct("]")?;
                            callee = Expr::Member {
                                obj: Box::new(callee),
                                key: MemberKey::Computed(Box::new(k)),
                                optional: false,
                            };
                            continue;
                        }
                        break;
                    }
                    let args = if self.at_punct("(") {
                        self.arguments()?
                    } else {
                        Vec::new()
                    };
                    Ok(Expr::New(Box::new(callee), args))
                }
                "typeof" | "void" | "delete" | "await" | "yield" => {
                    let e = self.parse_unary()?;
                    Ok(e)
                }
                // `async` is only a keyword when it actually introduces an
                // async function or arrow. A variable named `async` — which
                // real packages do constantly, `var async = require("async")` —
                // must parse as the identifier it is.
                "async" if self.is_async_prefix() => {
                    self.bump();
                    if self.at_kw("function") {
                        let mut f = self.function(false, false)?;
                        f.is_async = true;
                        return Ok(Expr::Func(Box::new(f)));
                    }
                    self.parse_primary()
                }
                "get" | "set" | "static" | "of" | "in" | "let" | "const" | "var" | "async" => {
                    self.bump();
                    Ok(Expr::Ident(k.to_string()))
                }
                _ => {
                    self.bump();
                    Ok(Expr::Ident(k.to_string()))
                }
            },
            Tok::Punct(p) => match p {
                "(" => {
                    self.bump();
                    let e = self.expression()?;
                    self.expect_punct(")")?;
                    Ok(e)
                }
                "[" => self.array_literal(),
                "{" => self.object_literal(),
                _ => Err(self.err(&format!("unexpected {}", t.describe()))),
            },
            Tok::Eof => Err(self.err("unexpected end of input")),
        }
    }

    fn function_expr(&mut self) -> Result<Expr, String> {
        let mut is_generator = false;
        if self.at_punct("*") {
            self.bump();
            is_generator = true;
        }
        let name = if self.is_ident() { Some(self.ident_name()?) } else { None };
        let params = self.params()?;
        let body = self.function_body()?;
        Ok(Expr::Func(Box::new(Func {
            name,
            params,
            body,
            is_arrow: false,
            is_async: false,
            is_generator,
            expr_body: None,
        })))
    }

    fn class_body_after_keyword(&mut self) -> Result<ClassDef, String> {
        let name = if self.is_ident() { Some(self.ident_name()?) } else { None };
        let superclass = if self.eat_kw("extends") {
            Some(Box::new(self.parse_unary()?))
        } else {
            None
        };
        self.expect_punct("{")?;
        let mut members = Vec::new();
        while !self.at_punct("}") && !self.at_end() {
            self.eat_punct(";");
            if self.at_punct("}") {
                break;
            }
            let mut is_static = false;
            if self.at_kw("static") && !self.at_punct2("(") && !self.at_punct2("=") {
                self.bump();
                if self.at_punct("{") {
                    let mut depth = 0;
                    loop {
                        if self.at_punct("{") {
                            depth += 1;
                        } else if self.at_punct("}") {
                            depth -= 1;
                            if depth == 0 {
                                self.bump();
                                break;
                            }
                        }
                        self.bump();
                    }
                    continue;
                }
                is_static = true;
            }
            members.push(self.class_member(is_static)?);
        }
        self.expect_punct("}")?;
        Ok(ClassDef { name, superclass, members })
    }

    fn array_literal(&mut self) -> Result<Expr, String> {
        self.expect_punct("[")?;
        let mut items: Vec<Option<Expr>> = Vec::new();
        while !self.at_punct("]") {
            if self.at_punct(",") {
                self.bump();
                items.push(None);
                continue;
            }
            if self.eat_punct("...") {
                items.push(Some(Expr::Spread(Box::new(self.assignment_expr()?))));
            } else {
                items.push(Some(self.assignment_expr()?));
            }
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct("]")?;
        Ok(Expr::Array(items))
    }

    fn object_literal(&mut self) -> Result<Expr, String> {
        self.expect_punct("{")?;
        let mut props = Vec::new();
        while !self.at_punct("}") {
            if self.eat_punct("...") {
                props.push(ObjProp::Spread(Box::new(self.assignment_expr()?)));
                if !self.eat_punct(",") {
                    break;
                }
                continue;
            }
            let mut is_get = false;
            let mut is_set = false;
            if (self.at_kw("get") || self.at_kw("set"))
                && !self.at_punct2("(")
                && !self.at_punct2(":")
                && !self.at_punct2(",")
            {
                if self.at_kw("get") {
                    is_get = true;
                } else {
                    is_set = true;
                }
                self.bump();
            }
            let mut is_async = false;
            // Same rule as class members: an `async` entering on a fresh line is
            // still the modifier, not an identifier.
            if self.at_kw("async")
                && !self.at_punct2(":")
                && !self.at_punct2(",")
                && !self.at_punct2("(")
                && !self.at_punct2("=")
            {
                self.bump();
                is_async = true;
            }
            let mut is_gen = false;
            if self.at_punct("*") {
                self.bump();
                is_gen = true;
            }

            let key = if self.at_punct("[") {
                self.bump();
                let e = self.assignment_expr()?;
                self.expect_punct("]")?;
                MemberKey::Computed(Box::new(e))
            } else {
                MemberKey::Dot(self.property_key_name()?)
            };

            if self.at_punct("(") || is_get || is_set {
                let params = self.params()?;
                let body = self.function_body()?;
                let kind = if is_get {
                    MethodKind::Get
                } else if is_set {
                    MethodKind::Set
                } else {
                    MethodKind::Method
                };
                props.push(ObjProp::Method {
                    key,
                    func: Box::new(Func {
                        name: None,
                        params,
                        body,
                        is_arrow: false,
                        is_async: is_async,
                        is_generator: is_gen,
                        expr_body: None,
                    }),
                    kind,
                });
            } else if self.eat_punct(":") {
                let v = self.assignment_expr()?;
                props.push(ObjProp::Init(key, Box::new(v)));
            } else if self.eat_punct("=") {
                let v = self.assignment_expr()?;
                if let MemberKey::Dot(n) = &key {
                    props.push(ObjProp::Init(
                        MemberKey::Dot(n.clone()),
                        Box::new(Expr::Cond(
                            Box::new(Expr::Ident(n.clone())),
                            Box::new(Expr::Ident(n.clone())),
                            Box::new(v),
                        )),
                    ));
                } else {
                    props.push(ObjProp::Init(key, Box::new(v)));
                }
            } else if let MemberKey::Dot(n) = &key {
                props.push(ObjProp::Shorthand(n.clone()));
            }
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct("}")?;
        Ok(Expr::Object(props))
    }

    /// Try to parse an arrow function; restore the cursor on failure.
    fn try_arrow(&mut self) -> Result<Option<Expr>, String> {
        let is_async = self.is_async_prefix();
        if !(self.at_punct("(") || self.is_ident()) && !is_async {
            return Ok(None);
        }
        let start = self.i;
        let result = (|| -> Result<Option<Expr>, String> {
            let mut is_async_flag = false;
            if is_async {
                self.bump();
                is_async_flag = true;
                if self.newline_before() {
                    return Ok(None);
                }
            }
            // A lone identifier followed by `=>` needs no parameter scan.
            if self.is_ident() && self.at_punct2("=>") {
                let name = self.ident_name()?;
                self.bump(); // =>
                return Ok(Some(self.finish_arrow(vec![Param {
                    target: Target::Simple(name),
                    default: None,
                    rest: false,
                }], is_async_flag)?));
            }
            if !self.at_punct("(") {
                return Ok(None);
            }
            // Scan the parenthesised part looking for `=>` right after the closing `)`.
            let mut depth = 0usize;
            let mut j = self.i;
            let mut found = None;
            while j < self.toks.len() {
                match &self.toks[j].0 {
                    Tok::Punct("(") | Tok::Punct("[") | Tok::Punct("{") => depth += 1,
                    Tok::Punct(")") | Tok::Punct("]") | Tok::Punct("}") => {
                        if depth == 0 {
                            break;
                        }
                        depth -= 1;
                        if depth == 0 {
                            if j + 1 < self.toks.len() && matches!(&self.toks[j + 1].0, Tok::Punct("=>")) {
                                found = Some(j + 1);
                            }
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            if found.is_none() {
                return Ok(None);
            }
            let params = self.params()?;
            if !self.eat_punct("=>") {
                return Ok(None);
            }
            Ok(Some(self.finish_arrow(params, is_async_flag)?))
        })();
        match result {
            Ok(Some(expr)) => Ok(Some(expr)),
            _ => {
                self.i = start;
                Ok(None)
            }
        }
    }

    fn finish_arrow(&mut self, params: Vec<Param>, is_async: bool) -> Result<Expr, String> {
        if self.at_punct("{") {
            let body = self.function_body()?;
            Ok(Expr::Arrow(Box::new(Func {
                name: None,
                params,
                body,
                is_arrow: true,
                is_async,
                is_generator: false,
                expr_body: None,
            })))
        } else {
            let e = self.assignment_expr()?;
            Ok(Expr::Arrow(Box::new(Func {
                name: None,
                params,
                body: Vec::new(),
                is_arrow: true,
                is_async,
                is_generator: false,
                expr_body: Some(Box::new(e)),
            })))
        }
    }
}

fn is_assign_op(p: &str) -> bool {
    matches!(
        p,
        "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "**=" | "<<=" | ">>=" | ">>>=" | "&=" | "|="
            | "^=" | "&&=" | "||=" | "??="
    )
}

/// A binary operator or a short-circuiting logical one, so the Pratt loop keeps
/// a single precedence table for both.
#[derive(Clone, Copy)]
enum BinaryKind {
    Op(BinOp),
    And,
    Or,
    Nullish,
}

fn make_binary(op: BinaryKind, l: Expr, r: Expr) -> Expr {
    match op {
        BinaryKind::And => Expr::Logical(LogicalOp::And, Box::new(l), Box::new(r)),
        BinaryKind::Or => Expr::Logical(LogicalOp::Or, Box::new(l), Box::new(r)),
        BinaryKind::Nullish => Expr::Logical(LogicalOp::Nullish, Box::new(l), Box::new(r)),
        BinaryKind::Op(o) => Expr::Binary(o, Box::new(l), Box::new(r)),
    }
}

fn to_target(e: &Expr) -> Option<Target> {
    match e {
        Expr::Ident(s) => Some(Target::Simple(s.clone())),
        Expr::Member { .. } => Some(Target::Member(Box::new(e.clone()))),
        Expr::Array(items) => Some(Target::Array(
            items
                .iter()
                .map(|o| o.as_ref().and_then(to_target))
                .collect(),
        )),
        Expr::Object(props) => {
            let mut fields = Vec::new();
            for p in props {
                match p {
                    ObjProp::Init(MemberKey::Dot(n), v) => {
                        let t = to_target(v)?;
                        fields.push(ObjPatField {
                            key: MemberKey::Dot(n.clone()),
                            target: Some(Box::new(t)),
                            default: None,
                            rest: false,
                        });
                    }
                    ObjProp::Shorthand(n) => fields.push(ObjPatField {
                        key: MemberKey::Dot(n.clone()),
                        target: None,
                        default: None,
                        rest: false,
                    }),
                    _ => return None,
                }
            }
            Some(Target::Object(fields))
        }
        _ => None,
    }
}
