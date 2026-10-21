//! Pattern parsing: the `match` statement and its `case` patterns, `case`
//! conditions of `if`/`while`, and recursive destructuring targets.
//!
//! Patterns and targets are two grammars over one vocabulary (`var`/`ref`
//! bindings, `_`, tuples, sequences). A case pattern is a test that may
//! bind; a target is a place to write or a name to declare. Neither is an
//! expression: an expression appears only as a literal, an `==` operand, a
//! call-shaped head, or a target's place.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::{
    Condition, DestructureTarget, DestructureTargetKind, MatchCase, MatchPattern, MatchPatternKind,
    SequencePattern, SequenceTarget,
};

/// One element of a parenthesized or bracketed statement display, before the
/// statement decides whether it is a destructuring target or an expression.
pub(super) enum TargetItem {
    /// A target: it holds a `var`/`ref` leaf, a star, or was parsed under a
    /// declaring prefix, so only `=` may follow.
    Target(DestructureTarget),
    /// An expression, convertible to a target if `=` follows.
    Expr(Expr),
}

impl<I: Iterator<Item = Result<(Token, Span), LexError>>> Parser<I> {
    /// `match subject: case ...` after the keyword is peeked. `comptime`
    /// selects the compile-time form. A comma-separated subject forms a
    /// tuple; the cases either share the `match` indent or sit one level in.
    pub(super) fn parse_match(&mut self, comptime: bool) -> Result<StmtKind, ParseError> {
        self.expect(&Token::Match, "Expected 'match'")?;
        let subject = self.parse_tuple_display()?;
        self.expect(&Token::Colon, "expected ':' after match subject")?;
        self.expect(
            &Token::Newline,
            "expected a newline after the match subject",
        )?;
        let indented = matches!(self.peek_token()?, Some(Token::Indent));
        if indented {
            self.next_token()?;
        }
        let mut cases = Vec::new();
        loop {
            while matches!(self.peek_token()?, Some(Token::Newline)) {
                self.next_token()?;
            }
            if !matches!(self.peek_token()?, Some(Token::Case)) {
                break;
            }
            cases.push(self.parse_case()?);
        }
        if indented {
            self.expect(&Token::Dedent, "expected a 'case' block")?;
        }
        if cases.is_empty() {
            return Err(ParseError::UnexpectedToken(
                Token::Match,
                "'match' statement must have at least one 'case' block".to_string(),
            ));
        }
        Ok(if comptime {
            StmtKind::ComptimeMatch { subject, cases }
        } else {
            StmtKind::Match { subject, cases }
        })
    }

    /// An `if`/`elif`/`while` condition: `case pattern = value`, or a
    /// boolean expression.
    pub(super) fn parse_condition(&mut self) -> Result<Condition, ParseError> {
        if !matches!(self.peek_token()?, Some(Token::Case)) {
            return Ok(Condition::Expr(self.parse_expression(Precedence::Lowest)?));
        }
        self.next_token()?; // consume 'case'
        let pattern = self.parse_case_patterns()?;
        self.expect(&Token::Assign, "expected '=' after the 'case' pattern")?;
        let value = self.parse_tuple_display()?;
        Ok(Condition::Case {
            pattern: Box::new(pattern),
            value,
        })
    }

