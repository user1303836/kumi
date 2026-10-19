//! A GenExpr interpreter for the part of the language Kumi's own building blocks are written in, so their tests run the
//! very code a device ships, one sample at a time. It isn't Max's compiler. It reads:
//! - Param, History and Buffer declarations;
//! - assignments (=, +=, -=, *=, /=), if/else, and poke(buffer, value, index) into a Buffer;
//! - expressions of numbers, names and in1…inN, using + - * / %, comparisons, && || !, and ?:;
//! - the operators the building blocks call.
//!
//! A History keeps its value from one sample to the next. With `delayed`, a History's new value lands only at the next
//! sample, which shows the code means the same whichever way gen~ orders them. Anything else (an unknown name or
//! operator, a local read before it's set) fails the test that runs it.

pub mod live;

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Word(String),
    Symbol(String),
}

fn tokens(code: &str) -> Vec<Token> {
    let chars: Vec<char> = code.chars().collect();
    let mut out = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        if c.is_whitespace() {
            at += 1;
        } else if c == '/' && chars.get(at + 1) == Some(&'/') {
            while at < chars.len() && chars[at] != '\n' {
                at += 1;
            }
        } else if c == '/' && chars.get(at + 1) == Some(&'*') {
            at += 2;
            while at + 1 < chars.len() && !(chars[at] == '*' && chars[at + 1] == '/') {
                at += 1;
            }
            at += 2;
        } else if c.is_ascii_digit() || (c == '.' && chars.get(at + 1).is_some_and(char::is_ascii_digit)) {
            let start = at;
            while at < chars.len() && (chars[at].is_ascii_digit() || chars[at] == '.') {
                at += 1;
            }
            if at < chars.len() && (chars[at] == 'e' || chars[at] == 'E') {
                at += 1;
                if at < chars.len() && (chars[at] == '-' || chars[at] == '+') {
                    at += 1;
                }
                while at < chars.len() && chars[at].is_ascii_digit() {
                    at += 1;
                }
            }
            let text: String = chars[start..at].iter().collect();
            out.push(Token::Number(text.parse().unwrap_or_else(|_| panic!("a number: {text}"))));
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = at;
            while at < chars.len() && (chars[at].is_ascii_alphanumeric() || chars[at] == '_') {
                at += 1;
            }
            out.push(Token::Word(chars[start..at].iter().collect()));
        } else {
            let two: String = chars[at..(at + 2).min(chars.len())].iter().collect();
            if ["==", "!=", "<=", ">=", "&&", "||", "+=", "-=", "*=", "/="].contains(&two.as_str()) {
                out.push(Token::Symbol(two));
                at += 2;
            } else {
                out.push(Token::Symbol(c.to_string()));
                at += 1;
            }
        }
    }
    out
}

