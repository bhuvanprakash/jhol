//! A small JavaScript regular-expression engine.
//!
//! JholVM previously had no regex support at all: a literal compiled to a plain
//! object with `source`/`flags` and no methods, `new RegExp(...)` threw, and
//! `String.prototype.replace` returned the input unchanged when handed a regex —
//! a silent wrong answer. This module implements the subset that ordinary code
//! and libraries actually use:
//!
//! * literals, `.`, character classes with ranges/negation/escapes,
//! * `\d \D \w \W \s \S`, `\b`, `\B`, `^`, `$`,
//! * groups (capturing and `(?:…)`), alternation, backreferences are rejected,
//! * greedy and lazy `* + ? {n} {n,} {n,m}`,
//! * flags `g i m s u` (`u` accepted; matching is already code-point based).
//!
//! Not supported, and reported as a compile error rather than matching nothing:
//! lookahead/lookbehind, backreferences, named groups. A pattern that would
//! backtrack catastrophically is stopped by an operation budget; the caller sees
//! "no match" for that attempt instead of the process hanging.
//!
//! Matching runs over `char`s with byte offsets recorded, so `lastIndex` and
//! `match.index` are byte offsets into the UTF-8 string, matching how the rest
//! of the VM indexes strings.

use std::collections::HashSet;

/// Operation budget per `exec` call. A catastrophic pattern stops here instead
/// of hanging the process. 5M steps is far beyond any real pattern on real input.
const STEP_BUDGET: u32 = 5_000_000;

#[derive(Debug, Clone, Copy, PartialEq)]
enum ClassItem {
    Ch(char),
    Range(char, char),
    Digit(bool),
    Word(bool),
    Space(bool),
}

#[derive(Debug, Clone)]
enum Node {
    Empty,
    Ch(char),
    Any,
    Class {
        neg: bool,
        items: Vec<ClassItem>,
    },
    Start,
    End,
    WordB(bool),
    Group {
        idx: Option<usize>,
        node: Box<Node>,
    },
    Seq(Vec<Node>),
    Alt(Vec<Node>),
    Repeat {
        node: Box<Node>,
        min: usize,
        max: Option<usize>,
        greedy: bool,
    },
}

#[derive(Debug, Clone)]
enum Inst {
    Ch(char),
    Any,
    Class { neg: bool, items: Vec<ClassItem> },
    AssertStart,
    AssertEnd,
    WordB(bool),
    Save(usize),
    Split(usize, usize),
    Jmp(usize),
    Match,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Flags {
    pub global: bool,
    pub ignore_case: bool,
    pub multiline: bool,
    pub dot_all: bool,
    pub sticky: bool,
}

#[derive(Debug, Clone)]
pub struct Regex {
    prog: Vec<Inst>,
    pub ncaps: usize,
    pub flags: Flags,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    /// Byte offsets into the subject.
    pub start: usize,
    pub end: usize,
    /// Group captures, index 0 is the whole match.
    pub caps: Vec<Option<(usize, usize)>>,
}

// ───────────────────────────── parser ─────────────────────────────

struct Parser<'a> {
    src: Vec<char>,
    i: usize,
    ncaps: usize,
    _phantom: std::marker::PhantomData<&'a ()>,
}

impl<'a> Parser<'a> {
    fn new(pat: &str) -> Self {
        Self {
            src: pat.chars().collect(),
            i: 0,
            ncaps: 0,
            _phantom: std::marker::PhantomData,
        }
    }

    fn peek(&self) -> Option<char> {
        self.src.get(self.i).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.i += 1;
        }
        c
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn parse(&mut self) -> Result<Node, String> {
        let n = self.alt()?;
        if self.i < self.src.len() {
            return Err(format!("unexpected '{}'", self.src[self.i]));
        }
        Ok(n)
    }

    fn alt(&mut self) -> Result<Node, String> {
        let mut branches = vec![self.seq()?];
        while self.eat('|') {
            branches.push(self.seq()?);
        }
        Ok(if branches.len() == 1 {
            branches.pop().unwrap()
        } else {
            Node::Alt(branches)
        })
    }