    /// A statement-initial `(` or `[` display, or a `var`/`ref`-prefixed one
    /// (`declare`): the elements with their binding leaves, which the caller
    /// resolves to a destructuring target or an expression. A parenthesized
    /// single expression is grouping and comes back as that expression.
    pub(super) fn parse_target_display(
        &mut self,
        declare: Option<BindingMode>,
    ) -> Result<TargetItem, ParseError> {
        let start = self.peek_start();
        let open = self.next_token()?;
        let (close, sequence) = match open {
            Token::LParen => (Token::RParen, false),
            Token::LBracket => (Token::RBracket, true),
            token => {
                return Err(ParseError::UnexpectedToken(
                    token,
                    "expected '(' or '[' to open a destructuring target".to_string(),
                ));
            }
        };
        let mut items = Vec::new();
        let mut starred = None;
        let mut saw_comma = false;
        while !matches!(self.peek_token()?, Some(token) if *token == close) {
            if sequence && matches!(self.peek_token()?, Some(Token::Star)) {
                let star_start = self.peek_start();
                self.next_token()?; // consume '*'
                if starred.is_some() {
                    return Err(ParseError::UnexpectedToken(
                        Token::Star,
                        "multiple starred expressions in list pattern".to_string(),
                    ));
                }
                let leaf = item_to_target(self.parse_target_item(declare, Precedence::Lowest)?)?;
                starred = Some(items.len());
                items.push(TargetItem::Target(DestructureTarget::new(
                    leaf.kind,
                    (star_start, self.last_significant_end),
                )));
            } else {
                let item = self.parse_target_item(declare, Precedence::Lowest)?;
                // A comprehension keeps the whole display an expression.
                if items.is_empty() && matches!(self.peek_token()?, Some(Token::For)) {
                    let TargetItem::Expr(value) = item else {
                        return Err(ParseError::UnexpectedToken(
                            Token::For,
                            "a comprehension is not a destructuring target".to_string(),
                        ));
                    };
                    if declare.is_some() {
                        return Err(ParseError::UnexpectedToken(
                            Token::For,
                            "a comprehension is not a destructuring target".to_string(),
                        ));
                    }
                    let clauses = self.parse_comprehension_clauses()?;
                    self.expect(&close, "Expected the closing bracket after a comprehension")?;
                    let kind = if sequence {
                        mojito_ast::ast::CollectionKind::List
                    } else {
                        return Err(ParseError::UnexpectedToken(
                            Token::LParen,
                            "generator expressions are not supported".to_string(),
                        ));
                    };
                    return Ok(TargetItem::Expr(self.node(
                        ExprKind::Comprehension {
                            kind,
                            key: None,
                            value: Box::new(value),
                            clauses,
                        },
                        start,
                    )));
                }
                items.push(item);
            }
            if matches!(self.peek_token()?, Some(Token::Comma)) {
                self.next_token()?;
                saw_comma = true;
            } else {
                break;
            }
        }
        self.expect(
            &close,
            if sequence {
                "Expected ']' after list elements"
            } else {
                "Expected ')' after tuple elements"
            },
        )?;
        let span = (start, self.last_significant_end);
        let has_target = items
            .iter()
            .any(|item| matches!(item, TargetItem::Target(_)));
        // `(e)` is grouping; `(target)` is that target.
        if !sequence && !saw_comma && items.len() == 1 && declare.is_none() {
            return Ok(items.pop().expect("one item"));
        }
        if !has_target && declare.is_none() && starred.is_none() {
            let elements = items
                .into_iter()
                .filter_map(|item| match item {
                    TargetItem::Expr(value) => Some(value),
                    TargetItem::Target(_) => None,
                })
                .collect();
            let kind = if sequence {
                ExprKind::ListLit(elements)
            } else {
                ExprKind::TupleLit(elements)
            };
            return Ok(TargetItem::Expr(self.node(kind, start)));
        }
        let targets = items
            .into_iter()
            .map(item_to_target)
            .collect::<Result<Vec<_>, _>>()?;
        let kind = if sequence {
            DestructureTargetKind::Sequence(
                targets
                    .into_iter()
                    .enumerate()
                    .map(|(index, target)| {
                        if starred == Some(index) {
                            SequenceTarget::Star(target)
                        } else {
                            SequenceTarget::Element(target)
                        }
                    })
                    .collect(),
            )
        } else {
            DestructureTargetKind::Tuple(targets)
        };
        Ok(TargetItem::Target(DestructureTarget::new(kind, span)))
    }

    /// One element of a destructuring display or comma list: a `var`/`ref`
    /// leaf, a nested display, or an expression (a name, a place, or any
    /// expression that is then only valid without `=`). Under a declaring
    /// prefix every bare name declares with that convention. A bare item
    /// (and the continuation after a display) parses above `min`, so a loop
    /// target stops before `in`.
    pub(super) fn parse_target_item(
        &mut self,
        declare: Option<BindingMode>,
        min: Precedence,
    ) -> Result<TargetItem, ParseError> {
        let start = self.peek_start();
        match self.peek_token()? {
            Some(Token::Var) => {
                self.next_token()?;
                self.parse_declared_target(BindingMode::Var, start)
            }
            Some(Token::Identifier(word)) if word == "ref" => {
                self.next_token()?;
                self.parse_declared_target(BindingMode::Ref, start)
            }
            Some(Token::LParen | Token::LBracket) => {
                let item = self.parse_target_display(declare)?;
                match item {
                    TargetItem::Expr(value) => {
                        Ok(TargetItem::Expr(self.parse_expression_from(value, min)?))
                    }
                    target @ TargetItem::Target(_) => Ok(target),
                }
            }
            _ => {
                let value = self.parse_expression(min)?;
                match declare {
                    None => Ok(TargetItem::Expr(value)),
                    Some(mode) => Ok(TargetItem::Target(declared_leaf(value, mode)?)),
                }
            }
        }
    }

