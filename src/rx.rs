//! A small regular-expression engine, written from scratch because the tool
//! must stay a single dependency-free binary.
//!
//! Supports: literals, `.`, `[...]` with ranges and negation, `*` `+` `?`
//! `{n}` `{n,}` `{n,m}`, alternation `|`, groups `(...)`, anchors `^` `$`,
//! shorthand `\d \w \s \D \W \S`, escaped metacharacters, and a `\b` word
//! boundary. Matching uses a Pike VM: linear in the text, no catastrophic
//! backtracking, and leftmost-first like Perl alternation.

#[derive(Clone, Debug)]
pub struct Class {
    pub neg: bool,
    pub ranges: Vec<(char, char)>,
    /// Case folding for this class, captured when the pattern was compiled.
    ///
    /// Without it `[a-z]` refused to match `A` even though a bare literal `a`
    /// matched `A` happily: `zebra` found `Zebra`, `[z]ebra` did not. The flag
    /// existed on the parser (`P.ci`) and on every literal instruction, but the
    /// matcher never consulted it for classes, so half the engine honoured
    /// `--case` and half ignored it.
    pub ci: bool,
}

impl Class {
    /// Every form of `c` that this class should be tested against.
    ///
    /// Case folding is per character and can expand to several characters
    /// (`ß` ↔ `SS`), but a class matches single characters, so only the
    /// single-character foldings are usable here. That covers ASCII and the
    /// Latin/Cyrillic/Greek runs where `to_lowercase` is one char.
    fn variants(&self, c: char) -> Vec<char> {
        let mut v = vec![c];
        if self.ci {
            for x in c.to_lowercase() {
                if x != c && !v.contains(&x) {
                    v.push(x);
                }
            }
            for x in c.to_uppercase() {
                if x != c && !v.contains(&x) {
                    v.push(x);
                }
            }
        }
        v
    }

    fn matches(&self, c: char) -> bool {
        // Negation applies to the *set* of variants, not to each one.
        //
        // With case folding, `[^abc]` must reject `A` because its lowercase form
        // is one of the excluded characters. Testing each variant against
        // `neg` separately gets that backwards: `A` misses the range, the miss
        // is then read as "satisfies the negation", and the excluded character
        // is admitted.
        let in_set = self
            .variants(c)
            .iter()
            .any(|cand| self.ranges.iter().any(|(a, b)| cand >= a && cand <= b));
        if self.neg {
            !in_set
        } else {
            in_set
        }
    }
}

#[derive(Clone, Debug)]
enum Inst {
    Char(char),
    Any,
    Cls(usize),
    Split(usize, usize),
    Jmp(usize),
    Start,
    End,
    WordB,
    Match,
}

#[derive(Clone, Debug)]
enum Ast {
    Empty,
    Lit(char),
    Any,
    Cls(Class),
    Start,
    End,
    WordB,
    Cat(Vec<Ast>),
    Alt(Vec<Ast>),
    Rep(Box<Ast>, u32, Option<u32>),
}

pub struct Regex {
    prog: Vec<Inst>,
    classes: Vec<Class>,
    ci: bool,
}

struct P<'a> {
    s: &'a [char],
    i: usize,
    ci: bool,
}

