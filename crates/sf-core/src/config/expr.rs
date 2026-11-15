//! Expression language used by `Eval:`, `If:` and inline backtick expressions.
//!
//! The language is intentionally small but supports everything documented by
//! the configuration reference: numbers, strings, booleans, arrays, variables,
//! arithmetic/comparison/bitwise operators, a ternary operator, assignment and
//! the documented function set. Semicolons separate expressions and the result
//! of the last one is returned.

use std::collections::HashMap;

/// A value produced by an expression.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Float(f64),
    Int(i64),
    Bool(bool),
    Str(String),
    Array(Vec<Value>),
}

impl Value {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Value::Str(s) => s.trim().parse::<f64>().ok(),
            Value::Array(_) => None,
        }
    }

    pub fn as_bool(&self) -> bool {
        match self {
            Value::Float(f) => *f != 0.0,
            Value::Int(i) => *i != 0,
            Value::Bool(b) => *b,
            Value::Str(s) => !s.is_empty(),
            Value::Array(a) => !a.is_empty(),
        }
    }

    pub fn type_char(&self) -> char {
        match self {
            Value::Float(_) => 'f',
            Value::Int(_) => 'i',
            Value::Bool(_) => 'b',
            Value::Str(_) => 's',
            Value::Array(_) => 'm',
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Float(v) => {
                // Trim trailing zeros so that `1000.0` renders as `1000`.
                if v.fract() == 0.0 && v.abs() < 1e15 {
                    write!(f, "{}", *v as i64)
                } else {
                    write!(f, "{v}")
                }
            }
            Value::Int(v) => write!(f, "{v}"),
            Value::Bool(v) => write!(f, "{v}"),
            Value::Str(v) => write!(f, "{v}"),
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ",")?;
                    }
                    write!(f, "{item}")?;
                }
                Ok(())
            }
        }
    }
}

/// Evaluation failure.
#[derive(Clone, Debug, PartialEq)]
pub struct EvalError(pub String);

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub type EvalResult<T> = Result<T, EvalError>;

fn err<T>(msg: impl Into<String>) -> EvalResult<T> {
    Err(EvalError(msg.into()))
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Number(f64, bool),
    Str(String),
    Ident(String),
    Op(&'static str),
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Question,
    Colon,
    Semi,
    End,
}

fn lex(src: &str) -> EvalResult<Vec<Token>> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0usize;
    let mut out = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '"' => {
                i += 1;
                let mut s = String::new();
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                        s.push(chars[i]);
                    } else {
                        s.push(chars[i]);
                    }
                    i += 1;
                }
                if i >= chars.len() {
                    return err("unterminated string literal");
                }
                i += 1;
                out.push(Token::Str(s));
            }
            '0'..='9' | '.' => {
                let start = i;
                let mut is_float = false;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    if chars[i] == '.' {
                        is_float = true;
                    }
                    i += 1;
                }
                // Scientific notation.
                if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                    let save = i;
                    i += 1;
                    if i < chars.len() && (chars[i] == '+' || chars[i] == '-') {
                        i += 1;
                    }
                    if i < chars.len() && chars[i].is_ascii_digit() {
                        is_float = true;
                        while i < chars.len() && chars[i].is_ascii_digit() {
                            i += 1;
                        }
                    } else {
                        i = save;
                    }
                }
                let text: String = chars[start..i].iter().collect();
                let v: f64 = text
                    .parse()
                    .map_err(|_| EvalError(format!("bad number '{text}'")))?;
                out.push(Token::Number(v, is_float));
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                out.push(Token::Ident(chars[start..i].iter().collect()));
            }
            '(' => {
                out.push(Token::LParen);
                i += 1;
            }
            ')' => {
                out.push(Token::RParen);
                i += 1;
            }
            '{' => {
                out.push(Token::LBrace);
                i += 1;
            }
            '}' => {
                out.push(Token::RBrace);
                i += 1;
            }
            '[' => {
                out.push(Token::LBracket);
                i += 1;
            }
            ']' => {
                out.push(Token::RBracket);
                i += 1;
            }
            ',' => {
                out.push(Token::Comma);
                i += 1;
            }
            '?' => {
                out.push(Token::Question);
                i += 1;
            }
            ':' => {
                out.push(Token::Colon);
                i += 1;
            }
            ';' => {
                out.push(Token::Semi);
                i += 1;
            }
            _ => {
                let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
                let op: Option<&'static str> = match two.as_str() {
                    "==" => Some("=="),
                    "!=" => Some("!="),
                    "<=" => Some("<="),
                    ">=" => Some(">="),
                    "&&" => Some("and"),
                    "||" => Some("or"),
                    "+=" => Some("+="),
                    "-=" => Some("-="),
                    "*=" => Some("*="),
                    "/=" => Some("/="),
                    "<<" => Some("<<"),
                    ">>" => Some(">>"),
                    _ => None,
                };
                if let Some(op) = op {
                    out.push(Token::Op(op));
                    i += 2;
                    continue;
                }
                let one = match c {
                    '+' => Some("+"),
                    '-' => Some("-"),
                    '*' => Some("*"),
                    '/' => Some("/"),
                    '^' => Some("^"),
                    '<' => Some("<"),
                    '>' => Some(">"),
                    '=' => Some("="),
                    '&' => Some("&"),
                    '|' => Some("|"),
                    '!' => Some("!"),
                    _ => None,
                };
                match one {
                    Some(op) => {
                        out.push(Token::Op(op));
                        i += 1;
                    }
                    None => return err(format!("unexpected character '{c}'")),
                }
            }
        }
    }
    out.push(Token::End);
    Ok(out)
}

