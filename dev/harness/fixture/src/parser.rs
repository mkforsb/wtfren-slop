//! A recursive descent parser for arithmetic expressions.

use crate::lexer::Token;

pub enum Expr {
    Number(f64),
    Binary(Box<Expr>, Token, Box<Expr>),
}

impl Expr {
    pub fn eval(&self) -> f64 {
        match self {
            Expr::Number(n) => *n,
            Expr::Binary(a, op, b) => match op {
                Token::Plus => a.eval() + b.eval(),
                Token::Minus => a.eval() - b.eval(),
                Token::Star => a.eval() * b.eval(),
                Token::Slash => a.eval() / b.eval(),
                _ => unreachable!("not a binary operator"),
            },
        }
    }
}

pub fn parse(tokens: &[Token]) -> Result<Expr, String> {
    let mut pos = 0;
    let expr = sum(tokens, &mut pos)?;
    if pos == tokens.len() { Ok(expr) } else { Err(format!("unexpected token at {pos}")) }
}

fn sum(tokens: &[Token], pos: &mut usize) -> Result<Expr, String> {
    let mut left = product(tokens, pos)?;
    while let Some(op @ (Token::Plus | Token::Minus)) = tokens.get(*pos) {
        *pos += 1;
        left = Expr::Binary(Box::new(left), op.clone(), Box::new(product(tokens, pos)?));
    }
    Ok(left)
}

fn product(tokens: &[Token], pos: &mut usize) -> Result<Expr, String> {
    let mut left = atom(tokens, pos)?;
    while let Some(op @ (Token::Star | Token::Slash)) = tokens.get(*pos) {
        *pos += 1;
        left = Expr::Binary(Box::new(left), op.clone(), Box::new(atom(tokens, pos)?));
    }
    Ok(left)
}

fn atom(tokens: &[Token], pos: &mut usize) -> Result<Expr, String> {
    match tokens.get(*pos) {
        Some(Token::Number(n)) => {
            *pos += 1;
            Ok(Expr::Number(*n))
        }
        Some(Token::LParen) => {
            *pos += 1;
            let inner = sum(tokens, pos)?;
            *pos += 1;
            Ok(inner)
        }
        other => Err(format!("unexpected {other:?}")),
    }
}
