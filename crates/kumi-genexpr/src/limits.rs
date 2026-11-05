//! What GenExpr may do on Live's audio thread, checked on the code as written, never by running it (CC-22 22-r1, 22-r1
//! memory, 22-r2). gen~ runs the code once for every sample, so code that loops long, or forever, holds Live's audio
//! thread up before any output stage runs. So:
//! - no `while` and no `do`; a `for` only with literal bounds, at most 64 times, nested at most 2 deep (a loop in a
//!   function called inside a loop counts as nested);
//! - in the worst case at most 4,096 operations per sample: each operator and call, times the loops around it, with each
//!   function the code defines counted where it's called (a function that calls itself, however indirectly, is refused);
//! - every `Delay` and `Data` sized by a literal, at most 2^21 samples each and 2^23 for the device;
//! - `Data` only, never `Buffer`, which reads a `buffer~` anything in the Set can fill;
//! - at most 400 lines and 32 KB of code.
//!
//! Each refusal is a finding the model can act on.

use crate::{tokens, Token};
use std::collections::HashMap;

pub const MOST_LINES: usize = 400;
pub const MOST_BYTES: usize = 32 * 1024;
pub const MOST_ITERATIONS: u64 = 64;
pub const MOST_NESTING: usize = 2;
pub const MOST_OPERATIONS: u64 = 4096;
pub const MOST_SAMPLES: u64 = 1 << 21;
pub const MOST_DEVICE_SAMPLES: u64 = 1 << 23;

/// How far a `for` is simulated to say how many times it runs; past this it's said never to end.
const SIMULATED: u64 = 1 << 20;

/// The symbols that are operations, each counted once where it's written.
const OPERATORS: [&str; 22] =
    ["+", "-", "*", "/", "%", "<", "<=", ">", ">=", "==", "!=", "&&", "||", "!", "?", "+=", "-=", "*=", "/=", "&", "|", "^"];

const DECLARATIONS: [&str; 5] = ["Param", "History", "Delay", "Data", "Buffer"];

/// The findings for `code`: empty when it may run on Live's audio thread.
pub fn check(code: &str) -> Vec<String> {
    let mut found = Vec::new();
    let lines = code.lines().count();
    if lines > MOST_LINES || code.len() > MOST_BYTES {
        found.push(format!(
            "code: it's {lines} lines and {} bytes, past the {MOST_LINES} lines or {} KB Kumi builds: make it shorter.",
            code.len(),
            MOST_BYTES / 1024
        ));
    }
    let tokens = match tokens(code) {
        Ok(tokens) => tokens,
        Err(why) => {
            found.push(format!("code: Kumi can't read it: {why}."));
            return found;
        }
    };
    let words = |word: &str| tokens.iter().any(|token| matches!(token, Token::Word(w) if w == word));
    if words("while") {
        found.push("code: it has a while loop, which could run forever on Live's audio thread: use a for with literal bounds (for (i = 0; i < 16; i += 1)).".into());
    }
    if words("do") {
        found.push("code: it has a do loop, which could run forever on Live's audio thread: use a for with literal bounds (for (i = 0; i < 16; i += 1)).".into());
    }
    if words("Buffer") {
        found.push(
            "code: it declares a Buffer, which reads a buffer~ anything in the Set can fill: use Data, the device's own memory.".into(),
        );
    }
    if !found.is_empty() {
        return found;
    }
    let mut reader = Reader { tokens: &tokens, at: 0, found: Vec::new(), memory: 0 };
    let program = match reader.program() {
        Ok(program) => program,
        Err(why) => {
            found.push(format!("code: Kumi can't read it: {why}."));
            return found;
        }
    };
    found.append(&mut reader.found);
    if reader.memory > MOST_DEVICE_SAMPLES {
        found.push(format!(
            "code: its Delays and Datas hold {} samples together, past the {MOST_DEVICE_SAMPLES} a device may: make them smaller.",
            reader.memory
        ));
    }
    let mut costs = Costs { functions: &program.functions, known: HashMap::new(), visiting: Vec::new() };
    match costs.nodes(&program.main) {
        Err(why) => found.push(format!("code: {why}.")),
        Ok(cost) => {
            if cost.depth > MOST_NESTING {
                found.push(format!(
                    "code: its for loops nest {} deep (counting loops in the functions they call), past the {MOST_NESTING} Kumi allows on Live's audio thread.",
                    cost.depth
                ));
            }
            if cost.operations > MOST_OPERATIONS {
                found.push(format!(
                    "code: in the worst case it takes {} operations each sample (each operator and call, times the loops around it, with its functions counted where they're called), past the {MOST_OPERATIONS} Kumi allows on Live's audio thread: do less in its loops.",
                    cost.operations
                ));
            }
        }
    }
    found
}