#[derive(Debug, Clone)]
enum Expr {
    Number(f64),
    Name(String),
    Not(Box<Expr>),
    Negate(Box<Expr>),
    Binary(String, Box<Expr>, Box<Expr>),
    Choose(Box<Expr>, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

#[derive(Debug, Clone)]
enum Statement {
    Assign(String, String, Expr),
    If(Expr, Vec<Statement>, Vec<Statement>),
    /// poke(buffer, value, index): writes into a Buffer.
    Poke(String, Expr, Expr),
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }
    fn next(&mut self) -> Token {
        let token = self.tokens.get(self.at).cloned().expect("more code");
        self.at += 1;
        token
    }
    fn symbol(&self, symbol: &str) -> bool {
        matches!(self.peek(), Some(Token::Symbol(s)) if s == symbol)
    }
    fn word(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Token::Word(w)) if w == word)
    }
    fn expect(&mut self, symbol: &str) {
        let token = self.next();
        assert_eq!(token, Token::Symbol(symbol.into()), "expected {symbol} at token {}", self.at);
    }
    fn name(&mut self) -> String {
        match self.next() {
            Token::Word(word) => word,
            other => panic!("expected a name, not {other:?}"),
        }
    }

    fn block(&mut self) -> Vec<Statement> {
        if self.symbol("{") {
            self.expect("{");
            let mut body = Vec::new();
            while !self.symbol("}") {
                body.push(self.statement());
            }
            self.expect("}");
            body
        } else {
            vec![self.statement()]
        }
    }

    fn statement(&mut self) -> Statement {
        if self.word("if") {
            self.next();
            self.expect("(");
            let test = self.expression();
            self.expect(")");
            let then = self.block();
            let otherwise = if self.word("else") {
                self.next();
                self.block()
            } else {
                Vec::new()
            };
            return Statement::If(test, then, otherwise);
        }
        let target = self.name();
        if target == "poke" {
            self.expect("(");
            let buffer = self.name();
            self.expect(",");
            let value = self.expression();
            self.expect(",");
            let index = self.expression();
            // A channel (and more) may follow; the building blocks write channel 0.
            while self.symbol(",") {
                self.next();
                self.expression();
            }
            self.expect(")");
            self.expect(";");
            return Statement::Poke(buffer, value, index);
        }
        let operator = match self.next() {
            Token::Symbol(symbol) if ["=", "+=", "-=", "*=", "/="].contains(&symbol.as_str()) => symbol,
            other => panic!("expected an assignment to {target}, not {other:?}"),
        };
        let value = self.expression();
        self.expect(";");
        Statement::Assign(target, operator, value)
    }

    fn expression(&mut self) -> Expr {
        let test = self.binary(0);
        if self.symbol("?") {
            self.next();
            let yes = self.expression();
            self.expect(":");
            let no = self.expression();
            return Expr::Choose(Box::new(test), Box::new(yes), Box::new(no));
        }
        test
    }

    fn binary(&mut self, level: usize) -> Expr {
        const LEVELS: [&[&str]; 5] = [&["||"], &["&&"], &["<", "<=", ">", ">=", "==", "!="], &["+", "-"], &["*", "/", "%"]];
        if level == LEVELS.len() {
            return self.unary();
        }
        let mut left = self.binary(level + 1);
        while let Some(Token::Symbol(symbol)) = self.peek().cloned() {
            if !LEVELS[level].contains(&symbol.as_str()) {
                break;
            }
            self.next();
            let right = self.binary(level + 1);
            left = Expr::Binary(symbol, Box::new(left), Box::new(right));
        }
        left
    }

    fn unary(&mut self) -> Expr {
        if self.symbol("-") {
            self.next();
            return Expr::Negate(Box::new(self.unary()));
        }
        if self.symbol("!") {
            self.next();
            return Expr::Not(Box::new(self.unary()));
        }
        match self.next() {
            Token::Number(number) => Expr::Number(number),
            Token::Symbol(symbol) if symbol == "(" => {
                let inner = self.expression();
                self.expect(")");
                inner
            }
            Token::Word(word) if self.symbol("(") => {
                self.next();
                let mut arguments = Vec::new();
                while !self.symbol(")") {
                    arguments.push(self.expression());
                    if self.symbol(",") {
                        self.next();
                    }
                }
                self.expect(")");
                Expr::Call(word, arguments)
            }
            Token::Word(word) => Expr::Name(word),
            other => panic!("unexpected {other:?}"),
        }
    }
}

/// What a name in the code is, once it's read: a slot of the code's own (a Param, a History or a local), an inlet, an
/// outlet, or one of gen's constants.
#[derive(Debug, Clone, Copy)]
enum Name {
    Slot(usize),
    Input(usize),
    Output(usize),
    Constant(f64),
}

/// An expression with every name read.
#[derive(Debug, Clone)]
enum Op {
    Number(f64),
    Read(Name),
    Not(Box<Op>),
    Negate(Box<Op>),
    Binary(u8, Box<Op>, Box<Op>),
    Choose(Box<Op>, Box<Op>, Box<Op>),
    Call(u8, Vec<Op>),
}

#[derive(Debug, Clone)]
enum Step {
    Assign(Name, u8, Op),
    If(Op, Vec<Step>, Vec<Step>),
    Poke(usize, Op, Op),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Param,
    History,
    Local,
}

/// One gen~ codebox, run a sample at a time.
pub struct Gen {
    /// Each Buffer the code declares, by name, and what it holds (what a [peek~] of its buffer~ reads).
    pub buffers: Vec<(String, Vec<f64>)>,
    names: Vec<String>,
    kinds: Vec<Kind>,
    values: Vec<f64>,
    body: Vec<Step>,
    delayed: bool,
    outputs: usize,
    pub samplerate: f64,
    pub vectorsize: f64,
}