    /// A `for` statement's or comprehension clause's target, up to `in`:
    /// upstream's `parseTargetListExpr`. A comma list forms a tuple, a star
    /// is valid only inside a `[...]` display, and a bare name declares an
    /// immutable binding (upstream's `Bind` pattern) rather than assigning.
    pub(super) fn parse_loop_target(&mut self) -> Result<DestructureTarget, ParseError> {
        let start = self.peek_start();
        let mut targets = Vec::new();
        loop {
            let item = self.parse_target_item(None, Precedence::Comparison)?;
            targets.push(item_to_target(item)?);
            if !matches!(self.peek_token()?, Some(Token::Comma)) {
                break;
            }
            self.next_token()?;
            if matches!(self.peek_token()?, Some(Token::In)) {
                return Err(ParseError::UnexpectedToken(
                    Token::In,
                    "unexpected token in expression".to_string(),
                ));
            }
        }
        let mut target = if targets.len() == 1 {
            targets.pop().expect("one target")
        } else {
            DestructureTarget::new(
                DestructureTargetKind::Tuple(targets),
                (start, self.last_significant_end),
            )
        };
        bind_bare_names(&mut target);
        Ok(target)
    }

    /// The target after a consumed `var`/`ref`: a name, or a display whose
    /// every leaf name declares with `mode`.
    pub(super) fn parse_declared_target(
        &mut self,
        mode: BindingMode,
        start: usize,
    ) -> Result<TargetItem, ParseError> {
        if matches!(self.peek_token()?, Some(Token::LParen | Token::LBracket)) {
            let item = self.parse_target_display(Some(mode))?;
            let target = item_to_target(item)?;
            return Ok(TargetItem::Target(DestructureTarget::new(
                target.kind,
                (start, self.last_significant_end),
            )));
        }
        let keyword = match mode {
            BindingMode::Var => "var",
            BindingMode::Ref => "ref",
            BindingMode::Immutable => "a binding",
        };
        let name = self.expect_identifier(&format!("expected a name after '{keyword}'"))?;
        let kind = if name == "_" {
            DestructureTargetKind::Discard
        } else {
            DestructureTargetKind::Declare { name, mode }
        };
        Ok(TargetItem::Target(DestructureTarget::new(
            kind,
            (start, self.last_significant_end),
        )))
    }

    /// The statement after a complete destructuring target: `= value`.
    pub(super) fn finish_destructure(
        &mut self,
        target: DestructureTarget,
    ) -> Result<StmtKind, ParseError> {
        self.expect(&Token::Assign, "expected '=' after a destructuring target")?;
        let value = self.parse_tuple_display()?;
        self.expect_stmt_end()?;
        Ok(StmtKind::Destructure { target, value })
    }

    /// The statement after a first destructuring target: further
    /// comma-separated targets form a tuple with it (`var (p, q), r = ...`),
    /// then `= value`.
    pub(super) fn parse_destructure_tail(
        &mut self,
        first: DestructureTarget,
        declare: Option<BindingMode>,
    ) -> Result<StmtKind, ParseError> {
        let start = first.span.0;
        let mut targets = vec![first];
        let mut tuple = false;
        while matches!(self.peek_token()?, Some(Token::Comma)) {
            self.next_token()?;
            tuple = true;
            if matches!(self.peek_token()?, Some(Token::Assign)) {
                break;
            }
            let item = self.parse_target_item(declare, Precedence::Lowest)?;
            targets.push(item_to_target(item)?);
        }
        let target = if tuple {
            DestructureTarget::new(
                DestructureTargetKind::Tuple(targets),
                (start, self.last_significant_end),
            )
        } else {
            targets.pop().expect("the first target")
        };
        self.finish_destructure(target)
    }