impl<'a> P<'a> {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }
    fn next(&mut self) -> Option<char> {
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

    fn alt(&mut self) -> Option<Ast> {
        let mut branches = vec![self.cat()?];
        while self.eat('|') {
            branches.push(self.cat()?);
        }
        if branches.len() == 1 {
            Some(branches.pop().unwrap())
        } else {
            Some(Ast::Alt(branches))
        }
    }

    fn cat(&mut self) -> Option<Ast> {
        let mut items: Vec<Ast> = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.atom()?;
            let atom = self.quant(atom);
            items.push(atom);
        }
        match items.len() {
            0 => Some(Ast::Empty),
            1 => Some(items.pop().unwrap()),
            _ => Some(Ast::Cat(items)),
        }
    }

    fn quant(&mut self, a: Ast) -> Ast {
        let mut out = a;
        loop {
            let (min, max) = match self.peek() {
                Some('*') => {
                    self.i += 1;
                    (0, None)
                }
                Some('+') => {
                    self.i += 1;
                    (1, None)
                }
                Some('?') => {
                    self.i += 1;
                    (0, Some(1))
                }
                Some('{') => {
                    // only treat as a quantifier when it parses as one
                    let save = self.i;
                    self.i += 1;
                    let mut lo = String::new();
                    while let Some(c) = self.peek() {
                        if c.is_ascii_digit() {
                            lo.push(c);
                            self.i += 1;
                        } else {
                            break;
                        }
                    }
                    if lo.is_empty() {
                        self.i = save;
                        break;
                    }
                    let lo_n: u32 = lo.parse().unwrap_or(0);
                    let hi_n = if self.eat(',') {
                        let mut hi = String::new();
                        while let Some(c) = self.peek() {
                            if c.is_ascii_digit() {
                                hi.push(c);
                                self.i += 1;
                            } else {
                                break;
                            }
                        }
                        if hi.is_empty() {
                            None
                        } else {
                            Some(hi.parse().unwrap_or(lo_n))
                        }
                    } else {
                        Some(lo_n)
                    };
                    if !self.eat('}') {
                        self.i = save;
                        break;
                    }
                    (lo_n, hi_n)
                }
                _ => break,
            };
            out = Ast::Rep(Box::new(out), min, max);
            // lazy modifier `?` after a quantifier is accepted and ignored
            if self.peek() == Some('?') && matches!(self.s.get(self.i + 1), Some(_)) {
                // `a*?` etc: consume the lazy marker
                if matches!(
                    self.s.get(self.i.wrapping_sub(1)),
                    Some('*') | Some('+') | Some('?') | Some('}')
                ) {
                    self.i += 1;
                }
            }
        }
        out
    }

    fn class_body(&mut self) -> Class {
        let neg = self.eat('^');
        let mut ranges: Vec<(char, char)> = Vec::new();
        let mut first = true;
        loop {
            let c = match self.next() {
                Some(c) => c,
                None => break,
            };
            if c == ']' && !first {
                break;
            }
            first = false;
            let lo = if c == '\\' {
                match self.next() {
                    Some('d') => {
                        ranges.push(('0', '9'));
                        continue;
                    }
                    Some('w') => {
                        ranges.push(('a', 'z'));
                        ranges.push(('A', 'Z'));
                        ranges.push(('0', '9'));
                        ranges.push(('_', '_'));
                        continue;
                    }
                    Some('s') => {
                        ranges.push((' ', ' '));
                        ranges.push(('\t', '\t'));
                        ranges.push(('\n', '\n'));
                        ranges.push(('\r', '\r'));
                        continue;
                    }
                    Some(x) => x,
                    None => break,
                }
            } else {
                c
            };
            if self.peek() == Some('-') && self.s.get(self.i + 1).copied() != Some(']') {
                self.i += 1;
                let hi = match self.next() {
                    Some('\\') => self.next().unwrap_or(lo),
                    Some(x) => x,
                    None => lo,
                };
                ranges.push((lo, hi));
            } else {
                ranges.push((lo, lo));
            }
        }
        Class { neg, ranges, ci: self.ci }
    }

    fn atom(&mut self) -> Option<Ast> {
        let c = self.next()?;
        Some(match c {
            '.' => Ast::Any,
            '^' => Ast::Start,
            '$' => Ast::End,
            '(' => {
                // support (?:...) by skipping the marker
                if self.peek() == Some('?') {
                    let save = self.i;
                    self.i += 1;
                    if self.eat(':') {
                        // non-capturing, same behaviour
                    } else {
                        self.i = save;
                    }
                }
                let inner = self.alt()?;
                if !self.eat(')') {
                    return None;
                }
                inner
            }
            '[' => Ast::Cls(self.class_body()),
            '\\' => {
                let e = self.next()?;
                match e {
                    'd' => Ast::Cls(Class {
                        neg: false,
                        ranges: vec![('0', '9')],
                        ci: self.ci,
                    }),
                    'D' => Ast::Cls(Class {
                        neg: true,
                        ranges: vec![('0', '9')],
                        ci: self.ci,
                    }),
                    'w' => Ast::Cls(Class {
                        neg: false,
                        ranges: vec![('a', 'z'), ('A', 'Z'), ('0', '9'), ('_', '_')],
                        ci: self.ci,
                    }),
                    'W' => Ast::Cls(Class {
                        neg: true,
                        ranges: vec![('a', 'z'), ('A', 'Z'), ('0', '9'), ('_', '_')],
                        ci: self.ci,
                    }),
                    's' => Ast::Cls(Class {
                        neg: false,
                        ranges: vec![(' ', ' '), ('\t', '\t'), ('\n', '\n'), ('\r', '\r')],
                        ci: self.ci,
                    }),
                    'S' => Ast::Cls(Class {
                        neg: true,
                        ranges: vec![(' ', ' '), ('\t', '\t'), ('\n', '\n'), ('\r', '\r')],
                        ci: self.ci,
                    }),
                    'b' => Ast::WordB,
                    'n' => Ast::Lit('\n'),
                    't' => Ast::Lit('\t'),
                    'r' => Ast::Lit('\r'),
                    other => Ast::Lit(other),
                }
            }
            other => Ast::Lit(other),
        })
    }
}