impl Gen {
    /// The code's Params and Histories with their first values, and its statements. `delayed`: a History's new value
    /// lands at the next sample.
    pub fn new(code: &str, delayed: bool) -> Gen {
        let mut parser = Parser { tokens: tokens(code), at: 0 };
        let mut gen = Gen {
            buffers: Vec::new(),
            names: Vec::new(),
            kinds: Vec::new(),
            values: Vec::new(),
            body: Vec::new(),
            delayed,
            outputs: 0,
            samplerate: 44_100.0,
            vectorsize: 64.0,
        };
        let mut statements = Vec::new();
        while parser.peek().is_some() {
            if parser.word("Buffer") {
                parser.next();
                let name = parser.name();
                parser.expect(";");
                gen.buffers.push((name, vec![0.0; 16]));
                continue;
            }
            if parser.word("Param") || parser.word("History") {
                let kind = if parser.name() == "Param" { Kind::Param } else { Kind::History };
                loop {
                    let name = parser.name();
                    parser.expect("(");
                    let first = eval_constant(&parser.expression());
                    // A Param's settings (min=…, max=…) aren't used here.
                    while parser.symbol(",") {
                        parser.next();
                        parser.name();
                        parser.expect("=");
                        parser.expression();
                    }
                    parser.expect(")");
                    assert!(!gen.names.contains(&name), "{name} is declared twice");
                    gen.names.push(name);
                    gen.kinds.push(kind);
                    gen.values.push(first);
                    if parser.symbol(",") {
                        parser.next();
                    } else {
                        break;
                    }
                }
                parser.expect(";");
            } else {
                statements.push(parser.statement());
            }
        }
        gen.body = statements.iter().map(|statement| gen.step(statement)).collect();
        gen
    }

    fn name(&mut self, name: &str, assigned: bool) -> Name {
        match name {
            "samplerate" => return Name::Constant(self.samplerate),
            "vectorsize" => return Name::Constant(self.vectorsize),
            "pi" => return Name::Constant(std::f64::consts::PI),
            "twopi" => return Name::Constant(std::f64::consts::TAU),
            _ => {}
        }
        if let Some(index) = name.strip_prefix("in").and_then(|n| n.parse::<usize>().ok()).filter(|_| !assigned) {
            return Name::Input(index - 1);
        }
        if let Some(index) = name.strip_prefix("out").and_then(|n| n.parse::<usize>().ok()).filter(|_| assigned) {
            self.outputs = self.outputs.max(index);
            return Name::Output(index - 1);
        }
        match self.names.iter().position(|known| known == name) {
            Some(at) => {
                assert!(!(assigned && self.kinds[at] == Kind::Param), "the code assigns the Param {name}");
                Name::Slot(at)
            }
            None => {
                self.names.push(name.to_string());
                self.kinds.push(Kind::Local);
                self.values.push(0.0);
                Name::Slot(self.names.len() - 1)
            }
        }
    }

    fn step(&mut self, statement: &Statement) -> Step {
        match statement {
            Statement::Assign(target, operator, value) => {
                let value = self.op(value);
                let code = match operator.as_str() {
                    "=" => 0,
                    "+=" => 1,
                    "-=" => 2,
                    "*=" => 3,
                    _ => 4,
                };
                Step::Assign(self.name(target, true), code, value)
            }
            Statement::If(test, then, otherwise) => {
                let test = self.op(test);
                let then = then.iter().map(|statement| self.step(statement)).collect();
                let otherwise = otherwise.iter().map(|statement| self.step(statement)).collect();
                Step::If(test, then, otherwise)
            }
            Statement::Poke(buffer, value, index) => {
                let at = self.buffers.iter().position(|(name, _)| name == buffer).unwrap_or_else(|| panic!("no Buffer {buffer}"));
                Step::Poke(at, self.op(value), self.op(index))
            }
        }
    }

