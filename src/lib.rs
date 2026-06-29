pub mod error;
pub mod lexer;
pub mod token;

// You can uncomment these as you build them out:
// pub mod token;
// pub mod parser;

// Optional: Re-export commonly used types at the crate root for convenience
pub use error::LexError;
pub use lexer::Lexer;
pub use token::Token;