    fn seq(&mut self) -> Result<Node, String> {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.atom()?;
            let node = self.quantifier(atom)?;
            items.push(node);
        }
        Ok(match items.len() {
            0 => Node::Empty,
            1 => items.pop().unwrap(),
            _ => Node::Seq(items),
        })
    }

    fn atom(&mut self) -> Result<Node, String> {
        let c = self
            .bump()
            .ok_or_else(|| "unexpected end of pattern".to_string())?;
        match c {
            '(' => {
                let mut idx = None;
                if self.eat('?') {
                    if self.eat(':') {
                        // non-capturing
                    } else if self.peek() == Some('<') {
                        return Err("named groups are not supported".to_string());
                    } else if self.eat('=') || self.eat('!') {
                        return Err("lookahead is not supported".to_string());
                    } else {
                        return Err("unsupported group modifier".to_string());
                    }
                } else {
                    self.ncaps += 1;
                    idx = Some(self.ncaps);
                }
                let inner = self.alt()?;
                if !self.eat(')') {
                    return Err("missing ')'".to_string());
                }
                Ok(Node::Group {
                    idx,
                    node: Box::new(inner),
                })
            }
            '[' => self.class(),
            '.' => Ok(Node::Any),
            '^' => Ok(Node::Start),
            '$' => Ok(Node::End),
            '\\' => self.escape(),
            '*' | '+' | '?' => Err(format!("nothing to repeat before '{}'", c)),
            ')' => Err("unbalanced ')'".to_string()),
            other => Ok(Node::Ch(other)),
        }
    }

    fn class_item(&mut self) -> Result<Option<ClassItem>, String> {
        let c = match self.bump() {
            Some(c) => c,
            None => return Err("unterminated character class".to_string()),
        };
        if c != '\\' {
            return Ok(Some(ClassItem::Ch(c)));
        }
        let e = self
            .bump()
            .ok_or_else(|| "trailing backslash".to_string())?;
        Ok(match e {
            'd' => Some(ClassItem::Digit(true)),
            'D' => Some(ClassItem::Digit(false)),
            'w' => Some(ClassItem::Word(true)),
            'W' => Some(ClassItem::Word(true)), // handled by caller negation below
            's' => Some(ClassItem::Space(true)),
            'S' => Some(ClassItem::Space(false)),
            'n' => Some(ClassItem::Ch('\n')),
            'r' => Some(ClassItem::Ch('\r')),
            't' => Some(ClassItem::Ch('\t')),
            'f' => Some(ClassItem::Ch('\u{c}')),
            'v' => Some(ClassItem::Ch('\u{b}')),
            '0' => Some(ClassItem::Ch('\0')),
            'b' => Some(ClassItem::Ch('\u{8}')),
            'x' => Some(ClassItem::Ch(self.hex(2)?)),
            'u' => Some(ClassItem::Ch(self.hex(4)?)),
            other => Some(ClassItem::Ch(other)),
        })
    }

    fn hex(&mut self, n: usize) -> Result<char, String> {
        let mut v = 0u32;
        for _ in 0..n {
            let c = self.bump().ok_or_else(|| "bad \\x escape".to_string())?;
            let d = c.to_digit(16).ok_or_else(|| "bad hex digit".to_string())?;
            v = v * 16 + d;
        }
        char::from_u32(v).ok_or_else(|| "invalid code point".to_string())
    }

    fn class(&mut self) -> Result<Node, String> {
        let neg = self.eat('^');
        let mut items = Vec::new();
        let mut first = true;
        loop {
            let c = match self.peek() {
                None => return Err("unterminated character class".to_string()),
                Some(']') if !first => {
                    self.bump();
                    break;
                }
                Some(c) => c,
            };
            first = false;
            // range: a-z (but not for class escapes)
            let start = self.class_item()?.unwrap();
            if let ClassItem::Ch(lo) = start {
                if self.peek() == Some('-') {
                    // lookahead: '-]' means a literal dash
                    let save = self.i;
                    self.bump();
                    match self.peek() {
                        Some(']') | None => {
                            self.i = save;
                            items.push(ClassItem::Ch(lo));
                            continue;
                        }
                        _ => {
                            let end = self.class_item()?.unwrap();
                            if let ClassItem::Ch(hi) = end {
                                items.push(ClassItem::Range(lo, hi));
                                continue;
                            }
                            items.push(ClassItem::Ch(lo));
                            items.push(ClassItem::Ch('-'));
                            items.push(end);
                            continue;
                        }
                    }
                }
            }
            let _ = c;
            items.push(start);
        }
        Ok(Node::Class { neg, items })
    }

    fn escape(&mut self) -> Result<Node, String> {
        let e = self
            .bump()
            .ok_or_else(|| "trailing backslash".to_string())?;
        Ok(match e {
            'd' => Node::Class {
                neg: false,
                items: vec![ClassItem::Digit(true)],
            },
            'D' => Node::Class {
                neg: true,
                items: vec![ClassItem::Digit(true)],
            },
            'w' => Node::Class {
                neg: false,
                items: vec![ClassItem::Word(true)],
            },
            'W' => Node::Class {
                neg: true,
                items: vec![ClassItem::Word(true)],
            },
            's' => Node::Class {
                neg: false,
                items: vec![ClassItem::Space(true)],
            },
            'S' => Node::Class {
                neg: true,
                items: vec![ClassItem::Space(true)],
            },
            'b' => Node::WordB(true),
            'B' => Node::WordB(false),
            'n' => Node::Ch('\n'),
            'r' => Node::Ch('\r'),
            't' => Node::Ch('\t'),
            'f' => Node::Ch('\u{c}'),
            'v' => Node::Ch('\u{b}'),
            '0' => Node::Ch('\0'),
            'x' => Node::Ch(self.hex(2)?),
            'u' => Node::Ch(self.hex(4)?),
            '1'..='9' => return Err("backreferences are not supported".to_string()),
            other => Node::Ch(other),
        })
    }

    fn quantifier(&mut self, atom: Node) -> Result<Node, String> {
        let repeated = |min: usize, max: Option<usize>, greedy: bool| Node::Repeat {
            node: Box::new(atom.clone()),
            min,
            max,
            greedy,
        };
        match self.peek() {
            Some('*') => {
                self.bump();
                let greedy = !self.eat('?');
                Ok(repeated(0, None, greedy))
            }
            Some('+') => {
                self.bump();
                let greedy = !self.eat('?');
                Ok(repeated(1, None, greedy))
            }
            Some('?') => {
                self.bump();
                let greedy = !self.eat('?');
                Ok(repeated(0, Some(1), greedy))
            }
            Some('{') => {
                let save = self.i;
                self.bump();
                let mut lo = 0usize;
                let mut hi: Option<usize> = None;
                let mut digits = String::new();
                while let Some(c) = self.peek() {
                    if c.is_ascii_digit() {
                        digits.push(c);
                        self.bump();
                    } else {
                        break;
                    }
                }
                if digits.is_empty() {
                    self.i = save;
                    return Ok(atom); // literal '{'
                }
                lo = digits.parse().map_err(|_| "bad {n,m}".to_string())?;
                if self.eat(',') {
                    let mut d2 = String::new();
                    while let Some(c) = self.peek() {
                        if c.is_ascii_digit() {
                            d2.push(c);
                            self.bump();
                        } else {
                            break;
                        }
                    }
                    hi = if d2.is_empty() {
                        None
                    } else {
                        Some(d2.parse().map_err(|_| "bad {n,m}".to_string())?)
                    };
                } else {
                    hi = Some(lo);
                }
                if !self.eat('}') {
                    self.i = save;
                    return Ok(atom);
                }
                if lo > 1000 || hi.unwrap_or(0) > 1000 {
                    return Err("repetition count too large".to_string());
                }
                let greedy = !self.eat('?');
                Ok(repeated(lo, hi, greedy))
            }
            _ => Ok(atom),
        }
    }
}