struct Parser<'a> {
    tokens: Vec<Token>,
    pos: usize,
    env: &'a mut Env,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn next(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        t
    }

    fn eat_op(&mut self, op: &str) -> bool {
        if let Token::Op(o) = self.peek() {
            if *o == op {
                self.next();
                return true;
            }
        }
        false
    }

    fn parse_sequence(&mut self) -> EvalResult<Value> {
        loop {
            let value = self.parse_assignment()?;
            if matches!(self.peek(), Token::Semi) {
                self.next();
                if matches!(self.peek(), Token::End) {
                    return Ok(value);
                }
                continue;
            }
            return Ok(value);
        }
    }

    fn parse_assignment(&mut self) -> EvalResult<Value> {
        // Look ahead for `ident op= ...`.
        if let Token::Ident(name) = self.peek().clone() {
            let save = self.pos;
            self.next();
            let assign = match self.peek() {
                Token::Op("=") => Some("="),
                Token::Op("+=") => Some("+="),
                Token::Op("-=") => Some("-="),
                Token::Op("*=") => Some("*="),
                Token::Op("/=") => Some("/="),
                _ => None,
            };
            if let Some(op) = assign {
                self.next();
                let rhs = self.parse_assignment()?;
                let value = if op == "=" {
                    rhs
                } else {
                    let current = self.env.get(&name).unwrap_or(Value::Float(0.0));
                    let a = current
                        .as_f64()
                        .ok_or_else(|| EvalError(format!("'{name}' is not numeric")))?;
                    let b = rhs
                        .as_f64()
                        .ok_or_else(|| EvalError("right hand side is not numeric".into()))?;
                    let result = match op {
                        "+=" => a + b,
                        "-=" => a - b,
                        "*=" => a * b,
                        "/=" => a / b,
                        _ => unreachable!(),
                    };
                    Value::Float(result)
                };
                self.env.set(&name, value.clone());
                return Ok(value);
            }
            self.pos = save;
        }
        self.parse_ternary()
    }

    fn parse_ternary(&mut self) -> EvalResult<Value> {
        let cond = self.parse_or()?;
        if matches!(self.peek(), Token::Question) {
            self.next();
            let then = self.parse_assignment()?;
            if matches!(self.peek(), Token::Colon) {
                self.next();
            } else {
                return err("expected ':' in conditional expression");
            }
            let otherwise = self.parse_ternary()?;
            return Ok(if cond.as_bool() { then } else { otherwise });
        }
        Ok(cond)
    }

    fn parse_or(&mut self) -> EvalResult<Value> {
        let mut left = self.parse_and()?;
        loop {
            if matches!(self.peek(), Token::Op("or")) {
                self.next();
                let right = self.parse_and()?;
                left = Value::Bool(left.as_bool() || right.as_bool());
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_and(&mut self) -> EvalResult<Value> {
        let mut left = self.parse_xor()?;
        loop {
            if matches!(self.peek(), Token::Op("and")) {
                self.next();
                let right = self.parse_xor()?;
                left = Value::Bool(left.as_bool() && right.as_bool());
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_xor(&mut self) -> EvalResult<Value> {
        let mut left = self.parse_bitor()?;
        loop {
            if matches!(self.peek(), Token::Op("xor")) {
                self.next();
                let right = self.parse_bitor()?;
                left = Value::Bool(left.as_bool() ^ right.as_bool());
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_bitor(&mut self) -> EvalResult<Value> {
        let mut left = self.parse_bitand()?;
        while self.eat_op("|") {
            let right = self.parse_bitand()?;
            left = Value::Int(to_int(&left)? | to_int(&right)?);
        }
        Ok(left)
    }

    fn parse_bitand(&mut self) -> EvalResult<Value> {
        let mut left = self.parse_shift()?;
        while self.eat_op("&") {
            let right = self.parse_shift()?;
            left = Value::Int(to_int(&left)? & to_int(&right)?);
        }
        Ok(left)
    }

    fn parse_shift(&mut self) -> EvalResult<Value> {
        let mut left = self.parse_comparison()?;
        loop {
            if self.eat_op("<<") {
                let right = self.parse_comparison()?;
                left = Value::Int(to_int(&left)? << to_int(&right)?);
            } else if self.eat_op(">>") {
                let right = self.parse_comparison()?;
                left = Value::Int(to_int(&left)? >> to_int(&right)?);
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_comparison(&mut self) -> EvalResult<Value> {
        let left = self.parse_additive()?;
        let op = match self.peek() {
            Token::Op(o) => *o,
            _ => return Ok(left),
        };
        let applies = matches!(op, "==" | "!=" | "<" | ">" | "<=" | ">=");
        if !applies {
            return Ok(left);
        }
        self.next();
        let right = self.parse_additive()?;
        let result = match op {
            "==" => values_equal(&left, &right),
            "!=" => !values_equal(&left, &right),
            "<" => compare(&left, &right)? < 0,
            ">" => compare(&left, &right)? > 0,
            "<=" => compare(&left, &right)? <= 0,
            ">=" => compare(&left, &right)? >= 0,
            _ => false,
        };
        Ok(Value::Bool(result))
    }

    fn parse_additive(&mut self) -> EvalResult<Value> {
        let mut left = self.parse_multiplicative()?;
        loop {
            if self.eat_op("+") {
                let right = self.parse_multiplicative()?;
                left = match (&left, &right) {
                    (Value::Str(a), b) => Value::Str(format!("{a}{b}")),
                    (a, Value::Str(b)) => Value::Str(format!("{a}{b}")),
                    _ => Value::Float(as_float(&left)? + as_float(&right)?),
                };
            } else if self.eat_op("-") {
                let right = self.parse_multiplicative()?;
                left = Value::Float(as_float(&left)? - as_float(&right)?);
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_multiplicative(&mut self) -> EvalResult<Value> {
        let mut left = self.parse_unary()?;
        loop {
            if self.eat_op("*") {
                let right = self.parse_unary()?;
                left = Value::Float(as_float(&left)? * as_float(&right)?);
            } else if self.eat_op("/") {
                let right = self.parse_unary()?;
                let divisor = as_float(&right)?;
                if divisor == 0.0 {
                    return err("division by zero");
                }
                left = Value::Float(as_float(&left)? / divisor);
            } else {
                return Ok(left);
            }
        }
    }

    fn parse_unary(&mut self) -> EvalResult<Value> {
        if self.eat_op("-") {
            let v = self.parse_unary()?;
            return Ok(Value::Float(-as_float(&v)?));
        }
        if self.eat_op("+") {
            return self.parse_unary();
        }
        if self.eat_op("!") {
            let v = self.parse_unary()?;
            return Ok(Value::Bool(!v.as_bool()));
        }
        self.parse_power()
    }

    fn parse_power(&mut self) -> EvalResult<Value> {
        let base = self.parse_postfix()?;
        if self.eat_op("^") {
            let exponent = self.parse_unary()?;
            return Ok(Value::Float(as_float(&base)?.powf(as_float(&exponent)?)));
        }
        Ok(base)
    }

    fn parse_postfix(&mut self) -> EvalResult<Value> {
        let mut value = self.parse_primary()?;
        while matches!(self.peek(), Token::LBracket) {
            self.next();
            let idx = self.parse_assignment()?;
            if matches!(self.peek(), Token::RBracket) {
                self.next();
            } else {
                return err("expected ']'");
            }
            let i = to_int(&idx)?;
            value = match value {
                Value::Array(items) => items
                    .get(i.max(0) as usize)
                    .cloned()
                    .ok_or_else(|| EvalError(format!("array index {i} out of bounds")))?,
                Value::Str(s) => s
                    .chars()
                    .nth(i.max(0) as usize)
                    .map(|c| Value::Str(c.to_string()))
                    .ok_or_else(|| EvalError(format!("string index {i} out of bounds")))?,
                other => return err(format!("cannot index value of type {}", other.type_char())),
            };
        }
        Ok(value)
    }

    fn parse_primary(&mut self) -> EvalResult<Value> {
        match self.next() {
            Token::Number(v, is_float) => {
                if is_float {
                    Ok(Value::Float(v))
                } else if v.abs() < i64::MAX as f64 {
                    Ok(Value::Int(v as i64))
                } else {
                    Ok(Value::Float(v))
                }
            }
            Token::Str(s) => Ok(Value::Str(s)),
            Token::LParen => {
                let v = self.parse_sequence()?;
                if matches!(self.peek(), Token::RParen) {
                    self.next();
                }
                Ok(v)
            }
            Token::LBrace => {
                let mut items = Vec::new();
                if !matches!(self.peek(), Token::RBrace) {
                    loop {
                        items.push(self.parse_assignment()?);
                        if matches!(self.peek(), Token::Comma) {
                            self.next();
                            continue;
                        }
                        break;
                    }
                }
                if matches!(self.peek(), Token::RBrace) {
                    self.next();
                }
                Ok(Value::Array(items))
            }
            Token::Ident(name) => {
                if matches!(self.peek(), Token::LParen) {
                    self.next();
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Token::RParen) {
                        loop {
                            args.push(self.parse_assignment()?);
                            if matches!(self.peek(), Token::Comma) {
                                self.next();
                                continue;
                            }
                            break;
                        }
                    }
                    if matches!(self.peek(), Token::RParen) {
                        self.next();
                    } else {
                        return err("expected ')' after arguments");
                    }
                    self.env.call(&name, &args)
                } else if name == "true" {
                    Ok(Value::Bool(true))
                } else if name == "false" {
                    Ok(Value::Bool(false))
                } else if name == "pi" {
                    Ok(Value::Float(std::f64::consts::PI))
                } else if name == "e" {
                    Ok(Value::Float(std::f64::consts::E))
                } else {
                    self.env
                        .get(&name)
                        .ok_or_else(|| EvalError(format!("unknown variable '{name}'")))
                }
            }
            other => err(format!("unexpected token {other:?}")),
        }
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| values_equal(a, b))
        }
        _ => match (a.as_f64(), b.as_f64()) {
            (Some(x), Some(y)) => (x - y).abs() < 1e-12,
            _ => false,
        },
    }
}

fn compare(a: &Value, b: &Value) -> EvalResult<i32> {
    if let (Value::Str(x), Value::Str(y)) = (a, b) {
        return Ok(match x.cmp(y) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        });
    }
    let x = as_float(a)?;
    let y = as_float(b)?;
    Ok(if x < y {
        -1
    } else if x > y {
        1
    } else {
        0
    })
}

fn as_float(v: &Value) -> EvalResult<f64> {
    v.as_f64()
        .ok_or_else(|| EvalError(format!("expected a number, got type '{}'", v.type_char())))
}

fn to_int(v: &Value) -> EvalResult<i64> {
    match v {
        Value::Int(i) => Ok(*i),
        Value::Float(f) => Ok(*f as i64),
        Value::Bool(b) => Ok(if *b { 1 } else { 0 }),
        other => err(format!(
            "expected an integer, got type '{}'",
            other.type_char()
        )),
    }
}

/// Execution environment: variables plus the built-in constants of the audio
/// stream currently being compiled.
#[derive(Clone, Debug, Default)]
pub struct Env {
    vars: HashMap<String, Value>,
}

impl Env {
    pub fn new() -> Self {
        Self {
            vars: HashMap::new(),
        }
    }

    /// Creates an environment describing the stream being compiled.
    pub fn for_stream(
        sample_rate: u32,
        input_channels: usize,
        output_channels: usize,
        device_name: &str,
        connection_name: &str,
        device_guid: &str,
        stage: &str,
    ) -> Self {
        let mut env = Self::new();
        env.vars
            .insert("sampleRate".into(), Value::Int(sample_rate as i64));
        env.vars.insert(
            "inputChannelCount".into(),
            Value::Int(input_channels as i64),
        );
        env.vars.insert(
            "outputChannelCount".into(),
            Value::Int(output_channels as i64),
        );
        env.vars
            .insert("deviceName".into(), Value::Str(device_name.into()));
        env.vars
            .insert("connectionName".into(), Value::Str(connection_name.into()));
        env.vars
            .insert("deviceGuid".into(), Value::Str(device_guid.into()));
        env.vars.insert("stage".into(), Value::Str(stage.into()));
        env
    }

    pub fn set(&mut self, name: &str, value: Value) {
        self.vars.insert(name.to_string(), value);
    }

    pub fn get(&self, name: &str) -> Option<Value> {
        self.vars.get(name).cloned()
    }

    /// Evaluates `src`, mutating variables as needed.
    pub fn eval(&mut self, src: &str) -> EvalResult<Value> {
        let tokens = lex(src)?;
        let mut parser = Parser {
            tokens,
            pos: 0,
            env: self,
        };
        parser.parse_sequence()
    }

    fn call(&mut self, name: &str, args: &[Value]) -> EvalResult<Value> {
        let need = |n: usize| -> EvalResult<()> {
            if args.len() < n {
                Err(EvalError(format!("{name} expects {n} argument(s)")))
            } else {
                Ok(())
            }
        };
        let f = |i: usize| -> EvalResult<f64> { as_float(&args[i]) };
        match name {
            "abs" => {
                need(1)?;
                Ok(Value::Float(f(0)?.abs()))
            }
            "sin" => {
                need(1)?;
                Ok(Value::Float(f(0)?.sin()))
            }
            "cos" => {
                need(1)?;
                Ok(Value::Float(f(0)?.cos()))
            }
            "tan" => {
                need(1)?;
                Ok(Value::Float(f(0)?.tan()))
            }
            "sinh" => {
                need(1)?;
                Ok(Value::Float(f(0)?.sinh()))
            }
            "cosh" => {
                need(1)?;
                Ok(Value::Float(f(0)?.cosh()))
            }
            "tanh" => {
                need(1)?;
                Ok(Value::Float(f(0)?.tanh()))
            }
            "ln" => {
                need(1)?;
                Ok(Value::Float(f(0)?.ln()))
            }
            "log" => {
                need(1)?;
                Ok(Value::Float(f(0)?.ln()))
            }
            "log10" => {
                need(1)?;
                Ok(Value::Float(f(0)?.log10()))
            }
            "exp" => {
                need(1)?;
                Ok(Value::Float(f(0)?.exp()))
            }
            "sqrt" => {
                need(1)?;
                Ok(Value::Float(f(0)?.sqrt()))
            }
            "min" => {
                need(1)?;
                let mut m = f(0)?;
                for i in 1..args.len() {
                    m = m.min(f(i)?);
                }
                Ok(Value::Float(m))
            }
            "max" => {
                need(1)?;
                let mut m = f(0)?;
                for i in 1..args.len() {
                    m = m.max(f(i)?);
                }
                Ok(Value::Float(m))
            }
            "sum" => {
                let mut total = 0.0;
                for (i, a) in args.iter().enumerate() {
                    if let Value::Array(items) = a {
                        for item in items {
                            total += as_float(item)?;
                        }
                    } else {
                        total += f(i)?;
                    }
                }
                Ok(Value::Float(total))
            }
            "str2dbl" => {
                need(1)?;
                let text = match &args[0] {
                    Value::Str(s) => s.clone(),
                    other => other.to_string(),
                };
                let cleaned = text.trim().replace(',', ".");
                cleaned
                    .parse::<f64>()
                    .map(Value::Float)
                    .map_err(|_| EvalError(format!("'{text}' is not a number")))
            }
            "strlen" => {
                need(1)?;
                let len = match &args[0] {
                    Value::Str(s) => s.chars().count(),
                    other => other.to_string().chars().count(),
                };
                Ok(Value::Int(len as i64))
            }
            "tolower" => {
                need(1)?;
                Ok(Value::Str(args[0].to_string().to_lowercase()))
            }
            "toupper" => {
                need(1)?;
                Ok(Value::Str(args[0].to_string().to_uppercase()))
            }
            "sizeof" => {
                need(1)?;
                let n = match &args[0] {
                    Value::Array(items) => items.len(),
                    Value::Str(s) => s.chars().count(),
                    _ => 0,
                };
                Ok(Value::Int(n as i64))
            }
            "regexSearch" => {
                need(2)?;
                let pattern = args[0].to_string();
                let haystack = args[1].to_string();
                let re = regex::Regex::new(&pattern)
                    .map_err(|e| EvalError(format!("invalid regular expression: {e}")))?;
                match re.captures(&haystack) {
                    Some(caps) => {
                        let mut items = Vec::new();
                        for i in 0..caps.len() {
                            items.push(Value::Str(
                                caps.get(i).map(|m| m.as_str()).unwrap_or("").to_string(),
                            ));
                        }
                        Ok(Value::Array(items))
                    }
                    None => Ok(Value::Array(Vec::new())),
                }
            }
            "regexReplace" => {
                need(3)?;
                let pattern = args[0].to_string();
                let haystack = args[1].to_string();
                let replacement = args[2].to_string();
                let re = regex::Regex::new(&pattern)
                    .map_err(|e| EvalError(format!("invalid regular expression: {e}")))?;
                Ok(Value::Str(
                    re.replace_all(&haystack, replacement.as_str()).to_string(),
                ))
            }
            "readRegString" | "readRegDWORD" => {
                need(2)?;
                read_registry(
                    &args[0].to_string(),
                    &args[1].to_string(),
                    name == "readRegDWORD",
                )
            }
            other => err(format!("unknown function '{other}'")),
        }
    }
}

#[cfg(windows)]
fn read_registry(key: &str, value: &str, as_dword: bool) -> EvalResult<Value> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
        REG_DWORD, REG_SZ,
    };

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    // Accept both `HKLM\...` and `HKEY_LOCAL_MACHINE\...`.
    let normalized = key
        .replace("HKEY_LOCAL_MACHINE", "HKLM")
        .replace("HKEY_CURRENT_USER", "HKCU");
    let (root, rest) = if let Some(rest) = normalized.strip_prefix("HKLM\\") {
        (HKEY_LOCAL_MACHINE, rest)
    } else if normalized.starts_with("HKCU\\") {
        return Err(EvalError(
            "readRegString/readRegDWORD currently support HKLM only".into(),
        ));
    } else {
        return Err(EvalError(format!("unsupported registry root in '{key}'")));
    };
    let subkey = wide(rest);
    let mut handle: HKEY = std::ptr::null_mut();
    let status = unsafe { RegOpenKeyExW(root, subkey.as_ptr(), 0, KEY_READ, &mut handle) };
    if status != 0 {
        return Err(EvalError(format!(
            "cannot open registry key '{key}' (error {status})"
        )));
    }
    let name = wide(value);
    let mut ty: u32 = 0;
    let mut size: u32 = 0;
    let status = unsafe {
        RegQueryValueExW(
            handle,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut ty,
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if status != 0 {
        unsafe { RegCloseKey(handle) };
        return Ok(if as_dword {
            Value::Int(0)
        } else {
            Value::Str(String::new())
        });
    }
    let mut buffer = vec![0u8; size as usize + 2];
    let mut size_mut = size;
    let status = unsafe {
        RegQueryValueExW(
            handle,
            name.as_ptr(),
            std::ptr::null_mut(),
            &mut ty,
            buffer.as_mut_ptr(),
            &mut size_mut,
        )
    };
    unsafe { RegCloseKey(handle) };
    if status != 0 {
        return Err(EvalError(format!(
            "cannot read registry value '{value}' (error {status})"
        )));
    }
    if ty == REG_DWORD {
        let bytes: [u8; 4] = buffer[..4].try_into().unwrap_or([0, 0, 0, 0]);
        return Ok(Value::Int(u32::from_le_bytes(bytes) as i64));
    }
    if ty == REG_SZ {
        let units: Vec<u16> = buffer
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&v| v != 0)
            .collect();
        return Ok(Value::Str(String::from_utf16_lossy(&units)));
    }
    let _ = REG_SZ;
    Err(EvalError(format!(
        "registry value '{value}' has an unsupported type {ty}"
    )))
}

#[cfg(not(windows))]
fn read_registry(_key: &str, _value: &str, _as_dword: bool) -> EvalResult<Value> {
    Err(EvalError(
        "registry access is only available on Windows".into(),
    ))
}

/// Expands inline backtick expressions inside a command parameter string.
pub fn expand_inline(params: &str, env: &mut Env) -> EvalResult<String> {
    let mut out = String::with_capacity(params.len());
    let mut chars = params.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '`' {
            let mut expr = String::new();
            let mut closed = false;
            for c in chars.by_ref() {
                if c == '`' {
                    closed = true;
                    break;
                }
                expr.push(c);
            }
            if !closed {
                return err("unterminated inline expression");
            }
            let value = env.eval(&expr)?;
            out.push_str(&value.to_string());
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(src: &str) -> Value {
        Env::new().eval(src).expect("expression should evaluate")
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(eval("1+2*3").as_f64(), Some(7.0));
        assert_eq!(eval("(1+2)*3").as_f64(), Some(9.0));
        assert_eq!(eval("2^3^2").as_f64(), Some(512.0));
        assert_eq!(eval("-4+10").as_f64(), Some(6.0));
    }

    #[test]
    fn documented_examples_evaluate() {
        assert!(eval("sin(0)^2 + cos(0)^2 == 1").as_bool());
    }

    #[test]
    fn variable_assignment_persists() {
        let mut env = Env::new();
        env.eval("a=0; b=pi").unwrap();
        assert_eq!(env.get("a").unwrap().as_f64(), Some(0.0));
        assert!((env.get("b").unwrap().as_f64().unwrap() - std::f64::consts::PI).abs() < 1e-12);
    }

    #[test]
    fn semicolon_sequence_returns_last_value() {
        let mut env = Env::new();
        assert_eq!(env.eval("1; 2; 3").unwrap().as_f64(), Some(3.0));
    }

    #[test]
    fn comparison_selects_the_larger_value() {
        let mut env = Env::new();
        env.eval("a=2; b=5").unwrap();
        assert_eq!(
            env.eval("a > b ? \"a\" : \"b\"").unwrap(),
            Value::Str("b".into())
        );
    }

    #[test]
    fn log10_of_half_is_about_minus_three_db() {
        let v = eval("20*log10(0.5)").as_f64().unwrap();
        assert!((v + 6.0206).abs() < 1e-4);
    }

    #[test]
    fn inline_expression_can_use_variables() {
        let mut env = Env::new();
        env.eval("linGain = 0.5").unwrap();
        let out = expand_inline(
            "ON PK Fc 1000 Hz Gain `20*log10(linGain)` dB Q 10.0",
            &mut env,
        )
        .unwrap();
        assert_eq!(out, "ON PK Fc 1000 Hz Gain -6.020599913279624 dB Q 10.0");
    }

    #[test]
    fn arrays_and_sizeof() {
        assert_eq!(eval("sizeof({1,2,3})").as_f64(), Some(3.0));
        assert_eq!(eval("{10,20,30}[1]").as_f64(), Some(20.0));
        assert_eq!(eval("sum({1,2,3.5})").as_f64(), Some(6.5));
    }

    #[test]
    fn regex_helpers() {
        let v =
            eval("regexSearch(\"High Definition .*\", \"Speakers High Definition Audio Device\")");
        assert_eq!(
            v,
            Value::Array(vec![Value::Str("High Definition Audio Device".into())])
        );
        let replaced = eval("regexReplace(\"o\", \"foo\", \"0\")");
        assert_eq!(replaced, Value::Str("f00".into()));
    }

    #[test]
    fn bitwise_operators() {
        assert_eq!(eval("6 & 3").as_f64(), Some(2.0));
        assert_eq!(eval("6 | 3").as_f64(), Some(7.0));
        assert_eq!(eval("1 << 4").as_f64(), Some(16.0));
        assert_eq!(eval("32 >> 3").as_f64(), Some(4.0));
    }

    #[test]
    fn string_concatenation() {
        assert_eq!(eval("\"a\"+\"b\""), Value::Str("ab".into()));
        assert_eq!(eval("\"gain \"+3"), Value::Str("gain 3".into()));
    }

    #[test]
    fn errors_are_reported_not_panicked() {
        assert!(Env::new().eval("1+").is_err());
        assert!(Env::new().eval("nope").is_err());
        assert!(Env::new().eval("1/0").is_err());
        assert!(Env::new().eval("`unterminated").is_err());
    }
}