    /// `case pattern [if guard]: suite`.
    fn parse_case(&mut self) -> Result<MatchCase, ParseError> {
        let start = self.peek_start();
        self.expect(&Token::Case, "Expected 'case'")?;
        let pattern = self.parse_case_patterns()?;
        let guard = if matches!(self.peek_token()?, Some(Token::If)) {
            self.next_token()?;
            Some(self.parse_expression(Precedence::Lowest)?)
        } else {
            None
        };
        self.expect(&Token::Colon, "expected ':' after case pattern")?;
        let body = self.parse_suite()?;
        Ok(MatchCase {
            pattern,
            guard,
            body,
            span: (start, self.last_significant_end),
        })
    }

    /// A comma-separated pattern list: one pattern, or a tuple of them when
    /// a comma appears (`case 0, y:`, `case 1,:`). Stops before `:`, `if`,
    /// and `=`.
    fn parse_case_patterns(&mut self) -> Result<MatchPattern, ParseError> {
        let start = self.peek_start();
        let first = self.parse_pattern()?;
        if !matches!(self.peek_token()?, Some(Token::Comma)) {
            return Ok(first);
        }
        let mut items = vec![first];
        while matches!(self.peek_token()?, Some(Token::Comma)) {
            self.next_token()?;
            if matches!(
                self.peek_token()?,
                Some(Token::Colon | Token::If | Token::Assign | Token::RParen)
            ) {
                break;
            }
            items.push(self.parse_pattern()?);
        }
        Ok(MatchPattern::new(
            MatchPatternKind::Tuple(items),
            (start, self.last_significant_end),
        ))
    }

    /// `closed ('|' closed)* ('as' NAME)*`: `as` binds looser than `|`, so
    /// `3 | 4 as n` names the whole alternative set.
    fn parse_pattern(&mut self) -> Result<MatchPattern, ParseError> {
        let start = self.peek_start();
        let mut alternatives = vec![self.parse_closed_pattern()?];
        while matches!(self.peek_token()?, Some(Token::Pipe)) {
            self.next_token()?;
            alternatives.push(self.parse_closed_pattern()?);
        }
        let mut pattern = if alternatives.len() == 1 {
            alternatives.pop().expect("one alternative")
        } else {
            MatchPattern::new(
                MatchPatternKind::Or(alternatives),
                (start, self.last_significant_end),
            )
        };
        while matches!(self.peek_token()?, Some(Token::As)) {
            self.next_token()?;
            let name = self.expect_identifier("expected a name after 'as'")?;
            pattern = MatchPattern::new(
                MatchPatternKind::As {
                    pattern: Box::new(pattern),
                    name,
                },
                (start, self.last_significant_end),
            );
        }
        Ok(pattern)
    }

