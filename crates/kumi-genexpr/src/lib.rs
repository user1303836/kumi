//! A GenExpr interpreter for the part of the language Kumi's own code is written in, so its tests (and a device's build)
//! run the very code a device ships, one sample at a time. It isn't Max's compiler. It reads:
//! - Param, History and Buffer declarations;
//! - assignments (=, +=, -=, *=, /=), if/else, and poke(buffer, value, index) into a Buffer;
//! - expressions of numbers, names and in1…inN, using + - * / %, comparisons, && || !, and ?:;
//! - the operators Kumi's code calls: floor, ceil, abs, min, max, clamp, mix, dbtoa, fixnan, fixdenorm, isnan,
//!   isdenorm and dcblock (each dcblock in the code keeps its own state).
//!
//! A History keeps its value from one sample to the next. With `delayed`, a History's new value lands only at the next
//! sample, which shows the code means the same whichever way gen~ orders them. Code it can't run (an unknown name or
//! operator, a local read before it's set, a value that isn't finite) is an error, never a panic.

pub mod guard;
pub mod limits;
pub mod live;

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Word(String),
    Symbol(String),
    /// A quoted string: a Buffer's or a Data's name in Max.
    Text(String),
}

fn tokens(code: &str) -> Result<Vec<Token>, String> {
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
        } else if c == '"' || c == '\'' {
            let start = at + 1;
            at = start;
            while at < chars.len() && chars[at] != c {
                at += 1;
            }
            if at >= chars.len() {
                return Err("a string that never ends".into());
            }
            out.push(Token::Text(chars[start..at].iter().collect()));
            at += 1;
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
            out.push(Token::Number(text.parse().map_err(|_| format!("{text} isn't a number"))?));
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
    Ok(out)
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
    fn next(&mut self) -> Result<Token, String> {
        let token = self.tokens.get(self.at).cloned().ok_or("the code ends too soon")?;
        self.at += 1;
        Ok(token)
    }
    fn symbol(&self, symbol: &str) -> bool {
        matches!(self.peek(), Some(Token::Symbol(s)) if s == symbol)
    }
    fn word(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Token::Word(w)) if w == word)
    }
    fn expect(&mut self, symbol: &str) -> Result<(), String> {
        match self.next()? {
            Token::Symbol(found) if found == symbol => Ok(()),
            other => Err(format!("expected {symbol} at token {}, not {other:?}", self.at)),
        }
    }
    fn name(&mut self) -> Result<String, String> {
        match self.next()? {
            Token::Word(word) => Ok(word),
            other => Err(format!("expected a name, not {other:?}")),
        }
    }

    fn block(&mut self) -> Result<Vec<Statement>, String> {
        if self.symbol("{") {
            self.expect("{")?;
            let mut body = Vec::new();
            while !self.symbol("}") {
                body.push(self.statement()?);
            }
            self.expect("}")?;
            Ok(body)
        } else {
            Ok(vec![self.statement()?])
        }
    }

    fn statement(&mut self) -> Result<Statement, String> {
        if self.word("if") {
            self.next()?;
            self.expect("(")?;
            let test = self.expression()?;
            self.expect(")")?;
            let then = self.block()?;
            let otherwise = if self.word("else") {
                self.next()?;
                self.block()?
            } else {
                Vec::new()
            };
            return Ok(Statement::If(test, then, otherwise));
        }
        let target = self.name()?;
        if target == "poke" {
            self.expect("(")?;
            let buffer = self.name()?;
            self.expect(",")?;
            let value = self.expression()?;
            self.expect(",")?;
            let index = self.expression()?;
            // A channel (and more) may follow; the building blocks write channel 0.
            while self.symbol(",") {
                self.next()?;
                self.expression()?;
            }
            self.expect(")")?;
            self.expect(";")?;
            return Ok(Statement::Poke(buffer, value, index));
        }
        let operator = match self.next()? {
            Token::Symbol(symbol) if ["=", "+=", "-=", "*=", "/="].contains(&symbol.as_str()) => symbol,
            other => return Err(format!("expected an assignment to {target}, not {other:?}")),
        };
        let value = self.expression()?;
        self.expect(";")?;
        Ok(Statement::Assign(target, operator, value))
    }

    fn expression(&mut self) -> Result<Expr, String> {
        let test = self.binary(0)?;
        if self.symbol("?") {
            self.next()?;
            let yes = self.expression()?;
            self.expect(":")?;
            let no = self.expression()?;
            return Ok(Expr::Choose(Box::new(test), Box::new(yes), Box::new(no)));
        }
        Ok(test)
    }

    fn binary(&mut self, level: usize) -> Result<Expr, String> {
        const LEVELS: [&[&str]; 5] = [&["||"], &["&&"], &["<", "<=", ">", ">=", "==", "!="], &["+", "-"], &["*", "/", "%"]];
        if level == LEVELS.len() {
            return self.unary();
        }
        let mut left = self.binary(level + 1)?;
        while let Some(Token::Symbol(symbol)) = self.peek().cloned() {
            if !LEVELS[level].contains(&symbol.as_str()) {
                break;
            }
            self.next()?;
            let right = self.binary(level + 1)?;
            left = Expr::Binary(symbol, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.symbol("-") {
            self.next()?;
            return Ok(Expr::Negate(Box::new(self.unary()?)));
        }
        if self.symbol("!") {
            self.next()?;
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        match self.next()? {
            Token::Number(number) => Ok(Expr::Number(number)),
            Token::Symbol(symbol) if symbol == "(" => {
                let inner = self.expression()?;
                self.expect(")")?;
                Ok(inner)
            }
            Token::Word(word) if self.symbol("(") => {
                self.next()?;
                let mut arguments = Vec::new();
                while !self.symbol(")") {
                    arguments.push(self.expression()?);
                    if self.symbol(",") {
                        self.next()?;
                    }
                }
                self.expect(")")?;
                Ok(Expr::Call(word, arguments))
            }
            Token::Word(word) => Ok(Expr::Name(word)),
            other => Err(format!("unexpected {other:?}")),
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

/// The operators the interpreter runs, by name and how many arguments each takes.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Function {
    Floor,
    Ceil,
    Abs,
    Min,
    Max,
    Clamp,
    Mix,
    DbToA,
    FixNan,
    FixDenorm,
    IsNan,
    IsDenorm,
}

impl Function {
    fn of(name: &str, arguments: usize) -> Option<Function> {
        Some(match (name, arguments) {
            ("floor", 1) => Function::Floor,
            ("ceil", 1) => Function::Ceil,
            ("abs", 1) => Function::Abs,
            ("min", 2) => Function::Min,
            ("max", 2) => Function::Max,
            ("clamp", 3) => Function::Clamp,
            ("mix", 3) => Function::Mix,
            ("dbtoa", 1) => Function::DbToA,
            ("fixnan", 1) => Function::FixNan,
            ("fixdenorm", 1) => Function::FixDenorm,
            ("isnan", 1) => Function::IsNan,
            ("isdenorm", 1) => Function::IsDenorm,
            _ => return None,
        })
    }
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
    Call(Function, Vec<Op>),
    /// dcblock(x), with the index of its own state.
    DcBlock(usize, Box<Op>),
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

/// One dcblock's state, as genlib's: the last input and output.
#[derive(Debug, Clone, Copy, Default)]
struct DcBlock {
    x1: f64,
    y1: f64,
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
    dcblocks: Vec<DcBlock>,
    pub samplerate: f64,
    pub vectorsize: f64,
}

impl Gen {
    /// The code's Params and Histories with their first values, and its statements. `delayed`: a History's new value
    /// lands at the next sample. Err says why the code can't run here.
    pub fn new(code: &str, delayed: bool) -> Result<Gen, String> {
        let mut parser = Parser { tokens: tokens(code)?, at: 0 };
        let mut gen = Gen {
            buffers: Vec::new(),
            names: Vec::new(),
            kinds: Vec::new(),
            values: Vec::new(),
            body: Vec::new(),
            delayed,
            outputs: 0,
            dcblocks: Vec::new(),
            samplerate: 44_100.0,
            vectorsize: 64.0,
        };
        let mut statements = Vec::new();
        while parser.peek().is_some() {
            if parser.word("Buffer") {
                parser.next()?;
                let name = parser.name()?;
                parser.expect(";")?;
                gen.buffers.push((name, vec![0.0; 16]));
                continue;
            }
            if parser.word("Param") || parser.word("History") {
                let kind = if parser.name()? == "Param" { Kind::Param } else { Kind::History };
                loop {
                    let name = parser.name()?;
                    parser.expect("(")?;
                    let first = eval_constant(&parser.expression()?)?;
                    // A Param's settings (min=…, max=…) aren't used here.
                    while parser.symbol(",") {
                        parser.next()?;
                        parser.name()?;
                        parser.expect("=")?;
                        parser.expression()?;
                    }
                    parser.expect(")")?;
                    if gen.names.contains(&name) {
                        return Err(format!("{name} is declared twice"));
                    }
                    gen.names.push(name);
                    gen.kinds.push(kind);
                    gen.values.push(first);
                    if parser.symbol(",") {
                        parser.next()?;
                    } else {
                        break;
                    }
                }
                parser.expect(";")?;
            } else {
                statements.push(parser.statement()?);
            }
        }
        gen.body = statements.iter().map(|statement| gen.step(statement)).collect::<Result<_, _>>()?;
        Ok(gen)
    }

    fn name(&mut self, name: &str, assigned: bool) -> Result<Name, String> {
        match name {
            "samplerate" => return Ok(Name::Constant(self.samplerate)),
            "vectorsize" => return Ok(Name::Constant(self.vectorsize)),
            "pi" => return Ok(Name::Constant(std::f64::consts::PI)),
            "twopi" => return Ok(Name::Constant(std::f64::consts::TAU)),
            _ => {}
        }
        let numbered = |prefix: &str| name.strip_prefix(prefix).and_then(|n| n.parse::<usize>().ok()).filter(|n| *n > 0);
        if let Some(index) = numbered("in").filter(|_| !assigned) {
            return Ok(Name::Input(index - 1));
        }
        if let Some(index) = numbered("out").filter(|_| assigned) {
            self.outputs = self.outputs.max(index);
            return Ok(Name::Output(index - 1));
        }
        match self.names.iter().position(|known| known == name) {
            Some(at) if assigned && self.kinds[at] == Kind::Param => Err(format!("the code assigns the Param {name}")),
            Some(at) => Ok(Name::Slot(at)),
            None => {
                self.names.push(name.to_string());
                self.kinds.push(Kind::Local);
                self.values.push(0.0);
                Ok(Name::Slot(self.names.len() - 1))
            }
        }
    }

    fn step(&mut self, statement: &Statement) -> Result<Step, String> {
        Ok(match statement {
            Statement::Assign(target, operator, value) => {
                let value = self.op(value)?;
                let code = match operator.as_str() {
                    "=" => 0,
                    "+=" => 1,
                    "-=" => 2,
                    "*=" => 3,
                    _ => 4,
                };
                let target = self.name(target, true)?;
                if matches!(target, Name::Input(_) | Name::Constant(_)) {
                    return Err("the code assigns an inlet or a constant".into());
                }
                Step::Assign(target, code, value)
            }
            Statement::If(test, then, otherwise) => {
                let test = self.op(test)?;
                let then = then.iter().map(|statement| self.step(statement)).collect::<Result<_, _>>()?;
                let otherwise = otherwise.iter().map(|statement| self.step(statement)).collect::<Result<_, _>>()?;
                Step::If(test, then, otherwise)
            }
            Statement::Poke(buffer, value, index) => {
                let at = self.buffers.iter().position(|(name, _)| name == buffer).ok_or_else(|| format!("no Buffer {buffer}"))?;
                Step::Poke(at, self.op(value)?, self.op(index)?)
            }
        })
    }

    fn op(&mut self, expr: &Expr) -> Result<Op, String> {
        Ok(match expr {
            Expr::Number(number) => Op::Number(*number),
            Expr::Name(name) => Op::Read(self.name(name, false)?),
            Expr::Not(inner) => Op::Not(Box::new(self.op(inner)?)),
            Expr::Negate(inner) => Op::Negate(Box::new(self.op(inner)?)),
            Expr::Choose(test, yes, no) => Op::Choose(Box::new(self.op(test)?), Box::new(self.op(yes)?), Box::new(self.op(no)?)),
            Expr::Binary(operator, left, right) => {
                const OPERATORS: [&str; 13] = ["+", "-", "*", "/", "%", "<", "<=", ">", ">=", "==", "!=", "&&", "||"];
                let code = OPERATORS.iter().position(|known| known == operator).ok_or_else(|| format!("no operator {operator}"))? as u8;
                Op::Binary(code, Box::new(self.op(left)?), Box::new(self.op(right)?))
            }
            Expr::Call(name, arguments) if name == "dcblock" && arguments.len() == 1 => {
                self.dcblocks.push(DcBlock::default());
                Op::DcBlock(self.dcblocks.len() - 1, Box::new(self.op(&arguments[0])?))
            }
            Expr::Call(name, arguments) => {
                let function = Function::of(name, arguments.len())
                    .ok_or_else(|| format!("{name} with {} arguments isn't an operator Kumi's interpreter runs", arguments.len()))?;
                Op::Call(function, arguments.iter().map(|argument| self.op(argument)).collect::<Result<_, _>>()?)
            }
        })
    }

    /// A message "name value" to the gen~: sets its Param.
    pub fn set(&mut self, name: &str, value: f64) -> Result<(), String> {
        let at = self.names.iter().position(|known| known == name).filter(|at| self.kinds[*at] == Kind::Param);
        let at = at.ok_or_else(|| format!("no Param {name}"))?;
        self.values[at] = value;
        Ok(())
    }

    /// One sample: the inlets' values in, the outlets' values out.
    pub fn tick(&mut self, inputs: &[f64], outputs: usize) -> Result<Vec<f64>, String> {
        if outputs < self.outputs {
            return Err(format!("the code sends {} outlets", self.outputs));
        }
        let mut run = Run { inputs, outputs: vec![0.0; outputs], set: vec![false; self.names.len()], pending: Vec::new() };
        let body = std::mem::take(&mut self.body);
        let ran = self.steps(&body, &mut run);
        self.body = body;
        ran?;
        for (at, value) in run.pending {
            self.values[at] = value;
        }
        Ok(run.outputs)
    }

    fn steps(&mut self, body: &[Step], run: &mut Run) -> Result<(), String> {
        for step in body {
            match step {
                Step::If(test, then, otherwise) => {
                    if self.eval(test, run)? != 0.0 {
                        self.steps(then, run)?;
                    } else {
                        self.steps(otherwise, run)?;
                    }
                }
                Step::Poke(at, value, index) => {
                    let (value, index) = (self.eval(value, run)?, self.eval(index, run)?.floor());
                    let buffer = &mut self.buffers[*at].1;
                    if !(index >= 0.0 && (index as usize) < buffer.len()) {
                        return Err(format!("poke past the buffer: {index}"));
                    }
                    buffer[index as usize] = value;
                }
                Step::Assign(target, operator, value) => {
                    let value = self.eval(value, run)?;
                    let value = match operator {
                        0 => value,
                        _ => {
                            let was = self.read(*target, run)?;
                            match operator {
                                1 => was + value,
                                2 => was - value,
                                3 => was * value,
                                _ => was / value,
                            }
                        }
                    };
                    if !value.is_finite() {
                        return Err(format!("a value isn't finite: {value}"));
                    }
                    match *target {
                        Name::Output(index) => run.outputs[index] = value,
                        Name::Slot(at) if self.kinds[at] == Kind::History && self.delayed => run.pending.push((at, value)),
                        Name::Slot(at) => {
                            self.values[at] = value;
                            run.set[at] = true;
                        }
                        // Refused when the code was read.
                        Name::Input(_) | Name::Constant(_) => {}
                    }
                }
            }
        }
        Ok(())
    }

    fn read(&self, name: Name, run: &Run) -> Result<f64, String> {
        match name {
            Name::Constant(value) => Ok(value),
            Name::Input(index) => run.inputs.get(index).copied().ok_or_else(|| format!("in{} has no value", index + 1)),
            Name::Output(index) => run.outputs.get(index).copied().ok_or_else(|| format!("out{} isn't sent", index + 1)),
            Name::Slot(at) if self.kinds[at] == Kind::Local && !run.set[at] => Err(format!("{} is read before it's set", self.names[at])),
            Name::Slot(at) => Ok(self.values[at]),
        }
    }

    fn eval(&mut self, op: &Op, run: &Run) -> Result<f64, String> {
        let truth = |value: bool| if value { 1.0 } else { 0.0 };
        Ok(match op {
            Op::Number(number) => *number,
            Op::Read(name) => self.read(*name, run)?,
            Op::Negate(inner) => -self.eval(inner, run)?,
            Op::Not(inner) => truth(self.eval(inner, run)? == 0.0),
            Op::Choose(test, yes, no) => {
                if self.eval(test, run)? != 0.0 {
                    self.eval(yes, run)?
                } else {
                    self.eval(no, run)?
                }
            }
            Op::Binary(operator, left, right) => {
                let (a, b) = (self.eval(left, run)?, self.eval(right, run)?);
                match operator {
                    0 => a + b,
                    1 => a - b,
                    2 => a * b,
                    3 if b == 0.0 => return Err("a division by 0".into()),
                    3 => a / b,
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
            Op::DcBlock(at, inner) => {
                // genlib's dcblock: y = x - x1 + 0.9997·y1.
                let x = self.eval(inner, run)?;
                let state = &mut self.dcblocks[*at];
                let y = x - state.x1 + state.y1 * 0.9997;
                *state = DcBlock { x1: x, y1: y };
                y
            }
            Op::Call(function, arguments) => {
                let mut values = [0.0; 3];
                for (value, argument) in values.iter_mut().zip(arguments) {
                    *value = self.eval(argument, run)?;
                }
                let [a, b, c] = values;
                match function {
                    Function::Floor => a.floor(),
                    Function::Ceil => a.ceil(),
                    Function::Abs => a.abs(),
                    // genlib's: a NaN passes through them, as it does in gen~.
                    Function::Min => minimum(a, b),
                    Function::Max => maximum(a, b),
                    Function::Clamp => minimum(maximum(a, b), c),
                    Function::Mix => a + c * (b - a),
                    Function::DbToA => 10f64.powf(a * 0.05),
                    Function::FixNan => {
                        if a.is_nan() {
                            0.0
                        } else {
                            a
                        }
                    }
                    Function::FixDenorm => {
                        if a.is_subnormal() {
                            0.0
                        } else {
                            a
                        }
                    }
                    Function::IsNan => truth(a.is_nan()),
                    Function::IsDenorm => truth(a.is_subnormal()),
                }
            }
        })
    }
}

/// genlib's `minimum`: `y < x ? y : x`.
fn minimum(x: f64, y: f64) -> f64 {
    if y < x {
        y
    } else {
        x
    }
}

/// genlib's `maximum`: `x < y ? y : x`.
fn maximum(x: f64, y: f64) -> f64 {
    if x < y {
        y
    } else {
        x
    }
}

fn eval_constant(expr: &Expr) -> Result<f64, String> {
    match expr {
        Expr::Number(number) => Ok(*number),
        Expr::Negate(inner) => Ok(-eval_constant(inner)?),
        other => Err(format!("a declaration's value is a number, not {other:?}")),
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
        let mut gen = Gen::new(code, false).unwrap();
        assert_eq!(gen.tick(&[1.0], 3).unwrap(), vec![0.0, -15.0, -1.0 + 1.0 + 2.0 + 1.0 + 2.0 + 5.0]);
        assert_eq!(gen.tick(&[2.0], 3).unwrap(), vec![1.0, -14.0, 10.0], "5 % 4 is 1; the History kept its value");
        gen.set("rate", 0.0).unwrap();
        assert_eq!(gen.tick(&[2.0], 3).unwrap()[0], 1.0);
    }

    #[test]
    fn poke_writes_a_buffer_that_reads_back_at_once() {
        let mut gen = Gen::new("Buffer memory;\nHistory n(0);\nn = n + 1;\npoke(memory, n * 10, 1, 0);\nout1 = n;\n", false).unwrap();
        gen.tick(&[], 1).unwrap();
        gen.tick(&[], 1).unwrap();
        assert_eq!(gen.buffers[0].1[1], 20.0);
    }

    #[test]
    fn a_delayed_history_lands_at_the_next_sample() {
        let code = "History h(0);\nh = h + 1;\nout1 = h;\n";
        assert_eq!(Gen::new(code, false).unwrap().tick(&[], 1).unwrap(), vec![1.0]);
        let mut delayed = Gen::new(code, true).unwrap();
        assert_eq!(delayed.tick(&[], 1).unwrap(), vec![0.0]);
        assert_eq!(delayed.tick(&[], 1).unwrap(), vec![1.0]);
    }

    #[test]
    fn the_output_stages_operators_run_as_gen_runs_them() {
        let code = "out1 = fixnan(in1) + fixdenorm(in2) + isnan(in1) + isdenorm(in2);\nout2 = mix(2, 4, 0.25) + dbtoa(0);\nout3 = dcblock(in3);\nout4 = dbtoa(-20);\n";
        let mut gen = Gen::new(code, false).unwrap();
        let out = gen.tick(&[f64::NAN, f64::MIN_POSITIVE / 4.0, 1.0], 4).unwrap();
        assert_eq!(out[..2], [2.0, 3.5]);
        assert!((out[3] - 0.1).abs() < 1e-15, "{}", out[3]);
        assert_eq!(out[2], 1.0, "a step passes at once");
        assert_eq!(gen.tick(&[0.0, 0.0, 1.0], 4).unwrap()[2], 0.9997, "then decays");
    }

    #[test]
    fn code_it_cant_run_is_an_error_never_a_panic() {
        for (code, why) in [
            ("out1 = later;\nlater = 1;\n", "read before it's set"),
            ("out1 = cycle(440);\n", "isn't an operator"),
            ("out1 = in1 / 0;\n", "division by 0"),
            ("out1 = in1 +;\n", "unexpected"),
            ("out1 = (in1;\n", "expected )"),
            ("out1 = 1", "ends too soon"),
            ("Param p(1);\np = 2;\nout1 = p;\n", "assigns the Param"),
            ("pi = 2;\nout1 = pi;\n", "assigns an inlet or a constant"),
            ("x = 1e999;\nout1 = x;\n", "isn't finite"),
            ("Buffer \"name\";\n", "expected a name"),
            ("out1 = \"text;\n", "never ends"),
        ] {
            let ran = Gen::new(code, false).and_then(|mut gen| gen.tick(&[1.0], 1));
            assert!(ran.as_ref().is_err_and(|error| error.contains(why)), "{code:?}: {ran:?}");
        }
        assert!(Gen::new("Param p(1);\nout1 = p;\n", false).unwrap().set("q", 1.0).is_err());
        assert!(Gen::new("out2 = 1;\n", false).unwrap().tick(&[], 1).is_err(), "fewer outlets than the code sends");
    }
}