/// What runs each sample, as written.
#[derive(Debug)]
enum Node {
    /// A statement: its own operations, and the names it calls (each a function of the code's, or one of gen's).
    Plain {
        operations: u64,
        calls: Vec<String>,
    },
    Block(Vec<Node>),
    If {
        test: Box<Node>,
        then: Box<Node>,
        otherwise: Option<Box<Node>>,
    },
    /// A for: how many times it runs and its body. Its header is the loop's own and isn't counted: the budget is 64 × 64
    /// times a one-operation body.
    For {
        times: u64,
        body: Box<Node>,
    },
}

struct Program {
    functions: HashMap<String, Node>,
    main: Vec<Node>,
}

struct Reader<'a> {
    tokens: &'a [Token],
    at: usize,
    /// Refusals found while reading: loops and sizes.
    found: Vec<String>,
    /// Samples the Delays and Datas hold together.
    memory: u64,
}

fn is(token: Option<&Token>, symbol: &str) -> bool {
    matches!(token, Some(Token::Symbol(s)) if s == symbol)
}

impl Reader<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    /// The index of the bracket that closes the one at `open`.
    fn closing(&self, open: usize) -> Result<usize, String> {
        let (left, right) = match &self.tokens[open] {
            Token::Symbol(s) if s == "(" => ("(", ")"),
            Token::Symbol(s) if s == "{" => ("{", "}"),
            other => return Err(format!("expected a bracket, not {other:?}")),
        };
        let mut depth = 0usize;
        for at in open..self.tokens.len() {
            if is(self.tokens.get(at), left) {
                depth += 1;
            } else if is(self.tokens.get(at), right) {
                depth -= 1;
                if depth == 0 {
                    return Ok(at);
                }
            }
        }
        Err(format!("a {left} that never closes"))
    }

    fn program(&mut self) -> Result<Program, String> {
        let mut functions = HashMap::new();
        let mut main = Vec::new();
        while self.at < self.tokens.len() {
            // A function: a name, its parameters, then its body in braces.
            if let (Some(Token::Word(name)), true) = (self.peek(), is(self.tokens.get(self.at + 1), "(")) {
                if !["if", "for"].contains(&name.as_str()) {
                    let close = self.closing(self.at + 1)?;
                    if is(self.tokens.get(close + 1), "{") {
                        let name = name.clone();
                        self.at = close + 1;
                        let body = self.statement()?;
                        if functions.insert(name.clone(), body).is_some() {
                            return Err(format!("{name} is defined twice"));
                        }
                        continue;
                    }
                }
            }
            main.push(self.statement()?);
        }
        Ok(Program { functions, main })
    }

    fn statement(&mut self) -> Result<Node, String> {
        match self.peek() {
            None => Err("the code ends too soon".into()),
            Some(Token::Symbol(s)) if s == "{" => {
                let close = self.closing(self.at)?;
                self.at += 1;
                let mut body = Vec::new();
                while self.at < close {
                    body.push(self.statement()?);
                }
                self.at = close + 1;
                Ok(Node::Block(body))
            }
            Some(Token::Symbol(s)) if s == ";" => {
                self.at += 1;
                Ok(Node::Block(Vec::new()))
            }
            Some(Token::Word(w)) if w == "if" => {
                let open = self.at + 1;
                if !is(self.tokens.get(open), "(") {
                    return Err("an if without its test in parentheses".into());
                }
                let close = self.closing(open)?;
                let test = plain(&self.tokens[open + 1..close]);
                self.at = close + 1;
                let then = self.statement()?;
                let otherwise = if matches!(self.peek(), Some(Token::Word(w)) if w == "else") {
                    self.at += 1;
                    Some(Box::new(self.statement()?))
                } else {
                    None
                };
                Ok(Node::If { test: Box::new(test), then: Box::new(then), otherwise })
            }
            Some(Token::Word(w)) if w == "for" => self.for_loop(),
            Some(Token::Word(w)) if DECLARATIONS.contains(&w.as_str()) && matches!(self.tokens.get(self.at + 1), Some(Token::Word(_))) => {
                let word = w.clone();
                self.declaration(&word)
            }
            Some(_) => {
                let start = self.at;
                let mut depth = 0i64;
                loop {
                    match self.tokens.get(self.at) {
                        None => return Err("a statement without its ;".into()),
                        Some(Token::Symbol(s)) if s == "(" => depth += 1,
                        Some(Token::Symbol(s)) if s == ")" => depth -= 1,
                        Some(Token::Symbol(s)) if s == ";" && depth == 0 => break,
                        Some(Token::Symbol(s)) if (s == "{" || s == "}") && depth == 0 => return Err("a statement without its ;".into()),
                        _ => {}
                    }
                    self.at += 1;
                }
                let node = plain(&self.tokens[start..self.at]);
                self.at += 1;
                Ok(node)
            }
        }
    }

    /// `Param a(0), b(1, min=0);`, `Delay d(48000);`, `Data d(512, 2);`: what a Delay or a Data holds is counted.
    fn declaration(&mut self, word: &str) -> Result<Node, String> {
        self.at += 1;
        loop {
            let name = match self.tokens.get(self.at) {
                Some(Token::Word(name)) => name.clone(),
                other => return Err(format!("a {word} without its name, at {other:?}")),
            };
            self.at += 1;
            let tokens = self.tokens;
            let arguments: Vec<Vec<Token>> = if is(self.peek(), "(") {
                let open = self.at;
                let close = self.closing(open)?;
                self.at = close + 1;
                tokens[open + 1..close].split(|token| is(Some(token), ",")).map(<[Token]>::to_vec).collect()
            } else {
                Vec::new()
            };
            if word == "Delay" || word == "Data" {
                self.sized(word, &name, &arguments);
            }
            match self.peek() {
                Some(Token::Symbol(s)) if s == "," => self.at += 1,
                Some(Token::Symbol(s)) if s == ";" => {
                    self.at += 1;
                    return Ok(Node::Block(Vec::new()));
                }
                other => return Err(format!("a {word} that doesn't end in ;, at {other:?}")),
            }
        }
    }

    /// A Delay's or a Data's size: a literal whole number of samples (a Data's times its literal channels).
    fn sized(&mut self, word: &str, name: &str, arguments: &[Vec<Token>]) {
        let literal = |argument: Option<&Vec<Token>>| match argument.map(Vec::as_slice) {
            Some([Token::Number(n)]) if *n >= 1.0 && n.fract() == 0.0 => Some(*n as u64),
            _ => None,
        };
        let Some(size) = literal(arguments.first()) else {
            self.found.push(format!(
                "code: the {word} {name} needs its size as a literal number of samples ({word} {name}(48000)), so Kumi knows what it holds."
            ));
            return;
        };
        let channels = if word == "Data" && arguments.len() > 1 {
            match literal(arguments.get(1)) {
                Some(channels) => channels,
                None => {
                    self.found.push(format!("code: the Data {name} needs its channels as a literal number."));
                    return;
                }
            }
        } else {
            1
        };
        let samples = size.saturating_mul(channels);
        if samples > MOST_SAMPLES {
            self.found.push(format!(
                "code: the {word} {name} holds {samples} samples, past the {MOST_SAMPLES} (about 10.9 s at 192 kHz) one may: make it smaller."
            ));
        }
        self.memory = self.memory.saturating_add(samples);
    }

    /// `for (i = A; i < B; i += C) body`, with A, B and C literal numbers and i changed only by the step.
    fn for_loop(&mut self) -> Result<Node, String> {
        let tokens = self.tokens;
        let open = self.at + 1;
        if !is(tokens.get(open), "(") {
            return Err("a for without its parentheses".into());
        }
        let close = self.closing(open)?;
        let parts: Vec<&[Token]> = tokens[open + 1..close].split(|token| is(Some(token), ";")).collect();
        self.at = close + 1;
        let body_start = self.at;
        let body = self.statement()?;
        let body_tokens = &tokens[body_start..self.at];
        let bounds = match parts.as_slice() {
            [init, test, step] => bounds(init, test, step),
            _ => None,
        };
        let Some((counter, times)) = bounds else {
            self.found.push("code: a for loop without literal bounds: Kumi builds a for only as for (i = 0; i < 16; i += 1), with numbers for its start, end and step, so it's known to end.".into());
            return Ok(Node::For { times: 1, body: Box::new(body) });
        };
        let assigned = body_tokens.windows(2).any(|pair| {
            matches!(&pair[0], Token::Word(w) if *w == counter)
                && matches!(&pair[1], Token::Symbol(s) if ["=", "+=", "-=", "*=", "/="].contains(&s.as_str()))
        });
        if assigned {
            self.found.push(format!("code: a for loop changes its own counter {counter} in its body, so its bounds aren't what it runs: change {counter} only in the loop's step."));
        }
        match times {
            None => self.found.push("code: a for loop never ends (its step never reaches its end): Live's audio thread would stop.".into()),
            Some(times) if times > MOST_ITERATIONS => self.found.push(format!(
                "code: a for loop runs {times} times each sample, past the {MOST_ITERATIONS} Kumi allows on Live's audio thread."
            )),
            Some(_) => {}
        }
        Ok(Node::For { times: times.unwrap_or(1), body: Box::new(body) })
    }
}