    /// One pattern operand: a binding, `_`, a literal, `== expr`, a
    /// parenthesized or bracketed pattern, `{field=p}`, or a call-shaped or
    /// member head. A signed number is not a pattern: it needs `==`.
    fn parse_closed_pattern(&mut self) -> Result<MatchPattern, ParseError> {
        let start = self.peek_start();
        let kind = match self.peek_token()? {
            Some(Token::Identifier(word)) if word == "_" => {
                self.next_token()?;
                MatchPatternKind::Wildcard
            }
            Some(Token::Var) => {
                self.next_token()?;
                self.parse_binding_pattern(BindingMode::Var)?
            }
            Some(Token::Identifier(word)) if word == "ref" => {
                self.next_token()?;
                self.parse_binding_pattern(BindingMode::Ref)?
            }
            Some(
                Token::IntLiteral(..)
                | Token::FloatLiteral(..)
                | Token::BoolLiteral(_)
                | Token::StringLiteral(..)
                | Token::TripleStringLiteral(..)
                | Token::None,
            ) => {
                let (value, spelling) = self.parse_spelled_operand(Self::parse_prefix)?;
                MatchPatternKind::Literal { value, spelling }
            }
            Some(Token::EqEq) => {
                self.next_token()?;
                let (value, spelling) = self
                    .parse_spelled_operand(|parser| parser.parse_expression(Precedence::Unary))?;
                MatchPatternKind::Equal { value, spelling }
            }
            Some(Token::LParen) => {
                self.next_token()?;
                if matches!(self.peek_token()?, Some(Token::RParen)) {
                    self.next_token()?;
                    MatchPatternKind::Tuple(Vec::new())
                } else {
                    let first = self.parse_pattern()?;
                    if matches!(self.peek_token()?, Some(Token::Comma)) {
                        let mut items = vec![first];
                        while matches!(self.peek_token()?, Some(Token::Comma)) {
                            self.next_token()?;
                            if matches!(self.peek_token()?, Some(Token::RParen)) {
                                break;
                            }
                            items.push(self.parse_pattern()?);
                        }
                        self.expect(&Token::RParen, "Expected ')' after tuple pattern")?;
                        MatchPatternKind::Tuple(items)
                    } else {
                        self.expect(&Token::RParen, "Expected ')' after pattern")?;
                        return Ok(first);
                    }
                }
            }
            Some(Token::LBracket) => {
                self.next_token()?;
                self.parse_sequence_pattern()?
            }
            Some(Token::LBrace) => {
                self.next_token()?;
                self.parse_fields_pattern()?
            }
            Some(Token::Dot) => {
                self.next_token()?;
                let sentinel = self.node(
                    ExprKind::Identifier(mojito_ast::ast::CONTEXTUAL_SENTINEL.into()),
                    start,
                );
                let field = self.expect_case_name("Expected a member name after a leading '.'")?;
                let head = self.node(
                    ExprKind::Member {
                        object: Box::new(sentinel),
                        field,
                    },
                    start,
                );
                self.parse_head_pattern(head, start)?
            }
            Some(Token::Identifier(_)) => {
                let name = self.expect_identifier("Expected a name")?;
                if matches!(
                    self.peek_token()?,
                    Some(Token::Dot | Token::LParen | Token::LBracket)
                ) {
                    let head = self.node(ExprKind::Identifier(name), start);
                    self.parse_head_pattern(head, start)?
                } else {
                    MatchPatternKind::Bind {
                        name,
                        mode: BindingMode::Immutable,
                    }
                }
            }
            _ => {
                let token = self.next_token()?;
                return Err(ParseError::UnexpectedToken(
                    token,
                    "expression is not a valid match pattern; use '==' to compare against its value"
                        .to_string(),
                ));
            }
        };
        Ok(MatchPattern::new(kind, (start, self.last_significant_end)))
    }

    /// A literal or `==` operand parsed by `parse`, with its source spelling
    /// when it is exactly one literal token (`0x4`, `'a'`, `True`), as
    /// upstream's literal node keeps it; `(4)`, `-3`, and an implicitly
    /// concatenated string have none.
    fn parse_spelled_operand(
        &mut self,
        parse: impl FnOnce(&mut Self) -> Result<Expr, ParseError>,
    ) -> Result<(Expr, Option<String>), ParseError> {
        let spelling = match self.peek_token()? {
            Some(
                Token::IntLiteral(_, spelling)
                | Token::FloatLiteral(_, spelling)
                | Token::StringLiteral(_, spelling)
                | Token::TripleStringLiteral(_, spelling),
            ) => Some(spelling.to_string()),
            Some(Token::BoolLiteral(value)) => Some(if *value { "True" } else { "False" }.into()),
            _ => None,
        };
        let value = parse(self)?;
        let spelling = spelling.filter(|spelling| value.span.1 - value.span.0 == spelling.len());
        Ok((value, spelling))
    }

    /// A member name in a pattern head: a member name, or `None`, which
    /// lexes as a literal but names `Optional`'s payload-less case.
    fn expect_case_name(&mut self, context: &str) -> Result<String, ParseError> {
        if matches!(self.peek_token()?, Some(Token::None)) {
            self.next_token()?;
            return Ok("None".to_string());
        }
        self.expect_member_name(context)
    }

    /// The name after a consumed `var`/`ref`.
    fn parse_binding_pattern(&mut self, mode: BindingMode) -> Result<MatchPatternKind, ParseError> {
        if matches!(self.peek_token()?, Some(Token::Var))
            || matches!(self.peek_token()?, Some(Token::Identifier(word)) if word == "ref")
        {
            return Err(ParseError::UnexpectedToken(
                self.next_token()?,
                "nested 'var' or 'ref' patterns are redundant, remove the outer pattern"
                    .to_string(),
            ));
        }
        let keyword = match mode {
            BindingMode::Var => "var",
            _ => "ref",
        };
        let name = self.expect_identifier(&format!("expected a name after '{keyword}'"))?;
        Ok(if name == "_" {
            MatchPatternKind::Wildcard
        } else {
            MatchPatternKind::Bind { name, mode }
        })
    }

