use mojo_lite::{Lexer, LexError};

fn main() {
    let test_cases = vec![
        (
            "Standard block",
            "var a = 1\nvar block:\n    var b = 2\n    var c = 3\nvar d = 4",
        ),
        (
            "Multi-line parens",
            "var a = (\n    True\n)\nvar b = 2",
        ),
        (
            "Indentation Error",
            "var a = 1\n    var b = 2", // Indents without a trigger
        ),
    ];

    for (name, source) in test_cases {
        println!("=== Test: {} ===", name);
        println!("Source:\n{}\n", source);
        println!("Tokens:");
        
        let lexer = Lexer::new(source);
        
        for token_result in lexer {
            match token_result {
                Ok(token) => println!("  {:?}", token),
                Err(err) => {
                    println!("  ❌ Error: {}", err);
                    break;
                }
            }
        }
        println!("\n");
    }
}