struct C {
    prog: Vec<Inst>,
    classes: Vec<Class>,
}

impl C {
    fn emit(&mut self, a: &Ast) {
        match a {
            Ast::Empty => {}
            Ast::Lit(c) => self.prog.push(Inst::Char(*c)),
            Ast::Any => self.prog.push(Inst::Any),
            Ast::Cls(cl) => {
                self.classes.push(cl.clone());
                self.prog.push(Inst::Cls(self.classes.len() - 1));
            }
            Ast::Start => self.prog.push(Inst::Start),
            Ast::End => self.prog.push(Inst::End),
            Ast::WordB => self.prog.push(Inst::WordB),
            Ast::Cat(v) => {
                for x in v {
                    self.emit(x);
                }
            }
            Ast::Alt(v) => {
                // split chain
                let mut jumps: Vec<usize> = Vec::new();
                for (i, br) in v.iter().enumerate() {
                    if i + 1 < v.len() {
                        let split = self.prog.len();
                        self.prog.push(Inst::Split(split + 1, 0));
                        self.emit(br);
                        let j = self.prog.len();
                        self.prog.push(Inst::Jmp(0));
                        jumps.push(j);
                        let next = self.prog.len();
                        if let Inst::Split(_, b) = &mut self.prog[split] {
                            *b = next;
                        }
                    } else {
                        self.emit(br);
                    }
                }
                let end = self.prog.len();
                for j in jumps {
                    if let Inst::Jmp(t) = &mut self.prog[j] {
                        *t = end;
                    }
                }
            }
            Ast::Rep(inner, min, max) => {
                let mut ends: Vec<usize> = Vec::new();
                for _ in 0..*min {
                    self.emit(inner);
                }
                match max {
                    None => {
                        // star: L: Split(body, end); body; Jmp L
                        let l = self.prog.len();
                        self.prog.push(Inst::Split(l + 1, 0));
                        self.emit(inner);
                        self.prog.push(Inst::Jmp(l));
                        let end = self.prog.len();
                        if let Inst::Split(_, b) = &mut self.prog[l] {
                            *b = end;
                        }
                    }
                    Some(mx) => {
                        for _ in *min..*mx {
                            let s = self.prog.len();
                            self.prog.push(Inst::Split(s + 1, 0));
                            ends.push(s);
                            self.emit(inner);
                        }
                        let end = self.prog.len();
                        for s in ends {
                            if let Inst::Split(_, b) = &mut self.prog[s] {
                                *b = end;
                            }
                        }
                    }
                }
            }
        }
    }
}