    /// `[p, *rest, q]` after the consumed `[`.
    fn parse_sequence_pattern(&mut self) -> Result<MatchPatternKind, ParseError> {
        let mut items = Vec::new();
        let mut starred = false;
        while !matches!(self.peek_token()?, Some(Token::RBracket)) {
            if matches!(self.peek_token()?, Some(Token::Star)) {
                self.next_token()?;
                if starred {
                    return Err(ParseError::UnexpectedToken(
                        Token::Star,
                        "multiple starred expressions in list pattern".to_string(),
                    ));
                }
                starred = true;
                items.push(SequencePattern::Star(Box::new(
                    self.parse_closed_pattern()?,
                )));
            } else {
                items.push(SequencePattern::Element(Box::new(self.parse_pattern()?)));
            }
            if matches!(self.peek_token()?, Some(Token::Comma)) {
                self.next_token()?;
            } else {
                break;
            }
        }
        self.expect(&Token::RBracket, "Expected ']' after list pattern")?;
        Ok(MatchPatternKind::Sequence(items))
    }

    /// `{name=p, ...}` after the consumed `{`.
    fn parse_fields_pattern(&mut self) -> Result<MatchPatternKind, ParseError> {
        let mut fields = Vec::new();
        while !matches!(self.peek_token()?, Some(Token::RBrace)) {
            let name = self.expect_identifier("expected a field name in a struct pattern")?;
            self.expect(&Token::Assign, "expected '=' after the field name")?;
            fields.push((name, self.parse_pattern()?));
            if matches!(self.peek_token()?, Some(Token::Comma)) {
                self.next_token()?;
            } else {
                break;
            }
        }
        self.expect(&Token::RBrace, "Expected '}' after struct pattern fields")?;
        Ok(MatchPatternKind::Fields(fields))
    }

    /// The rest of a member/call-shaped pattern after its first head
    /// segment: `.name` segments, an optional parameter list on a bare
    /// name and the `.name` segments after it, then an optional `(args)` of
    /// subpatterns.
    fn parse_head_pattern(
        &mut self,
        mut head: Expr,
        start: usize,
    ) -> Result<MatchPatternKind, ParseError> {
        while matches!(self.peek_token()?, Some(Token::Dot)) {
            self.next_token()?;
            let field = self.expect_case_name("Expected a member name after '.'")?;
            head = self.node(
                ExprKind::Member {
                    object: Box::new(head),
                    field,
                },
                start,
            );
        }
        if matches!(self.peek_token()?, Some(Token::LBracket)) {
            let ExprKind::Identifier(name) = &head.kind else {
                return Err(ParseError::UnexpectedToken(
                    Token::LBracket,
                    "a parameterized pattern head must be a bare struct name".to_string(),
                ));
            };
            let name = name.clone();
            let args = self.parse_param_args()?;
            head = self.node(ExprKind::TypeApply { name, args }, start);
            while matches!(self.peek_token()?, Some(Token::Dot)) {
                self.next_token()?;
                let field = self.expect_case_name("Expected a member name after '.'")?;
                head = self.node(
                    ExprKind::Member {
                        object: Box::new(head),
                        field,
                    },
                    start,
                );
            }
        }
        if !matches!(self.peek_token()?, Some(Token::LParen)) {
            if matches!(
                head.kind,
                ExprKind::Identifier(_) | ExprKind::TypeApply { .. }
            ) {
                return Err(ParseError::UnexpectedToken(
                    self.next_token()?,
                    "expected '(' after a struct pattern head".to_string(),
                ));
            }
            return Ok(MatchPatternKind::Head(head));
        }
        self.next_token()?; // consume '('
        let mut args = Vec::new();
        let mut kwargs = Vec::new();
        while !matches!(self.peek_token()?, Some(Token::RParen)) {
            if matches!(self.peek_token()?, Some(Token::Star | Token::DoubleStar)) {
                return Err(ParseError::UnexpectedToken(
                    self.next_token()?,
                    "struct patterns do not support positional or unpacked arguments".to_string(),
                ));
            }
            let pattern = self.parse_pattern()?;
            if let MatchPatternKind::Bind {
                name,
                mode: BindingMode::Immutable,
            } = &pattern.kind
                && matches!(self.peek_token()?, Some(Token::Assign))
            {
                self.next_token()?;
                let name = name.clone();
                if kwargs.iter().any(|(existing, _)| *existing == name) {
                    return Err(ParseError::UnexpectedToken(
                        Token::Identifier(name.clone()),
                        format!("keyword argument '{name}' was already used; remove the duplicate"),
                    ));
                }
                kwargs.push((name, self.parse_pattern()?));
            } else {
                if !kwargs.is_empty() {
                    return Err(ParseError::UnexpectedToken(
                        Token::Comma,
                        "a positional pattern cannot follow a keyword pattern".to_string(),
                    ));
                }
                args.push(pattern);
            }
            if matches!(self.peek_token()?, Some(Token::Comma)) {
                self.next_token()?;
            } else {
                break;
            }
        }
        self.expect(&Token::RParen, "Expected ')' after pattern arguments")?;
        Ok(MatchPatternKind::Call { head, args, kwargs })
    }
}