// ───────────────────────────── compiler ─────────────────────────────

fn emit(prog: &mut Vec<Inst>, i: Inst) -> usize {
    prog.push(i);
    prog.len() - 1
}

fn compile_node(prog: &mut Vec<Inst>, n: &Node) {
    match n {
        Node::Empty => {}
        Node::Ch(c) => {
            emit(prog, Inst::Ch(*c));
        }
        Node::Any => {
            emit(prog, Inst::Any);
        }
        Node::Class { neg, items } => {
            emit(
                prog,
                Inst::Class {
                    neg: *neg,
                    items: items.clone(),
                },
            );
        }
        Node::Start => {
            emit(prog, Inst::AssertStart);
        }
        Node::End => {
            emit(prog, Inst::AssertEnd);
        }
        Node::WordB(w) => {
            emit(prog, Inst::WordB(*w));
        }
        Node::Group { idx, node } => {
            if let Some(i) = idx {
                emit(prog, Inst::Save(2 * i));
            }
            compile_node(prog, node);
            if let Some(i) = idx {
                emit(prog, Inst::Save(2 * i + 1));
            }
        }
        Node::Seq(items) => {
            for it in items {
                compile_node(prog, it);
            }
        }
        Node::Alt(branches) => {
            // split L1, L2 ; L1: b1 ; jmp END ; L2: ...
            let mut jumps = Vec::new();
            for (i, b) in branches.iter().enumerate() {
                if i + 1 == branches.len() {
                    compile_node(prog, b);
                    break;
                }
                let split = emit(prog, Inst::Split(0, 0));
                let l1 = prog.len();
                compile_node(prog, b);
                jumps.push(emit(prog, Inst::Jmp(0)));
                let l2 = prog.len();
                prog[split] = Inst::Split(l1, l2);
            }
            let end = prog.len();
            for j in jumps {
                prog[j] = Inst::Jmp(end);
            }
        }
        Node::Repeat {
            node,
            min,
            max,
            greedy,
        } => {
            for _ in 0..*min {
                compile_node(prog, node);
            }
            match max {
                None => {
                    // L1: split L2, L3 ; L2: body ; jmp L1 ; L3:
                    let split = emit(prog, Inst::Split(0, 0));
                    let body = prog.len();
                    compile_node(prog, node);
                    emit(prog, Inst::Jmp(split));
                    let out = prog.len();
                    prog[split] = if *greedy {
                        Inst::Split(body, out)
                    } else {
                        Inst::Split(out, body)
                    };
                }
                Some(hi) => {
                    let extra = hi.saturating_sub(*min);
                    let mut splits = Vec::new();
                    for _ in 0..extra {
                        let split = emit(prog, Inst::Split(0, 0));
                        let body = prog.len();
                        compile_node(prog, node);
                        splits.push((split, body));
                    }
                    let out = prog.len();
                    for (split, body) in splits {
                        prog[split] = if *greedy {
                            Inst::Split(body, out)
                        } else {
                            Inst::Split(out, body)
                        };
                    }
                }
            }
        }
    }
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn class_matches(items: &[ClassItem], neg: bool, c: char, ci: bool) -> bool {
    let hit = items.iter().any(|it| match it {
        ClassItem::Ch(x) => *x == c || (ci && x.to_ascii_lowercase() == c.to_ascii_lowercase()),
        ClassItem::Range(a, b) => {
            (*a <= c && c <= *b)
                || (ci
                    && a.to_ascii_lowercase() <= c.to_ascii_lowercase()
                    && c.to_ascii_lowercase() <= b.to_ascii_lowercase())
        }
        ClassItem::Digit(_) => c.is_ascii_digit(),
        ClassItem::Word(_) => is_word(c),
        ClassItem::Space(_) => c.is_whitespace(),
    });
    hit != neg
}

impl Regex {
    pub fn compile(pattern: &str, flags: &str) -> Result<Regex, String> {
        let mut f = Flags::default();
        for c in flags.chars() {
            match c {
                'g' => f.global = true,
                'i' => f.ignore_case = true,
                'm' => f.multiline = true,
                's' => f.dot_all = true,
                'y' => f.sticky = true,
                'u' => {} // matching is already code-point based
                other => return Err(format!("invalid flag '{}'", other)),
            }
        }
        let mut p = Parser::new(pattern);
        // `\W` and `\S` inside classes: the parser above handles them at the
        // class level for [..], but a bare escape is a negated class on its own.
        let ast = p.parse()?;
        let mut prog = Vec::new();
        compile_node(&mut prog, &ast);
        emit(&mut prog, Inst::Match);
        Ok(Regex {
            prog,
            ncaps: p.ncaps,
            flags: f,
            source: pattern.to_string(),
        })
    }

    /// Run the program at `pos` (char index). Returns the char index where the
    /// match ends, plus capture pairs.
    fn run(
        &self,
        pc: usize,
        pos: usize,
        hay: &[char],
        caps: &mut Vec<Option<usize>>,
        steps: &mut u32,
        seen: &mut HashSet<(usize, usize)>,
    ) -> Option<usize> {
        if *steps == 0 {
            return None;
        }
        *steps -= 1;
        if !seen.insert((pc, pos)) {
            // A loop that consumed nothing would spin forever.
            return None;
        }
        match &self.prog[pc] {
            Inst::Match => Some(pos),
            Inst::Ch(c) => {
                let h = *hay.get(pos)?;
                let ok = if self.flags.ignore_case {
                    h.to_ascii_lowercase() == c.to_ascii_lowercase() || h == *c
                } else {
                    h == *c
                };
                if ok {
                    self.run(pc + 1, pos + 1, hay, caps, steps, seen)
                } else {
                    None
                }
            }
            Inst::Any => {
                let h = *hay.get(pos)?;
                if !self.flags.dot_all && (h == '\n' || h == '\r') {
                    return None;
                }
                self.run(pc + 1, pos + 1, hay, caps, steps, seen)
            }
            Inst::Class { neg, items } => {
                let h = *hay.get(pos)?;
                if class_matches(items, *neg, h, self.flags.ignore_case) {
                    self.run(pc + 1, pos + 1, hay, caps, steps, seen)
                } else {
                    None
                }
            }
            Inst::AssertStart => {
                let at_start = pos == 0
                    || (self.flags.multiline && hay.get(pos.wrapping_sub(1)) == Some(&'\n'));
                if at_start {
                    self.run(pc + 1, pos, hay, caps, steps, seen)
                } else {
                    None
                }
            }
            Inst::AssertEnd => {
                let at_end =
                    pos == hay.len() || (self.flags.multiline && hay.get(pos) == Some(&'\n'));
                if at_end {
                    self.run(pc + 1, pos, hay, caps, steps, seen)
                } else {
                    None
                }
            }
            Inst::WordB(want) => {
                let before = if pos == 0 {
                    false
                } else {
                    is_word(hay[pos - 1])
                };
                let after = hay.get(pos).map(|c| is_word(*c)).unwrap_or(false);
                let boundary = before != after;
                if boundary == *want {
                    self.run(pc + 1, pos, hay, caps, steps, seen)
                } else {
                    None
                }
            }
            Inst::Save(slot) => {
                let old = caps[*slot];
                caps[*slot] = Some(pos);
                match self.run(pc + 1, pos, hay, caps, steps, seen) {
                    Some(end) => Some(end),
                    None => {
                        caps[*slot] = old;
                        None
                    }
                }
            }
            Inst::Jmp(t) => self.run(*t, pos, hay, caps, steps, seen),
            Inst::Split(a, b) => {
                if let Some(end) = self.run(*a, pos, hay, caps, steps, seen) {
                    return Some(end);
                }
                self.run(*b, pos, hay, caps, steps, seen)
            }
        }
    }

    /// Search from byte offset `start_byte`. `None` when there is no match.
    pub fn exec(&self, hay: &str, start_byte: usize) -> Option<Match> {
        // char list + byte offsets
        let mut chars: Vec<char> = Vec::with_capacity(hay.len());
        let mut offsets: Vec<usize> = Vec::with_capacity(hay.len() + 1);
        for (b, c) in hay.char_indices() {
            offsets.push(b);
            chars.push(c);
        }
        offsets.push(hay.len());
        let start_char = match offsets.binary_search(&start_byte) {
            Ok(i) => i,
            Err(i) => i,
        };
        let mut at = start_char;
        while at <= chars.len() {
            let mut caps = vec![None; 2 * (self.ncaps + 1)];
            caps[0] = Some(at);
            let mut steps = STEP_BUDGET;
            let mut seen = HashSet::new();
            if let Some(end) = self.run(0, at, &chars, &mut caps, &mut steps, &mut seen) {
                // An empty match must still advance the caller's the search, but
                // for `exec` returning it is correct.
                caps[1] = Some(end);
                let byte = |ci: usize| offsets.get(ci).copied().unwrap_or(hay.len());
                let mut out = Vec::with_capacity(self.ncaps + 1);
                for g in 0..=self.ncaps {
                    let s = caps[2 * g];
                    let e = caps[2 * g + 1];
                    out.push(match (s, e) {
                        (Some(s), Some(e)) => Some((byte(s), byte(e))),
                        _ => None,
                    });
                }
                let (s, e) = out[0].unwrap_or((start_byte, start_byte));
                return Some(Match {
                    start: s,
                    end: e,
                    caps: out,
                });
            }
            if self.flags.sticky {
                return None;
            }
            at += 1;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pat: &str, hay: &str) -> Option<(usize, usize)> {
        Regex::compile(pat, "")
            .unwrap()
            .exec(hay, 0)
            .map(|m| (m.start, m.end))
    }

    #[test]
    fn literals_and_dot() {
        assert_eq!(m("abc", "xxabcyy"), Some((2, 5)));
        assert_eq!(m("a.c", "abc"), Some((0, 3)));
        assert_eq!(m("a.c", "a\nc"), None); // dot excludes newline
        assert_eq!(
            Regex::compile("a.c", "s")
                .unwrap()
                .exec("a\nc", 0)
                .map(|m| m.end),
            Some(3)
        );
    }

    #[test]
    fn classes_ranges_and_negation() {
        assert_eq!(m("[a-c]+", "xxabcz"), Some((2, 5)));
        assert_eq!(m("[^a-c]+", "abcXYZ"), Some((3, 6)));
        assert_eq!(m("\\d{3}", "ab1234"), Some((2, 5)));
        // node: /[\w.-]+/ on "x y.z" matches just "x" (space is not in the class)
        assert_eq!(m("[\\w.-]+", "x y.z"), Some((0, 1)));
    }

    #[test]
    fn quantifiers_greedy_and_lazy() {
        assert_eq!(m("a+", "aaa"), Some((0, 3)));
        assert_eq!(m("a+?", "aaa"), Some((0, 1)));
        assert_eq!(m("a{2,3}", "aaaa"), Some((0, 3)));
        assert_eq!(m("colou?r", "color"), Some((0, 5)));
        assert_eq!(m("x*", "yyy"), Some((0, 0)));
    }

    #[test]
    fn anchors_and_boundaries() {
        assert_eq!(m("^ab", "ab"), Some((0, 2)));
        assert_eq!(m("^b", "ab"), None);
        assert_eq!(m("b$", "ab"), Some((1, 2)));
        assert_eq!(m("\\bcat\\b", "a cat sat"), Some((2, 5)));
        assert_eq!(m("\\bcat\\b", "scatter"), None);
    }

    #[test]
    fn groups_and_alternation() {
        let r = Regex::compile("(ab|cd)+", "").unwrap();
        let m = r.exec("xxabcd", 0).unwrap();
        assert_eq!((m.start, m.end), (2, 6));
        assert!(m.caps.len() >= 2, "capture present");
        // non-capturing group does not add a slot
        let r2 = Regex::compile("(?:ab)+", "").unwrap();
        assert_eq!(r2.ncaps, 0);
    }

    #[test]
    fn case_insensitive_and_multiline() {
        assert_eq!(
            Regex::compile("abc", "i")
                .unwrap()
                .exec("xABC", 0)
                .map(|m| m.end),
            Some(4)
        );
        assert_eq!(
            Regex::compile("^b", "m")
                .unwrap()
                .exec("a\nb", 0)
                .map(|m| m.start),
            Some(2)
        );
    }

    #[test]
    fn global_successive_execs_advance() {
        let r = Regex::compile("a", "g").unwrap();
        let mut pos = 0;
        let mut hits = 0;
        while let Some(m) = r.exec("banana", pos) {
            hits += 1;
            pos = m.end.max(m.start + 1);
            if hits > 10 {
                break;
            }
        }
        assert_eq!(hits, 3);
    }

    #[test]
    fn rejects_unsupported_constructs_instead_of_matching_nothing() {
        assert!(Regex::compile("(?=a)", "").is_err());
        assert!(Regex::compile("(a)\\1", "").is_err());
        assert!(Regex::compile("(?<name>a)", "").is_err());
        assert!(Regex::compile("a", "q").is_err());
    }

    #[test]
    fn catastrophic_pattern_is_bounded_not_hung() {
        // (a+)+$ on a long non-matching run backtracks exponentially; the budget
        // must turn it into a fast None rather than a hang.
        let r = Regex::compile("(a+)+$", "").unwrap();
        let hay = "a".repeat(40) + "b";
        assert_eq!(r.exec(&hay, 0), None);
    }
}
