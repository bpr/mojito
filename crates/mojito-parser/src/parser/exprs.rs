//! Expression parsing: precedence climbing, comprehensions, string
//! sequences, lambdas, and prefix/infix forms.

use super::*;

impl<I: Iterator<Item = Result<(Token, Span), LexError>>> Parser<I> {
    /// Parses an expression whose operators all bind more tightly than
    /// `min_precedence` (precedence climbing).
    pub(super) fn parse_expression(
        &mut self,
        min_precedence: Precedence,
    ) -> Result<Expr, ParseError> {
        let left = self.parse_prefix()?;
        self.parse_expression_from(left, min_precedence)
    }

    /// Continues precedence climbing from an already-parsed `left` operand. Used
    /// when a leading atom has been consumed elsewhere (parameter-argument
    /// disambiguation).
    pub(super) fn parse_expression_from(
        &mut self,
        mut left: Expr,
        min_precedence: Precedence,
    ) -> Result<Expr, ParseError> {
        loop {
            // An adjacent `^` is the postfix transfer sigil (`primary '^'`),
            // which binds tightest: `p + q^` transfers `q`, not the sum. A
            // whitespace-separated `^` remains the bitwise-xor operator.
            let precedence = if matches!(self.peek_token()?, Some(Token::Caret))
                && left.span.1 == self.peek_start()
            {
                Precedence::Call
            } else {
                self.peek_precedence()?
            };
            if min_precedence >= precedence {
                break;
            }
            left = self.parse_infix(left)?;
        }
        Ok(left)
    }

    /// Parse the postfix generator/filter sequence shared by list, set, and
    /// dictionary comprehensions. Expressions use `Conditional` as their stop
    /// precedence so the next clause's `if` is not mistaken for a ternary.
    pub(super) fn parse_comprehension_clauses(
        &mut self,
    ) -> Result<Vec<mojito_ast::ast::ComprehensionClause>, ParseError> {
        use mojito_ast::ast::ComprehensionClause;

        let mut clauses = Vec::new();
        loop {
            match self.peek_token()? {
                Some(Token::For) => {
                    self.next_token()?;
                    let binding = self.parse_loop_binding_mode()?;
                    let var =
                        self.expect_identifier("Expected a comprehension variable after 'for'")?;
                    self.expect(Token::In, "Expected 'in' in comprehension")?;
                    let iter = self.parse_expression(Precedence::Conditional)?;
                    clauses.push(ComprehensionClause::For {
                        var,
                        binding,
                        iter: Box::new(iter),
                    });
                }
                Some(Token::If) => {
                    if clauses.is_empty() {
                        return Err(ParseError::UnexpectedToken(
                            Token::If,
                            "a comprehension must begin with a 'for' clause".to_string(),
                        ));
                    }
                    self.next_token()?;
                    let condition = self.parse_expression(Precedence::Conditional)?;
                    clauses.push(ComprehensionClause::If(Box::new(condition)));
                }
                _ => break,
            }
        }
        if clauses.is_empty() {
            return Err(ParseError::UnexpectedToken(
                self.peek_token()?.cloned().unwrap_or(Token::Eof),
                "expected a comprehension 'for' clause".to_string(),
            ));
        }
        Ok(clauses)
    }

    /// Parse one or more adjacent ordinary/triple string tokens, or one or more
    /// adjacent t-string tokens. Mojo concatenates within each family; a regular
    /// string and a `TString` remain distinct types and cannot form one literal.
    pub(super) fn build_string_sequence(
        &mut self,
        first: Token,
        start: usize,
    ) -> Result<Expr, ParseError> {
        let tstring_sequence = matches!(first, Token::TString { .. });
        let mut tokens = vec![first];
        while if tstring_sequence {
            matches!(self.peek_token()?, Some(Token::TString { .. }))
        } else {
            matches!(
                self.peek_token()?,
                Some(Token::StringLiteral(_)) | Some(Token::TripleStringLiteral(_))
            )
        } {
            tokens.push(self.next_token()?);
        }

        if !tstring_sequence {
            let mut value = String::new();
            for token in tokens {
                match token {
                    Token::StringLiteral(piece) | Token::TripleStringLiteral(piece) => {
                        value.push_str(&piece);
                    }
                    _ => unreachable!("t-string entered an ordinary literal sequence"),
                }
            }
            return Ok(self.node(ExprKind::Str(value), start));
        }

        let mut parts = Vec::new();
        let mut all_tstrings_raw = true;
        for token in tokens {
            match token {
                Token::TString { chunks, raw } => {
                    all_tstrings_raw &= raw;
                    for chunk in chunks {
                        match chunk {
                            TStringChunk::Text(text) => push_tstring_literal(&mut parts, text),
                            TStringChunk::Interp(src) => {
                                parts.push(TStringPart::Expr(Box::new(parse_interpolation(&src)?)))
                            }
                        }
                    }
                }
                _ => unreachable!("ordinary string entered a t-string sequence"),
            }
        }
        Ok(self.node(
            ExprKind::TString {
                parts,
                raw: all_tstrings_raw,
            },
            start,
        ))
    }