impl Regex {
    pub fn new(pat: &str, case_insensitive: bool) -> Result<Regex, String> {
        let chars: Vec<char> = pat.chars().collect();
        let mut p = P {
            s: &chars,
            i: 0,
            ci: case_insensitive,
        };
        let ast = match p.alt() {
            Some(a) => a,
            None => return Err(format!("正则语法错误: {pat}")),
        };
        if p.i < chars.len() {
            return Err(format!("正则未消费完（位置 {}）: {pat}", p.i));
        }
        let mut c = C {
            prog: Vec::new(),
            classes: Vec::new(),
        };
        c.emit(&ast);
        c.prog.push(Inst::Match);
        Ok(Regex {
            prog: c.prog,
            classes: c.classes,
            ci: case_insensitive,
        })
    }

    fn eq_char(&self, pat: char, t: char) -> bool {
        if pat == t {
            return true;
        }
        if !self.ci {
            return false;
        }
        let a: String = pat.to_lowercase().collect();
        let b: String = t.to_lowercase().collect();
        a == b
    }

    fn is_word(c: char) -> bool {
        c.is_ascii_alphanumeric() || c == '_' || (c as u32) >= 0x3400
    }

    fn add(
        &self,
        list: &mut Vec<(usize, usize)>,
        seen: &mut [bool],
        pc: usize,
        start: usize,
        pos: usize,
        chars: &[char],
        budget: &mut u32,
    ) {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        if pc >= self.prog.len() || seen[pc] {
            return;
        }
        seen[pc] = true;
        match &self.prog[pc] {
            Inst::Jmp(t) => self.add(list, seen, *t, start, pos, chars, budget),
            Inst::Split(a, b) => {
                self.add(list, seen, *a, start, pos, chars, budget);
                self.add(list, seen, *b, start, pos, chars, budget);
            }
            Inst::Start => {
                if pos == 0 {
                    self.add(list, seen, pc + 1, start, pos, chars, budget);
                }
            }
            Inst::End => {
                if pos == chars.len() {
                    self.add(list, seen, pc + 1, start, pos, chars, budget);
                }
            }
            Inst::WordB => {
                let left = pos > 0 && Regex::is_word(chars[pos - 1]);
                let right = pos < chars.len() && Regex::is_word(chars[pos]);
                if left != right {
                    self.add(list, seen, pc + 1, start, pos, chars, budget);
                }
            }
            _ => list.push((pc, start)),
        }
    }

    /// Find the leftmost match at or after `from`.
    pub fn find_at(&self, chars: &[char], from: usize) -> Option<(usize, usize)> {
        let n = chars.len();
        let mut clist: Vec<(usize, usize)> = Vec::new();
        let mut nlist: Vec<(usize, usize)> = Vec::new();
        let mut seen = vec![false; self.prog.len()];
        let mut budget: u32 = 4_000_000;
        let mut pos = from;
        loop {
            // start a new thread here (unanchored search)
            self.add(&mut clist, &mut seen, 0, pos, pos, chars, &mut budget);
            for (pc, st) in &clist {
                if matches!(self.prog[*pc], Inst::Match) {
                    return Some((*st, pos));
                }
            }
            if pos >= n {
                return None;
            }
            nlist.clear();
            for v in seen.iter_mut() {
                *v = false;
            }
            for (pc, st) in &clist {
                let ok = match &self.prog[*pc] {
                    Inst::Char(c) => self.eq_char(*c, chars[pos]),
                    Inst::Any => chars[pos] != '\n',
                    Inst::Cls(k) => self.classes[*k].matches(chars[pos]),
                    _ => false,
                };
                if ok {
                    self.add(&mut nlist, &mut seen, pc + 1, *st, pos + 1, chars, &mut budget);
                }
            }
            std::mem::swap(&mut clist, &mut nlist);
            if clist.is_empty() {
                // nothing alive; the next loop still adds a fresh start thread
            }
            pos += 1;
        }
    }

    pub fn is_match(&self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        self.find_at(&chars, 0).is_some()
    }
}
