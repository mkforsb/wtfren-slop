mod lexer;
mod parser;

use std::io::BufRead;

fn main() {
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = line.expect("read a line");
        let tokens = lexer::tokenize(&line);
        match parser::parse(&tokens) {
            Ok(expr) => println!("{}", expr.eval()),
            Err(e) => eprintln!("error: {e}"),
        }
    }
}
