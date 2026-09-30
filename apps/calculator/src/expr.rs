//! A small expression evaluator: `+ - × ÷ ^ %`, brackets, functions and constants.
//!
//! Grammar, lowest precedence first:
//!
//! ```text
//! expr   = term (("+" | "-") term)*
//! term   = unary (("*" | "/") unary | implicit-multiplication)*
//! unary  = ("-" | "+") unary | power
//! power  = postfix ("^" unary)?
//! postfix= atom ("%" | "!")*
//! atom   = number | constant | function "(" expr ")" | "(" expr ")"
//! ```

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Op(char),
    Open,
    Close,
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' => i += 1,
            '0'..='9' | '.' => {
                // Commas between digits are thousands separators: "1,000" reads as 1000.
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.' || chars[i] == ',' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit()) {
                    i += 1;
                }
                // Scientific notation: 1.5e3, 2E-4.
                if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') && i + 1 < chars.len() && (chars[i + 1].is_ascii_digit() || (chars[i + 1] == '-' || chars[i + 1] == '+') && i + 2 < chars.len() && chars[i + 2].is_ascii_digit()) {
                    i += 2;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                let s: String = chars[start..i].iter().filter(|c| **c != ',').collect();
                out.push(Tok::Num(s.parse().map_err(|_| format!("“{s}” is not a number"))?));
            }
            'a'..='z' | 'A'..='Z' | 'π' | '√' => {
                let start = i;
                if c == 'π' || c == '√' {
                    i += 1;
                } else {
                    while i < chars.len() && chars[i].is_ascii_alphanumeric() {
                        i += 1;
                    }
                }
                out.push(Tok::Ident(chars[start..i].iter().collect::<String>().to_lowercase()));
            }
            '+' | '-' | '−' | '*' | '×' | '·' | '/' | '÷' | '^' | '%' | '!' => {
                let op = match c {
                    '−' => '-',
                    '×' | '·' => '*',
                    '÷' => '/',
                    c => c,
                };
                out.push(Tok::Op(op));
                i += 1;
            }
            '(' => {
                out.push(Tok::Open);
                i += 1;
            }
            ')' => {
                out.push(Tok::Close);
                i += 1;
            }
            c => return Err(format!("Unexpected “{c}”")),
        }
    }
    Ok(out)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    /// Answer to the previous calculation, for `ans`.
    ans: f64,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn expr(&mut self) -> Result<f64, String> {
        let mut v = self.term()?;
        while let Some(Tok::Op(op @ ('+' | '-'))) = self.peek().cloned() {
            self.pos += 1;
            let r = self.term()?;
            v = if op == '+' { v + r } else { v - r };
        }
        Ok(v)
    }

    fn term(&mut self) -> Result<f64, String> {
        let mut v = self.unary()?;
        loop {
            match self.peek().cloned() {
                Some(Tok::Op(op @ ('*' | '/'))) => {
                    self.pos += 1;
                    let r = self.unary()?;
                    v = if op == '*' { v * r } else { v / r };
                }
                // Implicit multiplication: 2π, 3(4+1), (1+1)(2+2).
                Some(Tok::Open | Tok::Ident(_) | Tok::Num(_)) => v *= self.power()?,
                _ => return Ok(v),
            }
        }
    }

    fn unary(&mut self) -> Result<f64, String> {
        match self.peek() {
            Some(Tok::Op('-')) => {
                self.pos += 1;
                Ok(-self.unary()?)
            }
            Some(Tok::Op('+')) => {
                self.pos += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Result<f64, String> {
        let base = self.postfix()?;
        if let Some(Tok::Op('^')) = self.peek() {
            self.pos += 1;
            // Right-associative, and binds tighter than a leading minus: -2^2 = -4.
            let exp = self.unary()?;
            return Ok(base.powf(exp));
        }
        Ok(base)
    }

    fn postfix(&mut self) -> Result<f64, String> {
        let mut v = self.atom()?;
        loop {
            match self.peek() {
                Some(Tok::Op('%')) => {
                    self.pos += 1;
                    v /= 100.0;
                }
                Some(Tok::Op('!')) => {
                    self.pos += 1;
                    if v < 0.0 || v.fract() != 0.0 || v > 170.0 {
                        return Err("Factorial needs a whole number from 0 to 170".into());
                    }
                    v = (1..=v as u64).map(|n| n as f64).product();
                }
                _ => return Ok(v),
            }
        }
    }

    fn atom(&mut self) -> Result<f64, String> {
        match self.next() {
            Some(Tok::Num(n)) => Ok(n),
            Some(Tok::Open) => {
                let v = self.expr()?;
                // Forgive missing closing brackets at the end: "2*(3+4" is 14.
                match self.next() {
                    Some(Tok::Close) | None => Ok(v),
                    _ => Err("Expected “)”".into()),
                }
            }
            Some(Tok::Ident(name)) => {
                match name.as_str() {
                    "pi" | "π" => return Ok(std::f64::consts::PI),
                    "e" => return Ok(std::f64::consts::E),
                    "tau" => return Ok(std::f64::consts::TAU),
                    "ans" => return Ok(self.ans),
                    _ => {}
                }
                let f: fn(f64) -> f64 = match name.as_str() {
                    "sqrt" | "√" => f64::sqrt,
                    "cbrt" => f64::cbrt,
                    "sin" => f64::sin,
                    "cos" => f64::cos,
                    "tan" => f64::tan,
                    "asin" => f64::asin,
                    "acos" => f64::acos,
                    "atan" => f64::atan,
                    "ln" => f64::ln,
                    "log" => f64::log10,
                    "exp" => f64::exp,
                    "abs" => f64::abs,
                    "round" => f64::round,
                    "floor" => f64::floor,
                    "ceil" => f64::ceil,
                    _ => return Err(format!("Unknown name “{name}”")),
                };
                // Functions take a bracketed argument, or a single value: √2, sin π.
                let arg = if let Some(Tok::Open) = self.peek() { self.atom()? } else { self.power()? };
                Ok(f(arg))
            }
            Some(Tok::Close) => Err("Unexpected “)”".into()),
            Some(Tok::Op(op)) => Err(format!("Unexpected “{op}”")),
            None => Err("Incomplete expression".into()),
        }
    }
}

/// Evaluates `src`. `ans` is the value of the name `ans`.
pub fn eval(src: &str, ans: f64) -> Result<f64, String> {
    let toks = lex(src)?;
    if toks.is_empty() {
        return Err("Empty".into());
    }
    let mut p = Parser { toks, pos: 0, ans };
    let v = p.expr()?;
    if p.pos < p.toks.len() {
        return Err(format!("Unexpected {:?}", p.toks[p.pos]));
    }
    if v.is_nan() {
        return Err("Not a number".into());
    }
    Ok(v)
}

/// Formats a result: up to 12 significant digits, with thousands separators
/// and scientific notation for very large or small values.
pub fn format(v: f64) -> String {
    if v.is_infinite() {
        return if v > 0.0 { "∞".into() } else { "−∞".into() };
    }
    let a = v.abs();
    if a != 0.0 && !(1e-6..1e15).contains(&a) {
        let s = format!("{v:.9e}");
        let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
        let m = m.trim_end_matches('0').trim_end_matches('.');
        return format!("{m}×10^{e}").replace('-', "−");
    }
    let rounded = format!("{:.*}", (11 - a.log10().floor().max(0.0) as usize).min(11), v);
    let trimmed = if rounded.contains('.') { rounded.trim_end_matches('0').trim_end_matches('.') } else { &rounded };
    let (neg, digits) = trimmed.strip_prefix('-').map_or((false, trimmed), |d| (true, d));
    let (int, frac) = digits.split_once('.').map_or((digits, None), |(i, f)| (i, Some(f)));
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    let mut out = if neg && grouped != "0" || neg && frac.is_some() { format!("−{grouped}") } else { grouped };
    if let Some(f) = frac {
        out.push('.');
        out.push_str(f);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str) -> f64 {
        eval(s, 0.0).unwrap()
    }

    #[test]
    fn precedence_and_brackets() {
        assert_eq!(ev("2+3×4"), 14.0);
        assert_eq!(ev("(2+3)*4"), 20.0);
        assert_eq!(ev("2^3^2"), 512.0);
        assert_eq!(ev("-2^2"), -4.0);
        assert_eq!(ev("10 − 4 ÷ 2"), 8.0);
        assert_eq!(ev("2*(3+4"), 14.0);
    }

    #[test]
    fn functions_constants_and_implicit_multiplication() {
        assert!((ev("2π") - std::f64::consts::TAU).abs() < 1e-12);
        assert_eq!(ev("sqrt(16)"), 4.0);
        assert_eq!(ev("√9 + 1"), 4.0);
        assert_eq!(ev("3(1+1)"), 6.0);
        assert_eq!(ev("5!"), 120.0);
        assert_eq!(ev("50%"), 0.5);
        assert_eq!(ev("1,250 + 1.5e3"), 2750.0);
        assert_eq!(eval("ans*2", 21.0).unwrap(), 42.0);
    }

    #[test]
    fn errors() {
        assert!(eval("2+", 0.0).is_err());
        assert!(eval("foo(2)", 0.0).is_err());
        assert!(eval("sqrt(-1)", 0.0).is_err());
    }

    #[test]
    fn formatting() {
        assert_eq!(format(1234567.5), "1,234,567.5");
        assert_eq!(format(0.1 + 0.2), "0.3");
        assert_eq!(format(-42.0), "−42");
        assert_eq!(format(1e20), "1×10^20");
        assert_eq!(format(1.0 / 3.0), "0.33333333333");
    }
}