/// A literal number, maybe negative.
fn number(tokens: &[Token]) -> Option<f64> {
    match tokens {
        [Token::Number(n)] => Some(*n),
        [Token::Symbol(minus), Token::Number(n)] if minus == "-" => Some(-n),
        _ => None,
    }
}

/// A for's counter and how many times it runs (None: never ends), when its header is `i = A; i <op> B; i += C`.
fn bounds(init: &[Token], test: &[Token], step: &[Token]) -> Option<(String, Option<u64>)> {
    let (counter, start) = match init {
        [Token::Word(counter), Token::Symbol(eq), rest @ ..] if eq == "=" => (counter.clone(), number(rest)?),
        _ => return None,
    };
    let (compare, end) = match test {
        [Token::Word(w), Token::Symbol(compare), rest @ ..] if *w == counter => (compare.clone(), number(rest)?),
        _ => return None,
    };
    let by = match step {
        [Token::Word(w), Token::Symbol(op), rest @ ..] if *w == counter && op == "+=" => number(rest)?,
        [Token::Word(w), Token::Symbol(op), rest @ ..] if *w == counter && op == "-=" => -number(rest)?,
        [Token::Word(w), Token::Symbol(eq), Token::Word(again), Token::Symbol(op), rest @ ..]
            if *w == counter && eq == "=" && *again == counter =>
        {
            match op.as_str() {
                "+" => number(rest)?,
                "-" => -number(rest)?,
                _ => return None,
            }
        }
        _ => return None,
    };
    let holds = |i: f64| match compare.as_str() {
        "<" => i < end,
        "<=" => i <= end,
        ">" => i > end,
        ">=" => i >= end,
        "!=" => i != end,
        _ => false,
    };
    if !["<", "<=", ">", ">=", "!="].contains(&compare.as_str()) {
        return None;
    }
    let (mut i, mut times) = (start, 0u64);
    while holds(i) {
        times += 1;
        if times > SIMULATED {
            return Some((counter, None));
        }
        i += by;
    }
    Some((counter, Some(times)))
}