    /// Parse a lambda expression after its consumed `lambda` keyword:
    /// `lambda [params] [(args)] [effects] [{captures}] [-> T]: expr`.
    /// Synthesizes the hidden nested definition documented on
    /// [`ExprKind::Lambda`]: an unspellable `$lambda$<start>` name and a
    /// one-statement `return <expr>` body, so an omitted `-> T` pins the
    /// fixed `None` return and the body coercion is the ordinary return check.
    pub(super) fn parse_lambda(&mut self, start: usize) -> Result<Expr, ParseError> {
        let type_params = self.parse_type_params()?;
        let mut params = Vec::new();
        let mut positional_only = None;
        let mut keyword_only = None;
        if matches!(self.peek_token()?, Some(Token::LParen)) {
            self.next_token()?; // consume '('
            let list = self.parse_params()?;
            params = list.params;
            positional_only = list.positional_only;
            keyword_only = list.keyword_only;
            self.expect(Token::RParen, "Expected ')' after lambda arguments")?;
        } else if matches!(
            self.peek_token()?,
            Some(Token::Identifier(word)) if !matches!(word.as_str(), "capturing" | "thin" | "abi")
        ) {
            return Err(ParseError::UnexpectedToken(
                self.next_token()?,
                "lambda arguments must be parenthesized and typed, e.g. 'lambda (x: Int) ...'"
                    .to_string(),
            ));
        }
        let (raises, raises_type) = self.parse_callable_effects()?;
        let captures = self.parse_capture_list()?;
        let ret = if matches!(self.peek_token()?, Some(Token::Arrow)) {
            self.next_token()?; // consume '->'
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(Token::Colon, "Expected ':' before the lambda body")?;
        // The body is one expression above walrus level: a trailing ternary is
        // part of the body, while `:=`, `,`, and statement structure are not.
        let body = self.parse_expression(Precedence::Walrus)?;
        let body_span = body.span;
        let return_stmt = Stmt::new(StmtKind::Return(Some(body)), body_span);
        let def = Stmt::new(
            StmtKind::Def {
                name: format!("$lambda${start}"),
                decorators: Vec::new(),
                type_params,
                params,
                positional_only,
                keyword_only,
                captures,
                raises,
                raises_type,
                ret,
                where_clauses: Vec::new(),
                body: vec![return_stmt],
            },
            (start, self.last_span.1),
        );
        Ok(self.node(ExprKind::Lambda { def: Box::new(def) }, start))
    }

    pub(super) fn parse_prefix(&mut self) -> Result<Expr, ParseError> {
        let start = self.peek_start();
        let token = self.next_token()?;
        match token {
            Token::IntLiteral(val) => Ok(self.node(ExprKind::Int(val), start)),
            Token::FloatLiteral(val) => Ok(self.node(ExprKind::Float(val), start)),
            Token::BoolLiteral(val) => Ok(self.node(ExprKind::Bool(val), start)),
            token @ (Token::StringLiteral(_)
            | Token::TripleStringLiteral(_)
            | Token::TString { .. }) => self.build_string_sequence(token, start),
            Token::None => Ok(self.node(ExprKind::None, start)),
            Token::Ellipsis => Ok(self.node(ExprKind::Identifier("...".into()), start)),
            Token::Identifier(id) => Ok(self.node(ExprKind::Identifier(id), start)),
            Token::Def => {
                let ty = self.parse_function_type_tail()?;
                Ok(self.node(ExprKind::TypeValue(ty), start))
            }
            Token::Lambda => self.parse_lambda(start),
            Token::Minus => {
                let operand = self.parse_expression(Precedence::Unary)?;
                Ok(self.node(ExprKind::Prefix(PrefixOp::Neg, Box::new(operand)), start))
            }
            Token::Not => {
                let operand = self.parse_expression(Precedence::Not)?;
                Ok(self.node(ExprKind::Prefix(PrefixOp::Not, Box::new(operand)), start))
            }
            Token::LParen => {
                // `()` — the empty tuple.
                if matches!(self.peek_token()?, Some(Token::RParen)) {
                    self.next_token()?; // consume ')'
                    return Ok(self.node(ExprKind::TupleLit(Vec::new()), start));
                }
                let first = self.parse_expression(Precedence::Lowest)?;
                // A comma makes it a tuple: `(a,)`, `(a, b)`, `(a, b,)`. Without a
                // comma it is plain grouping `(e)`.
                if matches!(self.peek_token()?, Some(Token::Comma)) {
                    let mut elems = vec![first];
                    while matches!(self.peek_token()?, Some(Token::Comma)) {
                        self.next_token()?; // consume ','
                        if matches!(self.peek_token()?, Some(Token::RParen)) {
                            break; // trailing comma
                        }
                        elems.push(self.parse_expression(Precedence::Lowest)?);
                    }
                    self.expect(Token::RParen, "Expected ')' after tuple elements")?;
                    Ok(self.node(ExprKind::TupleLit(elems), start))
                } else {
                    self.expect(Token::RParen, "Expected closing ')' after expression")?;
                    Ok(first)
                }
            }
            // A list literal `[a, b, …]`. Empty `[]` can't infer an element type,
            // but is retained so an enclosing type annotation can supply it.
            Token::LBracket => {
                if matches!(self.peek_token()?, Some(Token::RBracket)) {
                    self.next_token()?; // consume ']'
                    return Ok(self.node(ExprKind::ListLit(Vec::new()), start));
                }
                let first = self.parse_expression(Precedence::Lowest)?;
                if matches!(self.peek_token()?, Some(Token::For)) {
                    let clauses = self.parse_comprehension_clauses()?;
                    self.expect(Token::RBracket, "Expected ']' after list comprehension")?;
                    return Ok(self.node(
                        ExprKind::Comprehension {
                            kind: mojito_ast::ast::CollectionKind::List,
                            key: None,
                            value: Box::new(first),
                            clauses,
                        },
                        start,
                    ));
                }
                let mut elems = vec![first];
                while matches!(self.peek_token()?, Some(Token::Comma)) {
                    self.next_token()?;
                    if matches!(self.peek_token()?, Some(Token::RBracket)) {
                        break;
                    }
                    elems.push(self.parse_expression(Precedence::Lowest)?);
                }
                self.expect(Token::RBracket, "Expected ']' after list elements")?;
                Ok(self.node(ExprKind::ListLit(elems), start))
            }
            Token::LBrace => {
                if matches!(self.peek_token()?, Some(Token::RBrace)) {
                    self.next_token()?;
                    return Ok(self.node(ExprKind::BraceLit(Vec::new()), start));
                }
                let first_key = self.parse_expression(Precedence::Lowest)?;
                let first_value = if matches!(self.peek_token()?, Some(Token::Colon)) {
                    self.next_token()?;
                    Some(self.parse_expression(Precedence::Lowest)?)
                } else {
                    None
                };
                if matches!(self.peek_token()?, Some(Token::For)) {
                    let kind = if first_value.is_some() {
                        mojito_ast::ast::CollectionKind::Dict
                    } else {
                        mojito_ast::ast::CollectionKind::Set
                    };
                    let value = first_value.unwrap_or_else(|| first_key.clone());
                    let clauses = self.parse_comprehension_clauses()?;
                    self.expect(Token::RBrace, "Expected '}' after collection comprehension")?;
                    return Ok(self.node(
                        ExprKind::Comprehension {
                            kind,
                            key: (kind == mojito_ast::ast::CollectionKind::Dict)
                                .then(|| Box::new(first_key)),
                            value: Box::new(value),
                            clauses,
                        },
                        start,
                    ));
                }
                let dictionary = first_value.is_some();
                let mut entries = vec![(first_key, first_value)];
                while matches!(self.peek_token()?, Some(Token::Comma)) {
                    self.next_token()?;
                    if matches!(self.peek_token()?, Some(Token::RBrace)) {
                        break;
                    }
                    let key = self.parse_expression(Precedence::Lowest)?;
                    let value = if matches!(self.peek_token()?, Some(Token::Colon)) {
                        self.next_token()?;
                        Some(self.parse_expression(Precedence::Lowest)?)
                    } else {
                        None
                    };
                    if dictionary != value.is_some() {
                        return Err(ParseError::UnexpectedToken(
                            self.peek_token()?.cloned().unwrap_or(Token::RBrace),
                            "set elements and dictionary key/value pairs cannot be mixed"
                                .to_string(),
                        ));
                    }
                    entries.push((key, value));
                }
                self.expect(Token::RBrace, "Expected '}' after brace literal")?;
                Ok(self.node(ExprKind::BraceLit(entries), start))
            }
            // A leading-dot contextual member reference (`.red`,
            // `.hsb_to_rgb(120, 100, 50)`): the base type comes from the
            // expression's expected type during checking. The object is the
            // compiler-internal `$contextual` sentinel (unspellable in
            // source), spanned at the dot so diagnostics point here; postfix
            // chains and calls attach through ordinary infix parsing.
            Token::Dot => {
                let sentinel = self.node(
                    ExprKind::Identifier(mojito_ast::ast::CONTEXTUAL_SENTINEL.into()),
                    start,
                );
                let field = self.expect_identifier("Expected a member name after a leading '.'")?;
                if matches!(self.peek_token()?, Some(Token::LParen)) {
                    self.next_token()?; // consume '('
                    let (args, kwargs) = self.parse_call_args()?;
                    self.expect(Token::RParen, "Expected ')' after arguments")?;
                    Ok(self.node(
                        ExprKind::MethodCall {
                            object: Box::new(sentinel),
                            method: field,
                            args,
                            kwargs,
                        },
                        start,
                    ))
                } else {
                    Ok(self.node(
                        ExprKind::Member {
                            object: Box::new(sentinel),
                            field,
                        },
                        start,
                    ))
                }
            }
            token => Err(ParseError::UnexpectedToken(
                token,
                "Expected an expression".into(),
            )),
        }
    }

    /// Parses an infix/postfix continuation of `left`: either a binary operator
    /// or a call `(...)`. Only invoked when the next token is such an operator.
    pub(super) fn parse_infix(&mut self, left: Expr) -> Result<Expr, ParseError> {
        // Every node built here spans from `left`'s start to the last token consumed.
        let start = left.span.0;
        // Postfix transfer sigil `expr '^'`.
        if matches!(self.peek_token()?, Some(Token::Caret)) && left.span.1 == self.peek_start() {
            self.next_token()?; // consume '^'
            return Ok(self.node(ExprKind::Transfer(Box::new(left)), start));
        }
        // Postfix member access `expr '.' NAME` or method call `expr '.' NAME (args)`.
        if matches!(self.peek_token()?, Some(Token::Dot)) {
            self.next_token()?; // consume '.'
            let field = self.expect_identifier("Expected a field or method name after '.'")?;
            if matches!(self.peek_token()?, Some(Token::LParen)) {
                self.next_token()?; // consume '('
                let (args, kwargs) = self.parse_call_args()?;
                self.expect(Token::RParen, "Expected ')' after arguments")?;
                return Ok(self.node(
                    ExprKind::MethodCall {
                        object: Box::new(left),
                        method: field,
                        args,
                        kwargs,
                    },
                    start,
                ));
            }
            return Ok(self.node(
                ExprKind::Member {
                    object: Box::new(left),
                    field,
                },
                start,
            ));
        }

        // Postfix `[`: a **slice** (`obj[lower:upper:step]`, any bound optional),
        // a call's explicit compile-time parameters (`NAME '[' args ']' '(' … ')'`),
        // or a plain subscript (`obj '[' index ']'`). A top-level `:` inside the
        // brackets marks a slice; otherwise `(` following decides call vs subscript.
        if matches!(self.peek_token()?, Some(Token::LBracket)) {
            return self.parse_bracket_suffix(left, start);
        }

        // Postfix call without explicit parameters: `IDENT '(' args ')'`.
        if matches!(self.peek_token()?, Some(Token::LParen)) {
            self.next_token()?; // consume '('
            let (args, kwargs) = self.parse_call_args()?;
            self.expect(Token::RParen, "Expected ')' after arguments")?;
            let kind = match left.kind {
                ExprKind::Identifier(name) => ExprKind::Call {
                    name,
                    param_args: Vec::new(),
                    args,
                    kwargs,
                },
                _ => ExprKind::Invoke {
                    callee: Box::new(left),
                    param_args: Vec::new(),
                    args,
                    kwargs,
                },
            };
            return Ok(self.node(kind, start));
        }

        // Walrus / named expression: `name := value`. The target must be a bare
        // name. MIR preserves this as an explicit unsupported operation.
        if matches!(self.peek_token()?, Some(Token::ColonEq)) {
            self.next_token()?; // consume ':='
            let ExprKind::Identifier(name) = left.kind else {
                return Err(ParseError::UnexpectedToken(
                    Token::ColonEq,
                    format!("the walrus ':=' target must be a name, got {:?}", left.kind),
                ));
            };
            let value = self.parse_expression(Precedence::Lowest)?;
            return Ok(self.node(
                ExprKind::Named {
                    name,
                    value: Box::new(value),
                },
                start,
            ));
        }

        // Conditional expression (ternary): `then_branch if cond else else_branch`.
        // The condition is parsed at `Conditional` (an or-test — it won't grab the
        // `else`); the else branch is a full expression (so ternaries nest right).
        if matches!(self.peek_token()?, Some(Token::If)) {
            self.next_token()?; // consume 'if'
            let cond = self.parse_expression(Precedence::Conditional)?;
            self.expect(Token::Else, "Expected 'else' in a conditional expression")?;
            let else_branch = self.parse_expression(Precedence::Lowest)?;
            return Ok(self.node(
                ExprKind::IfExpr {
                    cond: Box::new(cond),
                    then_branch: Box::new(left),
                    else_branch: Box::new(else_branch),
                },
                start,
            ));
        }

        // Comparison, possibly chained: `a < b`, `a in b`, `a not in b`, and chains
        // like `a < b <= c` or `0 <= i < n`. Each operand is parsed up to the next
        // comparison operator. A single comparison stays an `Infix` (so existing
        // behavior is unchanged); a chain of length ≥ 2 becomes an `Expr::Compare`.
        // In infix position, `not` can only begin `not in`.
        if self.peek_is_comparison()? {
            let mut rest: Vec<(InfixOp, Expr)> = Vec::new();
            loop {
                let op = self.parse_comparison_op()?;
                let right = self.parse_expression(Precedence::Comparison)?;
                rest.push((op, right));
                if !self.peek_is_comparison()? {
                    break;
                }
            }
            if rest.len() == 1 {
                let (op, right) = rest.into_iter().next().unwrap();
                return Ok(self.node(ExprKind::Infix(op, Box::new(left), Box::new(right)), start));
            }
            return Ok(self.node(
                ExprKind::Compare {
                    first: Box::new(left),
                    rest,
                },
                start,
            ));
        }

        let op_token = self.next_token()?;
        let op = match op_token {
            Token::Plus => InfixOp::Add,
            Token::Minus => InfixOp::Sub,
            Token::Star => InfixOp::Mul,
            Token::Slash => InfixOp::Div,
            Token::DoubleSlash => InfixOp::FloorDiv,
            Token::Percent => InfixOp::Mod,
            Token::At => InfixOp::MatMul,
            Token::Shl => InfixOp::Shl,
            Token::Shr => InfixOp::Shr,
            Token::Amp => InfixOp::BitAnd,
            Token::Pipe => InfixOp::BitOr,
            Token::Caret => InfixOp::BitXor,
            Token::DoubleStar => InfixOp::Pow,
            // Comparisons (`== != < > <= >=`, `in`, `not in`) are handled by the
            // chained-comparison path above, never here.
            Token::And => InfixOp::And,
            Token::Or => InfixOp::Or,
            token => {
                return Err(ParseError::UnexpectedToken(
                    token,
                    "Expected a binary operator".into(),
                ));
            }
        };

        // Left-associative: parse the right operand at the operator's own
        // precedence so equal-precedence operators don't get reabsorbed.
        let right = self.parse_expression(infix_precedence(op))?;
        Ok(self.node(ExprKind::Infix(op, Box::new(left), Box::new(right)), start))
    }
}