    fn op(&mut self, expr: &Expr) -> Op {
        match expr {
            Expr::Number(number) => Op::Number(*number),
            Expr::Name(name) => Op::Read(self.name(name, false)),
            Expr::Not(inner) => Op::Not(Box::new(self.op(inner))),
            Expr::Negate(inner) => Op::Negate(Box::new(self.op(inner))),
            Expr::Choose(test, yes, no) => Op::Choose(Box::new(self.op(test)), Box::new(self.op(yes)), Box::new(self.op(no))),
            Expr::Binary(operator, left, right) => {
                const OPERATORS: [&str; 14] = ["+", "-", "*", "/", "%", "<", "<=", ">", ">=", "==", "!=", "&&", "||", ""];
                let code = OPERATORS.iter().position(|known| known == operator).expect("an operator") as u8;
                Op::Binary(code, Box::new(self.op(left)), Box::new(self.op(right)))
            }
            Expr::Call(name, arguments) => {
                let code = match (name.as_str(), arguments.len()) {
                    ("floor", 1) => 0,
                    ("ceil", 1) => 1,
                    ("abs", 1) => 2,
                    ("min", 2) => 3,
                    ("max", 2) => 4,
                    ("clamp", 3) => 5,
                    _ => panic!("{name} with {} arguments isn't one the building blocks use", arguments.len()),
                };
                Op::Call(code, arguments.iter().map(|argument| self.op(argument)).collect())
            }
        }
    }

    /// A message "name value" to the gen~: sets its Param.
    pub fn set(&mut self, name: &str, value: f64) {
        let at = self.names.iter().position(|known| known == name).filter(|at| self.kinds[*at] == Kind::Param);
        let at = at.unwrap_or_else(|| panic!("no Param {name}"));
        self.values[at] = value;
    }

    /// One sample: the inlets' values in, the outlets' values out.
    pub fn tick(&mut self, inputs: &[f64], outputs: usize) -> Vec<f64> {
        assert!(outputs >= self.outputs, "the code sends {} outlets", self.outputs);
        let mut run = Run { inputs, outputs: vec![0.0; outputs], set: vec![false; self.names.len()], pending: Vec::new() };
        let body = std::mem::take(&mut self.body);
        self.steps(&body, &mut run);
        self.body = body;
        for (at, value) in run.pending {
            self.values[at] = value;
        }
        run.outputs
    }

    fn steps(&mut self, body: &[Step], run: &mut Run) {
        for step in body {
            match step {
                Step::If(test, then, otherwise) => {
                    if self.eval(test, run) != 0.0 {
                        self.steps(then, run);
                    } else {
                        self.steps(otherwise, run);
                    }
                }
                Step::Poke(at, value, index) => {
                    let (value, index) = (self.eval(value, run), self.eval(index, run));
                    let index = index.floor();
                    assert!(index >= 0.0 && (index as usize) < self.buffers[*at].1.len(), "poke past the buffer: {index}");
                    self.buffers[*at].1[index as usize] = value;
                }
                Step::Assign(target, operator, value) => {
                    let value = self.eval(value, run);
                    let value = match operator {
                        0 => value,
                        _ => {
                            let was = self.read(*target, run);
                            match operator {
                                1 => was + value,
                                2 => was - value,
                                3 => was * value,
                                _ => was / value,
                            }
                        }
                    };
                    assert!(value.is_finite(), "a value isn't finite: {value}");
                    match *target {
                        Name::Output(index) => run.outputs[index] = value,
                        Name::Slot(at) if self.kinds[at] == Kind::History && self.delayed => run.pending.push((at, value)),
                        Name::Slot(at) => {
                            self.values[at] = value;
                            run.set[at] = true;
                        }
                        Name::Input(_) | Name::Constant(_) => panic!("the code assigns an inlet or a constant"),
                    }
                }
            }
        }
    }

    fn read(&self, name: Name, run: &Run) -> f64 {
        match name {
            Name::Constant(value) => value,
            Name::Input(index) => *run.inputs.get(index).unwrap_or_else(|| panic!("in{} has no value", index + 1)),
            Name::Output(index) => run.outputs[index],
            Name::Slot(at) => {
                assert!(self.kinds[at] != Kind::Local || run.set[at], "{} is read before it's set", self.names[at]);
                self.values[at]
            }
        }
    }