/// Resolve a display element to a target: an expression becomes a place, a
/// discard, or a nested tuple/sequence of them.
pub(super) fn item_to_target(item: TargetItem) -> Result<DestructureTarget, ParseError> {
    match item {
        TargetItem::Target(target) => Ok(target),
        TargetItem::Expr(value) => expr_to_target(value),
    }
}

/// A loop target's bare names: each written name becomes an immutable
/// declaration, through tuple and sequence items alike.
fn bind_bare_names(target: &mut DestructureTarget) {
    match &mut target.kind {
        DestructureTargetKind::Place(Expr {
            kind: ExprKind::Identifier(name),
            ..
        }) => {
            target.kind = DestructureTargetKind::Declare {
                name: std::mem::take(name),
                mode: BindingMode::Immutable,
            };
        }
        DestructureTargetKind::Tuple(items) => items.iter_mut().for_each(bind_bare_names),
        DestructureTargetKind::Sequence(items) => {
            for item in items {
                match item {
                    SequenceTarget::Element(target) | SequenceTarget::Star(target) => {
                        bind_bare_names(target);
                    }
                }
            }
        }
        DestructureTargetKind::Place(_)
        | DestructureTargetKind::Discard
        | DestructureTargetKind::Declare { .. } => {}
    }
}

/// A display element parsed under a `var`/`ref` prefix: a bare name
/// declares, `_` discards, anything else is not a declaration.
fn declared_leaf(value: Expr, mode: BindingMode) -> Result<DestructureTarget, ParseError> {
    let span = value.span;
    match value.kind {
        ExprKind::Identifier(name) if name == "_" => {
            Ok(DestructureTarget::new(DestructureTargetKind::Discard, span))
        }
        ExprKind::Identifier(name) => Ok(DestructureTarget::new(
            DestructureTargetKind::Declare { name, mode },
            span,
        )),
        other => Err(ParseError::UnexpectedToken(
            Token::Assign,
            format!("expected a name in a declaring destructuring target, got {other:?}"),
        )),
    }
}

/// An expression in target position: `_` discards, a name or place is
/// written, a tuple or list display nests.
pub(super) fn expr_to_target(value: Expr) -> Result<DestructureTarget, ParseError> {
    let span = value.span;
    if matches!(&value.kind, ExprKind::Identifier(name) if name == "_") {
        return Ok(DestructureTarget::new(DestructureTargetKind::Discard, span));
    }
    if matches!(
        value.kind,
        ExprKind::Identifier(_)
            | ExprKind::Member { .. }
            | ExprKind::Index { .. }
            | ExprKind::Slice { .. }
            | ExprKind::MultiIndex { .. }
            | ExprKind::TypeApply { .. }
            | ExprKind::Call { .. }
            | ExprKind::MethodCall { .. }
    ) {
        return Ok(DestructureTarget::new(
            DestructureTargetKind::Place(value),
            span,
        ));
    }
    let kind = match value.kind {
        ExprKind::TupleLit(items) => DestructureTargetKind::Tuple(
            items
                .into_iter()
                .map(expr_to_target)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        ExprKind::ListLit(items) => DestructureTargetKind::Sequence(
            items
                .into_iter()
                .map(|item| expr_to_target(item).map(SequenceTarget::Element))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        other => {
            return Err(ParseError::UnexpectedToken(
                Token::Assign,
                format!("invalid assignment target: {other:?}"),
            ));
        }
    };
    Ok(DestructureTarget::new(kind, span))
}
