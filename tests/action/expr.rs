//! GitHub expression evaluation for the forms `action.yml` uses.
//!
//! The tokenizer, parser and interpreter port the Python harness, and the
//! `and` and `or` nodes return their operands verbatim, which the cache key
//! expression `inputs.context-target != '' && inputs.context-target || 'any'`
//! relies on.

use serde_json::Value;

/// One value an expression can produce.
#[derive(Clone, Debug, PartialEq)]
enum Val {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
}

/// One token of an expression.
#[derive(Clone, Debug)]
enum Tok {
    Str(String),
    Op(&'static str),
    Lparen,
    Rparen,
    Ident(String),
}

/// One node of the parsed expression.
#[derive(Debug)]
enum Node {
    Lit(Val),
    Path(String),
    Call(String),
    And(Box<Node>, Box<Node>),
    Or(Box<Node>, Box<Node>),
    Not(Box<Node>),
    Cmp(&'static str, Box<Node>, Box<Node>),
}

fn decode_str(tok: &str) -> Val {
    if tok.starts_with('"') {
        let s = serde_json::from_str::<String>(tok)
            .unwrap_or_else(|e| panic!("bad string literal {tok}: {e}"));
        Val::Str(s)
    } else {
        Val::Str(tok[1..tok.len() - 1].to_string())
    }
}

fn tokenize(expr: &str) -> Vec<Tok> {
    let b = expr.as_bytes();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'"' || c == b'\'' {
            let mut j = i + 1;
            let mut closed = false;
            while j < b.len() {
                if b[j] == b'\\' {
                    j += 2;
                } else if b[j] == c {
                    closed = true;
                    break;
                } else {
                    j += 1;
                }
            }
            assert!(closed, "cannot tokenize expression {expr:?} at {i}");
            toks.push(Tok::Str(expr[i..=j].to_string()));
            i = j + 1;
        } else if c == b'(' {
            toks.push(Tok::Lparen);
            i += 1;
        } else if c == b')' {
            toks.push(Tok::Rparen);
            i += 1;
        } else if b[i..].starts_with(b"&&") {
            toks.push(Tok::Op("&&"));
            i += 2;
        } else if b[i..].starts_with(b"||") {
            toks.push(Tok::Op("||"));
            i += 2;
        } else if b[i..].starts_with(b"==") {
            toks.push(Tok::Op("=="));
            i += 2;
        } else if b[i..].starts_with(b"!=") {
            toks.push(Tok::Op("!="));
            i += 2;
        } else if c == b'>' && b.get(i + 1) == Some(&b'=') {
            toks.push(Tok::Op(">="));
            i += 2;
        } else if c == b'<' && b.get(i + 1) == Some(&b'=') {
            toks.push(Tok::Op("<="));
            i += 2;
        } else if c == b'>' {
            toks.push(Tok::Op(">"));
            i += 1;
        } else if c == b'<' {
            toks.push(Tok::Op("<"));
            i += 1;
        } else if c == b'!' {
            toks.push(Tok::Op("!"));
            i += 1;
        } else if c.is_ascii_alphanumeric() || c == b'.' || c == b'_' || c == b'-' {
            let mut j = i + 1;
            while j < b.len()
                && (b[j].is_ascii_alphanumeric() || b[j] == b'.' || b[j] == b'_' || b[j] == b'-')
            {
                j += 1;
            }
            toks.push(Tok::Ident(expr[i..j].to_string()));
            i = j;
        } else {
            panic!("cannot tokenize expression {expr:?} at {i}");
        }
    }
    toks
}

struct Parser {
    toks: Vec<Tok>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.i)
    }

    fn parse(mut self) -> Node {
        let node = self.parse_or();
        assert!(self.peek().is_none(), "trailing tokens in expression");
        node
    }

    fn parse_or(&mut self) -> Node {
        let mut node = self.parse_and();
        while self.peek().is_some_and(|t| matches!(t, Tok::Op("||"))) {
            self.i += 1;
            node = Node::Or(Box::new(node), Box::new(self.parse_and()));
        }
        node
    }

    fn parse_and(&mut self) -> Node {
        let mut node = self.parse_cmp();
        while self.peek().is_some_and(|t| matches!(t, Tok::Op("&&"))) {
            self.i += 1;
            node = Node::And(Box::new(node), Box::new(self.parse_cmp()));
        }
        node
    }

    fn parse_cmp(&mut self) -> Node {
        let left = self.parse_not();
        if let Some(Tok::Op(op)) = self.peek().cloned()
            && matches!(op, "==" | "!=" | ">" | "<" | ">=" | "<=")
        {
            self.i += 1;
            return Node::Cmp(op, Box::new(left), Box::new(self.parse_not()));
        }
        left
    }

    fn parse_not(&mut self) -> Node {
        if self.peek().is_some_and(|t| matches!(t, Tok::Op("!"))) {
            self.i += 1;
            return Node::Not(Box::new(self.parse_not()));
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> Node {
        let Some(tok) = self.toks.get(self.i).cloned() else {
            panic!("unexpected end of expression");
        };
        self.i += 1;
        match tok {
            Tok::Str(s) => Node::Lit(decode_str(&s)),
            Tok::Lparen => {
                let node = self.parse_or();
                assert!(
                    matches!(self.peek(), Some(Tok::Rparen)),
                    "missing ) in expression"
                );
                self.i += 1;
                node
            }
            Tok::Ident(name) => {
                if matches!(self.peek(), Some(Tok::Lparen)) {
                    self.i += 1;
                    assert!(
                        matches!(self.peek(), Some(Tok::Rparen)),
                        "missing ) in call {name}"
                    );
                    self.i += 1;
                    Node::Call(name)
                } else if name == "true" {
                    Node::Lit(Val::Bool(true))
                } else if name == "false" {
                    Node::Lit(Val::Bool(false))
                } else if !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()) {
                    Node::Lit(Val::Num(name.parse::<f64>().expect("digit literal")))
                } else {
                    Node::Path(name)
                }
            }
            Tok::Op(_) | Tok::Rparen => panic!("unexpected token in expression"),
        }
    }
}