    fn eval(&self, op: &Op, run: &Run) -> f64 {
        let truth = |value: bool| if value { 1.0 } else { 0.0 };
        match op {
            Op::Number(number) => *number,
            Op::Read(name) => self.read(*name, run),
            Op::Negate(inner) => -self.eval(inner, run),
            Op::Not(inner) => truth(self.eval(inner, run) == 0.0),
            Op::Choose(test, yes, no) => {
                if self.eval(test, run) != 0.0 {
                    self.eval(yes, run)
                } else {
                    self.eval(no, run)
                }
            }
            Op::Binary(operator, left, right) => {
                let (a, b) = (self.eval(left, run), self.eval(right, run));
                match operator {
                    0 => a + b,
                    1 => a - b,
                    2 => a * b,
                    3 => {
                        assert!(b != 0.0, "a division by 0");
                        a / b
                    }
                    4 => a % b,
                    5 => truth(a < b),
                    6 => truth(a <= b),
                    7 => truth(a > b),
                    8 => truth(a >= b),
                    9 => truth(a == b),
                    10 => truth(a != b),
                    11 => truth(a != 0.0 && b != 0.0),
                    _ => truth(a != 0.0 || b != 0.0),
                }
            }
            Op::Call(function, arguments) => {
                let value = |at: usize| self.eval(&arguments[at], run);
                match function {
                    0 => value(0).floor(),
                    1 => value(0).ceil(),
                    2 => value(0).abs(),
                    3 => value(0).min(value(1)),
                    4 => value(0).max(value(1)),
                    _ => value(0).max(value(1)).min(value(2)),
                }
            }
        }
    }
}

fn eval_constant(expr: &Expr) -> f64 {
    match expr {
        Expr::Number(number) => *number,
        Expr::Negate(inner) => -eval_constant(inner),
        other => panic!("a declaration's value is a number, not {other:?}"),
    }
}

struct Run<'a> {
    inputs: &'a [f64],
    outputs: Vec<f64>,
    /// Which locals are set this sample.
    set: Vec<bool>,
    pending: Vec<(usize, f64)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_reads_what_the_building_blocks_are_written_in() {
        let code = "// a comment\nParam rate(2, min=0, max=4);\nHistory count(0), total(-1.5e1);\n/* and another */\nx = in1 * rate + 1;\nif (x > 3 && !(x == 9)) { count += 1; } else if (x < 0) count = -1; else { count = count; }\ntotal = total + (x >= 5 ? x : 0) % 4;\nout1 = count; out2 = total; out3 = floor(-0.5) + ceil(0.2) + abs(-2) + min(1, 2) + max(1, 2) + clamp(7, 0, 5);\n";
        let mut gen = Gen::new(code, false);
        assert_eq!(gen.tick(&[1.0], 3), vec![0.0, -15.0, -1.0 + 1.0 + 2.0 + 1.0 + 2.0 + 5.0]);
        assert_eq!(gen.tick(&[2.0], 3), vec![1.0, -14.0, 10.0], "5 % 4 is 1; the History kept its value");
        gen.set("rate", 0.0);
        assert_eq!(gen.tick(&[2.0], 3)[0], 1.0);
    }

    #[test]
    fn poke_writes_a_buffer_that_reads_back_at_once() {
        let mut gen = Gen::new("Buffer memory;\nHistory n(0);\nn = n + 1;\npoke(memory, n * 10, 1, 0);\nout1 = n;\n", false);
        gen.tick(&[], 1);
        gen.tick(&[], 1);
        assert_eq!(gen.buffers[0].1[1], 20.0);
    }

    #[test]
    fn a_delayed_history_lands_at_the_next_sample() {
        let code = "History h(0);\nh = h + 1;\nout1 = h;\n";
        assert_eq!(Gen::new(code, false).tick(&[], 1), vec![1.0]);
        let mut delayed = Gen::new(code, true);
        assert_eq!(delayed.tick(&[], 1), vec![0.0]);
        assert_eq!(delayed.tick(&[], 1), vec![1.0]);
    }

    #[test]
    #[should_panic(expected = "read before it's set")]
    fn a_local_read_before_its_set_fails() {
        Gen::new("out1 = later;\nlater = 1;\n", false).tick(&[], 1);
    }
}
