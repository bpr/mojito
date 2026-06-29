use crate::error::LexError;
use std::collections::VecDeque;

/// The minimal token set required to parse our pseudo-Python language.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Keywords & Identifiers
    Var,
    Identifier(String),
    
    // Literals
    IntLiteral(i64),
    BoolLiteral(bool),
    
    // Operators & Punctuation
    Assign,
    Colon,
    LParen,
    RParen,
    
    // Structural (Offside Rule) Tokens
    Newline,
    Indent,
    Dedent,
    Eof,
}

pub struct Lexer<'a> {
    input: &'a str,
    pos: usize,
    
    // Offside rule and scoping state
    indent_stack: Vec<usize>,
    paren_count: usize,
    at_line_start: bool,
    eof_emitted: bool,
    
    // Queue for when a single character (or EOF) produces multiple tokens
    pending_tokens: VecDeque<Token>,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        Self {
            input,
            pos: 0,
            indent_stack: vec![0],
            paren_count: 0,
            at_line_start: true,
            eof_emitted: false,
            pending_tokens: VecDeque::new(),
        }
    }

    /// Helper to peek at the remaining string
    fn remainder(&self) -> &'a str {
        &self.input[self.pos..]
    }
}

impl<'a> Iterator for Lexer<'a> {
    type Item = Result<Token, LexError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // 1. Drain any pending tokens first (e.g., multiple Dedents)
            if let Some(token) = self.pending_tokens.pop_front() {
                return Some(Ok(token));
            }

            // 2. Stop completely if we've already emitted the EOF token
            if self.eof_emitted {
                return None;
            }

            // 3. Handle End of File
            if self.pos >= self.input.len() {
                // If the file didn't end with a newline but had tokens, emit one
                if !self.at_line_start {
                    self.pending_tokens.push_back(Token::Newline);
                }
                
                // Unwind the indentation stack
                while self.indent_stack.len() > 1 {
                    self.indent_stack.pop();
                    self.pending_tokens.push_back(Token::Dedent);
                }
                
                self.pending_tokens.push_back(Token::Eof);
                self.eof_emitted = true;
                continue; // Loop around to pop the tokens we just enqueued
            }

            // 4. Handle indentation at the start of a logical line
            if self.at_line_start {
                let mut spaces = 0;
                let mut temp_pos = self.pos;
                let mut is_blank_line = false;

                // Count leading spaces
                for c in self.remainder().chars() {
                    if c == ' ' {
                        spaces += 1;
                        temp_pos += c.len_utf8();
                    } else if c == '\n' || c == '\r' {
                        is_blank_line = true;
                        break;
                    } else {
                        break;
                    }
                }

                if is_blank_line {
                    // Ignore completely blank lines. Consume up to the newline.
                    self.pos = temp_pos;
                    if self.remainder().starts_with("\r\n") {
                        self.pos += 2;
                    } else {
                        self.pos += 1;
                    }
                    self.at_line_start = true;
                    continue; 
                }

                self.pos = temp_pos;
                self.at_line_start = false;

                // Only evaluate indentation if we are NOT inside parentheses
                if self.paren_count == 0 {
                    let current_indent = *self.indent_stack.last().unwrap();
                    
                    if spaces > current_indent {
                        self.indent_stack.push(spaces);
                        self.pending_tokens.push_back(Token::Indent);
                        continue;
                    } else if spaces < current_indent {
                        while let Some(&top) = self.indent_stack.last() {
                            if top > spaces {
                                self.indent_stack.pop();
                                self.pending_tokens.push_back(Token::Dedent);
                            } else if top == spaces {
                                break;
                            } else {
                                return Some(Err(LexError::IndentationError(self.pos)));
                            }
                        }
                        if !self.pending_tokens.is_empty() {
                            continue;
                        }
                    }
                }
            }

            // 5. Consume characters
            let c = self.remainder().chars().next().unwrap();

            match c {
                ' ' | '\t' | '\r' => {
                    // Inline whitespace is ignored
                    self.pos += c.len_utf8();
                }
                '\n' => {
                    self.pos += 1;
                    if self.paren_count == 0 {
                        self.at_line_start = true;
                        self.pending_tokens.push_back(Token::Newline);
                        continue;
                    }
                }
                '(' => {
                    self.pos += 1;
                    self.paren_count += 1;
                    self.pending_tokens.push_back(Token::LParen);
                    continue;
                }
                ')' => {
                    self.pos += 1;
                    if self.paren_count > 0 {
                        self.paren_count -= 1;
                    } else {
                        return Some(Err(LexError::UnmatchedParenthesis(self.pos)));
                    }
                    self.pending_tokens.push_back(Token::RParen);
                    continue;
                }
                ':' => {
                    self.pos += 1;
                    self.pending_tokens.push_back(Token::Colon);
                    continue;
                }
                '=' => {
                    self.pos += 1;
                    self.pending_tokens.push_back(Token::Assign);
                    continue;
                }
                _ if c.is_ascii_alphabetic() || c == '_' => {
                    let start = self.pos;
                    while self.pos < self.input.len() {
                        let next_c = self.remainder().chars().next().unwrap();
                        if next_c.is_ascii_alphanumeric() || next_c == '_' {
                            self.pos += next_c.len_utf8();
                        } else {
                            break;
                        }
                    }
                    
                    let text = &self.input[start..self.pos];
                    let token = match text {
                        "var" => Token::Var,
                        "True" => Token::BoolLiteral(true),
                        "False" => Token::BoolLiteral(false),
                        _ => Token::Identifier(text.to_string()),
                    };
                    self.pending_tokens.push_back(token);
                    continue;
                }
                _ if c.is_ascii_digit() => {
                    let start = self.pos;
                    while self.pos < self.input.len() {
                        let next_c = self.remainder().chars().next().unwrap();
                        if next_c.is_ascii_digit() {
                            self.pos += next_c.len_utf8();
                        } else {
                            break;
                        }
                    }
                    
                    let text = &self.input[start..self.pos];
                    if let Ok(num) = text.parse::<i64>() {
                        self.pending_tokens.push_back(Token::IntLiteral(num));
                    } else {
                        return Some(Err(LexError::InvalidInteger(start)));
                    }
                    continue;
                }
                _ => {
                    return Some(Err(LexError::UnexpectedCharacter(c, self.pos)));
                }
            }
        }
    }
}
