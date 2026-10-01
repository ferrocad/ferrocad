//! A small expression parser + evaluator for property expressions.
//! Supports numbers, `+ - * /`, unary minus, parentheses, and identifiers
//! (including qualified `Object.Property` names).

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Number(f64),
    Var(String),
    UnaryNeg(Box<Expr>),
    Binary(Box<Expr>, Op, Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

pub fn parse(input: &str) -> Result<Expr, String> {
    let mut parser = Parser {
        chars: input.chars().peekable(),
    };
    let expr = parser.parse_additive()?;
    if let Some(c) = parser.peek() {
        return Err(format!("unexpected character '{c}'"));
    }
    Ok(expr)
}

impl Expr {
    pub fn eval(&self, resolve: &dyn Fn(&str) -> Option<f64>) -> Result<f64, String> {
        match self {
            Expr::Number(n) => Ok(*n),
            Expr::Var(name) => resolve(name).ok_or_else(|| format!("undefined variable '{name}'")),
            Expr::UnaryNeg(e) => Ok(-e.eval(resolve)?),
            Expr::Binary(l, op, r) => {
                let lv = l.eval(resolve)?;
                let rv = r.eval(resolve)?;
                match op {
                    Op::Add => Ok(lv + rv),
                    Op::Sub => Ok(lv - rv),
                    Op::Mul => Ok(lv * rv),
                    Op::Div => {
                        if rv == 0.0 {
                            Err("division by zero".to_string())
                        } else {
                            Ok(lv / rv)
                        }
                    }
                }
            }
        }
    }
}

struct Parser<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while matches!(self.chars.peek(), Some(c) if c.is_whitespace()) {
            self.chars.next();
        }
    }

    fn peek(&mut self) -> Option<char> {
        self.skip_ws();
        self.chars.peek().copied()
    }

    fn next(&mut self) -> Option<char> {
        self.skip_ws();
        self.chars.next()
    }

    fn parse_additive(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_multiplicative()?;
        loop {
            match self.peek() {
                Some('+') => {
                    self.next();
                    let rhs = self.parse_multiplicative()?;
                    left = Expr::Binary(Box::new(left), Op::Add, Box::new(rhs));
                }
                Some('-') => {
                    self.next();
                    let rhs = self.parse_multiplicative()?;
                    left = Expr::Binary(Box::new(left), Op::Sub, Box::new(rhs));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_primary()?;
        loop {
            match self.peek() {
                Some('*') => {
                    self.next();
                    let rhs = self.parse_primary()?;
                    left = Expr::Binary(Box::new(left), Op::Mul, Box::new(rhs));
                }
                Some('/') => {
                    self.next();
                    let rhs = self.parse_primary()?;
                    left = Expr::Binary(Box::new(left), Op::Div, Box::new(rhs));
                }
                _ => break,
            }
        }
        Ok(left)
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Some('(') => {
                self.next();
                let e = self.parse_additive()?;
                match self.next() {
                    Some(')') => Ok(e),
                    _ => Err("expected ')'".to_string()),
                }
            }
            Some('-') => {
                self.next();
                Ok(Expr::UnaryNeg(Box::new(self.parse_primary()?)))
            }
            Some(c) if c.is_ascii_digit() || c == '.' => self.parse_number(),
            Some(c) if c.is_ascii_alphabetic() || c == '_' => Ok(Expr::Var(self.parse_ident())),
            Some(c) => Err(format!("unexpected character '{c}'")),
            None => Err("unexpected end of expression".to_string()),
        }
    }

    fn parse_number(&mut self) -> Result<Expr, String> {
        let mut s = String::new();
        while let Some(&c) = self.chars.peek() {
            if c.is_ascii_digit() || c == '.' {
                s.push(c);
                self.chars.next();
            } else {
                break;
            }
        }
        s.parse::<f64>()
            .map(Expr::Number)
            .map_err(|e| format!("invalid number '{s}': {e}"))
    }

    fn parse_ident(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                s.push(c);
                self.next();
            } else {
                break;
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(src: &str, vars: &[(&str, f64)]) -> Result<f64, String> {
        let e = parse(src)?;
        e.eval(&|name| vars.iter().find(|(n, _)| *n == name).map(|(_, v)| *v))
    }

    #[test]
    fn arithmetic() {
        assert_eq!(eval("1 + 2 * 3", &[]).unwrap(), 7.0);
        assert_eq!(eval("(1 + 2) * 3", &[]).unwrap(), 9.0);
        assert_eq!(eval("10 / 4", &[]).unwrap(), 2.5);
        assert_eq!(eval("-2 * 3", &[]).unwrap(), -6.0);
    }

    #[test]
    fn variables_and_qualified_names() {
        assert_eq!(eval("A.Width * B.Height", &[("A.Width", 10.0), ("B.Height", 5.0)]).unwrap(), 50.0);
        assert_eq!(eval("Width + 1", &[("Width", 4.0)]).unwrap(), 5.0);
    }

    #[test]
    fn errors() {
        assert!(parse("1 +").is_err());
        assert!(parse("(1 + 2").is_err());
        assert!(eval("Missing", &[]).is_err());
        assert!(eval("1 / 0", &[]).is_err());
    }
}