fn truthy(v: &Val) -> bool {
    match v {
        Val::Null => false,
        Val::Bool(b) => *b,
        Val::Num(n) => *n != 0.0,
        Val::Str(s) => !s.is_empty(),
    }
}

fn num(v: &Val) -> Option<f64> {
    match v {
        Val::Num(n) => Some(*n),
        Val::Str(s) => s.parse::<f64>().ok(),
        Val::Null | Val::Bool(_) => None,
    }
}

fn falsy_zero(v: &Val) -> bool {
    match v {
        Val::Null => true,
        Val::Bool(b) => !b,
        Val::Num(n) => *n == 0.0,
        Val::Str(s) => s.is_empty(),
    }
}

fn loose_eq(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::Null, x) | (x, Val::Null) => falsy_zero(x),
        (Val::Str(s), Val::Str(t)) => s == t,
        (Val::Bool(x), Val::Bool(y)) => x == y,
        _ => match (num(a), num(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
    }
}

fn cmp(op: &str, a: &Val, b: &Val) -> Val {
    match op {
        "==" => Val::Bool(loose_eq(a, b)),
        "!=" => Val::Bool(!loose_eq(a, b)),
        _ => match (num(a), num(b)) {
            (Some(x), Some(y)) => Val::Bool(match op {
                ">" => x > y,
                ">=" => x >= y,
                "<" => x < y,
                _ => x <= y,
            }),
            _ => match (a, b) {
                (Val::Str(s), Val::Str(t)) => Val::Bool(match op {
                    ">" => s > t,
                    ">=" => s >= t,
                    "<" => s < t,
                    _ => s <= t,
                }),
                _ => Val::Bool(false),
            },
        },
    }
}

fn lookup(ctx: &Value, path: &str) -> Val {
    let mut cur = ctx;
    for part in path.split('.') {
        match cur.get(part) {
            Some(v) => cur = v,
            None => return Val::Null,
        }
    }
    match cur {
        Value::Null | Value::Array(_) | Value::Object(_) => Val::Null,
        Value::Bool(b) => Val::Bool(*b),
        Value::Number(n) => Val::Num(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => Val::Str(s.clone()),
    }
}

fn eval(node: &Node, ctx: &Value) -> Val {
    match node {
        Node::Lit(v) => v.clone(),
        Node::Path(p) => lookup(ctx, p),
        Node::Call(name) => match name.as_str() {
            "always" => Val::Bool(true),
            "cancelled" => Val::Bool(false),
            _ => Val::Null,
        },
        Node::And(a, b) => {
            let x = eval(a, ctx);
            if truthy(&x) { eval(b, ctx) } else { x }
        }
        Node::Or(a, b) => {
            let x = eval(a, ctx);
            if truthy(&x) { x } else { eval(b, ctx) }
        }
        Node::Not(a) => Val::Bool(!truthy(&eval(a, ctx))),
        Node::Cmp(op, a, b) => cmp(op, &eval(a, ctx), &eval(b, ctx)),
    }
}

fn render(v: &Val) -> String {
    match v {
        Val::Null => String::new(),
        Val::Bool(b) => b.to_string(),
        Val::Num(n) => format!("{n}"),
        Val::Str(s) => s.clone(),
    }
}

/// Evaluate one expression against the runner context.
fn evaluate(expr: &str, ctx: &Value) -> Val {
    let toks = tokenize(expr);
    let node = Parser { toks, i: 0 }.parse();
    eval(&node, ctx)
}

/// Replace every `${{ expr }}` in the text with the rendered value.
pub(crate) fn interp(text: &str, ctx: &Value) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 3..];
        match after.find("}}") {
            Some(end) if !after[..end].is_empty() && !after[..end].contains('\n') => {
                out.push_str(&render(&evaluate(&after[..end], ctx)));
                rest = &after[end + 2..];
            }
            _ => {
                out.push_str("${{");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Evaluate a step `if` expression, empty meaning the step runs.
pub(crate) fn if_true(text: Option<&str>, ctx: &Value) -> bool {
    let Some(text) = text else {
        return true;
    };
    let t = text.trim();
    if t.is_empty() {
        return true;
    }
    let inner = if t.len() > 6 && t.starts_with("${{") && t.ends_with("}}") {
        &t[3..t.len() - 2]
    } else {
        t
    };
    truthy(&evaluate(inner, ctx))
}

/// Whether the `if` text carries an `always(` call.
pub(crate) fn has_always(text: &str) -> bool {
    let mut rest = text;
    while let Some(pos) = rest.find("always") {
        let boundary = rest[..pos]
            .chars()
            .last()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_');
        if boundary {
            let after = rest[pos + 6..].trim_start();
            if after.starts_with('(') {
                return true;
            }
        }
        rest = &rest[pos + 1..];
    }
    false
}
