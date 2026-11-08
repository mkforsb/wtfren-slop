//! Turns source text into tokens.
//! The mock walks through EVERY line of this file, to exercise deep scrolling.

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Number(f64),
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
}

pub const KEYWORD_0: &str = "keyword-0";
pub const KEYWORD_1: &str = "keyword-1";
pub const KEYWORD_2: &str = "keyword-2";
pub const KEYWORD_3: &str = "keyword-3";
pub const KEYWORD_4: &str = "keyword-4";
pub const KEYWORD_5: &str = "keyword-5";
pub const KEYWORD_6: &str = "keyword-6";
pub const KEYWORD_7: &str = "keyword-7";
pub const KEYWORD_8: &str = "keyword-8";
pub const KEYWORD_9: &str = "keyword-9";
pub const KEYWORD_10: &str = "keyword-10";
pub const KEYWORD_11: &str = "keyword-11";
pub const KEYWORD_12: &str = "keyword-12";
pub const KEYWORD_13: &str = "keyword-13";
pub const KEYWORD_14: &str = "keyword-14";
pub const KEYWORD_15: &str = "keyword-15";
pub const KEYWORD_16: &str = "keyword-16";
pub const KEYWORD_17: &str = "keyword-17";
pub const KEYWORD_18: &str = "keyword-18";
pub const KEYWORD_19: &str = "keyword-19";
pub const KEYWORD_20: &str = "keyword-20";
pub const KEYWORD_21: &str = "keyword-21";
pub const KEYWORD_22: &str = "keyword-22";
pub const KEYWORD_23: &str = "keyword-23";
pub const KEYWORD_24: &str = "keyword-24";
pub const KEYWORD_25: &str = "keyword-25";
pub const KEYWORD_26: &str = "keyword-26";
pub const KEYWORD_27: &str = "keyword-27";
pub const KEYWORD_28: &str = "keyword-28";
pub const KEYWORD_29: &str = "keyword-29";
pub const KEYWORD_30: &str = "keyword-30";
pub const KEYWORD_31: &str = "keyword-31";
pub const KEYWORD_32: &str = "keyword-32";
pub const KEYWORD_33: &str = "keyword-33";
pub const KEYWORD_34: &str = "keyword-34";
pub const KEYWORD_35: &str = "keyword-35";
pub const KEYWORD_36: &str = "keyword-36";
pub const KEYWORD_37: &str = "keyword-37";
pub const KEYWORD_38: &str = "keyword-38";
pub const KEYWORD_39: &str = "keyword-39";

pub fn tokenize(source: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = source.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' => {
                chars.next();
            }
            '+' => {
                tokens.push(Token::Plus);
                chars.next();
            }
            '-' => {
                tokens.push(Token::Minus);
                chars.next();
            }
            '*' => {
                tokens.push(Token::Star);
                chars.next();
            }
            '/' => {
                tokens.push(Token::Slash);
                chars.next();
            }
            '(' => {
                tokens.push(Token::LParen);
                chars.next();
            }
            ')' => {
                tokens.push(Token::RParen);
                chars.next();
            }
            _ => {
                let mut number = String::new();
                while let Some(&d) = chars.peek() {
                    if d.is_ascii_digit() || d == '.' {
                        number.push(d);
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(Token::Number(number.parse().unwrap_or(0.0)));
            }
        }
    }
    tokens
}