/// A statement's own operations and what it calls.
fn plain(tokens: &[Token]) -> Node {
    let mut operations = 0u64;
    let mut calls = Vec::new();
    for (at, token) in tokens.iter().enumerate() {
        match token {
            Token::Symbol(s) if OPERATORS.contains(&s.as_str()) => operations += 1,
            Token::Word(name) if is(tokens.get(at + 1), "(") => {
                // A method (d.read(…)) is one of gen's: one operation.
                if at > 0 && is(tokens.get(at - 1), ".") {
                    operations += 1;
                } else {
                    calls.push(name.clone());
                }
            }
            _ => {}
        }
    }
    Node::Plain { operations, calls }
}

/// What a part of the code costs in the worst case each sample: its operations, and how deep its loops nest.
#[derive(Debug, Clone, Copy, Default)]
struct Cost {
    operations: u64,
    depth: usize,
}

struct Costs<'a> {
    functions: &'a HashMap<String, Node>,
    known: HashMap<String, Cost>,
    visiting: Vec<String>,
}

impl Costs<'_> {
    fn nodes(&mut self, nodes: &[Node]) -> Result<Cost, String> {
        let mut total = Cost::default();
        for node in nodes {
            let cost = self.node(node)?;
            total = Cost { operations: total.operations.saturating_add(cost.operations), depth: total.depth.max(cost.depth) };
        }
        Ok(total)
    }

    fn node(&mut self, node: &Node) -> Result<Cost, String> {
        Ok(match node {
            Node::Plain { operations, calls } => {
                let mut cost = Cost { operations: *operations, depth: 0 };
                for name in calls {
                    let called = self.function(name)?;
                    cost = Cost { operations: cost.operations.saturating_add(called.operations), depth: cost.depth.max(called.depth) };
                }
                cost
            }
            Node::Block(nodes) => self.nodes(nodes)?,
            Node::If { test, then, otherwise } => {
                let (test, then) = (self.node(test)?, self.node(then)?);
                let otherwise = match otherwise {
                    Some(otherwise) => self.node(otherwise)?,
                    None => Cost::default(),
                };
                Cost {
                    operations: test.operations.saturating_add(then.operations.max(otherwise.operations)),
                    depth: test.depth.max(then.depth).max(otherwise.depth),
                }
            }
            Node::For { times, body } => {
                let body = self.node(body)?;
                Cost { operations: body.operations.saturating_mul(*times), depth: 1 + body.depth }
            }
        })
    }

    /// What a call costs: a function of the code's, counted whole; one of gen's, one operation.
    fn function(&mut self, name: &str) -> Result<Cost, String> {
        let Some(body) = self.functions.get(name) else { return Ok(Cost { operations: 1, depth: 0 }) };
        if let Some(cost) = self.known.get(name) {
            return Ok(*cost);
        }
        if self.visiting.iter().any(|visiting| visiting == name) {
            return Err(format!("the function {name} calls itself (through {}), which gen~ can't run", self.visiting.join(" → ")));
        }
        self.visiting.push(name.to_string());
        let cost = self.node(body)?;
        self.visiting.pop();
        let cost = Cost { operations: cost.operations.saturating_add(1), depth: cost.depth };
        self.known.insert(name.to_string(), cost);
        Ok(cost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(code: &str, why: &str) {
        let found = check(code);
        assert!(found.iter().any(|said| said.contains(why)), "{code:?} should say {why:?}: {found:?}");
    }

    const EFFECT: &str = "// a soft clipper with a short echo\nParam drive(1, min=0, max=4);\nDelay echo(4800);\nData table(512, 2);\nHistory last(0);\n\nshape(x, k) {\n    return tanh(x * k) / tanh(k);\n}\n\nwet = shape(in1, drive);\nsum = 0;\nfor (i = 0; i < 8; i += 1) {\n    sum += peek(table, i, 0) * 0.125;\n}\nif (wet > 0.5) { last = wet; } else last = last * 0.99;\necho.write(wet);\nout1 = mix(in1, echo.read(2400) + sum, 0.5);\nout2 = out1;\n";

    #[test]
    fn a_normal_effect_and_data_pass() {
        assert_eq!(check(EFFECT), Vec::<String>::new());
        assert_eq!(check("Data d(2097152);\nout1 = in1; out2 = in2;\n"), Vec::<String>::new(), "Data at its limit");
    }

    #[test]
    fn loops_that_could_hold_lives_audio_thread_are_refused() {
        refused("x = 0;\nwhile (x < 4) { x += 1; }\nout1 = x;\n", "while loop");
        refused("x = 0;\ndo { x += 1; } while (x < 4);\nout1 = x;\n", "do loop");
        refused("Param n(4);\nfor (i = 0; i < n; i += 1) { out1 = i; }\n", "without literal bounds");
        refused("for (i = 0; i < 65; i += 1) { out1 = i; }\n", "runs 65 times");
        refused("for (i = 0; i < 4; i -= 1) { out1 = i; }\n", "never ends");
        refused("for (i = 0; i < 4; i += 1) { i = 0; }\n", "changes its own counter");
        refused("for (i = 0; i < 4; i += 1) { for (j = 0; j < 4; j += 1) { for (k = 0; k < 4; k += 1) { out1 = k; } } }\n", "nest 3 deep");
        refused(
            "inner(x) { s = 0; for (k = 0; k < 4; k += 1) { s += x; } return s; }\nfor (i = 0; i < 4; i += 1) { for (j = 0; j < 4; j += 1) { out1 = inner(j); } }\n",
            "nest 3 deep",
        );
        assert_eq!(check("for (i = 64; i > 0; i = i - 1) { out1 = i; }\n"), Vec::<String>::new(), "64 times, counting down");
    }

    #[test]
    fn buffer_and_unbounded_memory_are_refused() {
        refused("Buffer b(\"shared\");\nout1 = peek(b, 0);\n", "declares a Buffer");
        refused("Delay d(4194304);\nout1 = d.read(1);\n", "holds 4194304 samples");
        refused("Data d(2097152, 2);\nout1 = in1;\n", "holds 4194304 samples");
        refused("Delay d(samplerate);\nout1 = d.read(1);\n", "needs its size as a literal");
        refused(
            "Delay a(2000000);\nDelay b(2000000);\nDelay c(2000000);\nDelay d(2000000);\nData e(1000000);\nout1 = in1;\n",
            "hold 9000000 samples together",
        );
    }

    #[test]
    fn the_worst_case_per_sample_is_budgeted() {
        // 64 × 64 of a one-operation body is the budget; of a two-operation body, past it.
        assert_eq!(check("for (i = 0; i < 64; i += 1) { for (j = 0; j < 64; j += 1) { out1 = in1 * 2; } }\n"), Vec::<String>::new());
        refused(
            "for (i = 0; i < 64; i += 1) { for (j = 0; j < 64; j += 1) { out1 = in1 * 2 + 1; } }\n",
            "takes 8192 operations each sample",
        );
        let deep = "heavy(x) { return x * x + x * x + x * x + x * x; }\n";
        let calls = (0..600).map(|_| "heavy(in1)").collect::<Vec<_>>().join(" + ");
        refused(&format!("{deep}out1 = {calls};\n"), "operations each sample");
        refused("a(x) { return b(x); }\nb(x) { return a(x); }\nout1 = a(in1);\n", "calls itself");
    }

    #[test]
    fn code_past_the_one_cap_is_refused() {
        refused(&"out1 = in1;\n".repeat(401), "401 lines");
        refused(&format!("// {}\nout1 = in1;\n", "x".repeat(33_000)), "past the 400 lines or 32 KB");
    }
}
