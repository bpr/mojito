//! Statement parsing: assignments, declarations, comptime blocks,
//! decorators, defs, imports, with/try, and control flow.

use super::*;

impl<I: Iterator<Item = Result<(Token, Span), LexError>>> Parser<I> {
    /// Consume one raw lexer item even when it is an error, guaranteeing forward
    /// progress during recovery.
    pub(super) fn discard_one(&mut self) {
        if let Some(Ok((token, span))) = self.tokens.next() {
            self.last_span = span;
            if !matches!(
                token,
                Token::Newline | Token::Indent | Token::Dedent | Token::Eof
            ) {
                self.last_significant_end = span.1;
            }
        }
    }

    /// Panic-mode synchronization. Newlines and layout boundaries are reliable
    /// because the lexer suppresses newlines while delimiters remain open.
    pub(super) fn synchronize_statement(&mut self) {
        loop {
            match self.tokens.next() {
                Some(Ok((token, span))) => {
                    self.last_span = span;
                    if matches!(
                        token,
                        Token::Newline | Token::Semicolon | Token::Dedent | Token::Eof
                    ) {
                        break;
                    }
                }
                Some(Err(_)) => continue,
                None => break,
            }
        }
    }

    // --- Statements ---

    /// Parses a single statement.
    pub(super) fn parse_statement(&mut self) -> Result<Stmt, ParseError> {
        // A statement spans from its first token to the last token consumed. Each
        // sub-parser returns a bare `StmtKind`; the span is stamped once, here.
        let start = self.peek_start();
        let kind = (|| -> Result<StmtKind, ParseError> {
            Ok(match self.peek_token()? {
                Some(Token::Var) => self.parse_var_decl()?,
                Some(Token::Identifier(word)) if word == "ref" => self.parse_ref_decl()?,
                Some(Token::Def) => self.parse_def(Vec::new())?,
                Some(Token::Struct) => self.parse_struct(Vec::new())?,
                // A decorator list precedes a `def` or `struct`.
                Some(Token::At) => {
                    let decorators = self.parse_decorators()?;
                    match self.peek_token()? {
                        Some(Token::Def) => self.parse_def(decorators)?,
                        Some(Token::Struct) => self.parse_struct(decorators)?,
                        other => {
                            return Err(ParseError::UnexpectedToken(
                                other.cloned().unwrap_or(Token::Eof),
                                "a decorator must precede a 'def' or 'struct'".into(),
                            ));
                        }
                    }
                }
                Some(Token::Trait) => self.parse_trait()?,
                Some(Token::Comptime) => self.parse_comptime()?,
                Some(Token::If) => self.parse_if()?,
                Some(Token::While) => self.parse_while()?,
                Some(Token::For) => self.parse_for()?,
                Some(Token::With) => self.parse_with()?,
                Some(Token::Try) => self.parse_try()?,
                Some(Token::Return) => self.parse_return()?,
                Some(Token::Raise) => self.parse_raise()?,
                Some(Token::Import) => self.parse_import()?,
                Some(Token::From) => self.parse_from_import()?,
                Some(Token::Pass) => {
                    self.next_token()?;
                    self.expect_stmt_end()?;
                    StmtKind::Pass
                }
                Some(Token::Break) => {
                    self.next_token()?;
                    self.expect_stmt_end()?;
                    StmtKind::Break
                }
                Some(Token::Continue) => {
                    self.next_token()?;
                    self.expect_stmt_end()?;
                    StmtKind::Continue
                }
                Some(Token::Ellipsis) => {
                    self.next_token()?;
                    self.expect_stmt_end()?;
                    StmtKind::Pass
                }
                // Mojo docstrings are triple-quoted string statements placed at
                // the start of a module/declaration body. Documentation metadata
                // is not retained yet, but it has no runtime effect.
                Some(Token::TripleStringLiteral(_)) => {
                    self.next_token()?;
                    self.expect_stmt_end()?;
                    StmtKind::Pass
                }
                _ => self.parse_expr_or_assign()?,
            })
        })()
        .map_err(|err| err.at(self.last_span))?;
        // End at the last significant token so the trailing newline (consumed by
        // `expect_stmt_end`) isn't included in the statement's span.
        Ok(Stmt::new(kind, (start, self.last_significant_end)))
    }

    /// A bare expression statement, or an assignment `target = value`. The two
    /// share a leading expression, so we parse that first and then look for `=`.
    /// A target is a variable (`x = e`) or a **place** — a field/index chain
    /// rooted at a variable (`p.x = e`, `xs[i] = e`, `p.items[i].x = e`). A
    /// top-level comma after the first target starts a **tuple unpacking**
    /// (`a, b = t`).
    pub(super) fn parse_expr_or_assign(&mut self) -> Result<StmtKind, ParseError> {
        let expr = self.parse_expression(Precedence::Lowest)?;

        // Tuple-unpacking target list: `a, b, … = value`. A top-level comma
        // (the Pratt parser never consumes one) after the first target starts an
        // unpack; collect the remaining comma-separated targets, then require `=`.
        if matches!(self.peek_token()?, Some(Token::Comma)) {
            let mut targets = vec![expr];
            while matches!(self.peek_token()?, Some(Token::Comma)) {
                // A trailing comma before `=` is allowed (`a, = t`, `a, b, = t`).
                // Without a following `=`, the same comma syntax is a bare tuple
                // display (`a, b`), because the comma creates the tuple.
                self.next_token()?; // consume ','
                if matches!(
                    self.peek_token()?,
                    Some(Token::Assign | Token::Newline | Token::Semicolon | Token::Eof) | None
                ) {
                    break;
                }
                targets.push(self.parse_expression(Precedence::Lowest)?);
            }
            if !matches!(self.peek_token()?, Some(Token::Assign)) {
                let start = targets[0].span.0;
                let display = self.node(ExprKind::TupleLit(targets), start);
                self.expect_stmt_end()?;
                return Ok(StmtKind::Expr(display));
            }
            for target in &targets {
                if !matches!(
                    target.kind,
                    ExprKind::Identifier(_) | ExprKind::Member { .. } | ExprKind::Index { .. }
                ) {
                    return Err(ParseError::UnexpectedToken(
                        Token::Comma,
                        format!("invalid unpacking target: {:?}", target.kind),
                    ));
                }
            }
            self.expect(
                Token::Assign,
                "Expected '=' after the unpacking target list",
            )?;
            let value = self.parse_tuple_display()?;
            self.expect_stmt_end()?;
            return Ok(StmtKind::Unpack {
                targets,
                value,
                declares: false,
            });
        }

        if matches!(self.peek_token()?, Some(Token::Assign)) {
            self.next_token()?; // consume '='
            let value = self.parse_tuple_display()?;
            let stmt = if let ExprKind::Identifier(name) = expr.kind {
                StmtKind::Assign { name, value }
            } else if matches!(
                expr.kind,
                ExprKind::Member { .. }
                    | ExprKind::Index { .. }
                    | ExprKind::Slice { .. }
                    | ExprKind::MultiIndex { .. }
                    | ExprKind::TypeApply { .. }
            ) {
                // A field/index chain — the checker verifies its root is a
                // mutable variable (or `mut self`) and that the write is valid.
                StmtKind::SetPlace { place: expr, value }
            } else {
                return Err(ParseError::UnexpectedToken(
                    Token::Assign,
                    format!("invalid assignment target: {:?}", expr.kind),
                ));
            };
            self.expect_stmt_end()?;
            return Ok(stmt);
        }

        // Augmented assignment `target OP= value` (target is a NAME or place).
        if let Some(op) = self.peek_token()?.and_then(aug_assign_op) {
            self.next_token()?; // consume the `OP=` token
            if !matches!(
                expr.kind,
                ExprKind::Identifier(_)
                    | ExprKind::Member { .. }
                    | ExprKind::Index { .. }
                    | ExprKind::Slice { .. }
                    | ExprKind::MultiIndex { .. }
                    | ExprKind::TypeApply { .. }
            ) {
                return Err(ParseError::UnexpectedToken(
                    Token::Assign,
                    format!("invalid augmented-assignment target: {:?}", expr.kind),
                ));
            }
            let value = self.parse_expression(Precedence::Lowest)?;
            self.expect_stmt_end()?;
            return Ok(StmtKind::AugAssign {
                place: expr,
                op,
                value,
            });
        }

        self.expect_stmt_end()?;
        Ok(StmtKind::Expr(expr))
    }

    /// `var name[: Type] = value` — the annotation is optional (inferred `var`).
    pub(super) fn parse_var_decl(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(Token::Var, "Statements must begin with a keyword")?;
        let name = self.expect_identifier("Expected identifier after 'var'")?;
        if matches!(self.peek_token()?, Some(Token::Comma)) {
            let mut targets = vec![Expr::new(ExprKind::Identifier(name), self.last_span)];
            while matches!(self.peek_token()?, Some(Token::Comma)) {
                self.next_token()?;
                let target = self.expect_identifier("Expected another variable name")?;
                targets.push(Expr::new(ExprKind::Identifier(target), self.last_span));
            }
            self.expect(
                Token::Assign,
                "Expected '=' after variable unpacking targets",
            )?;
            let value = self.parse_tuple_display()?;
            self.expect_stmt_end()?;
            return Ok(StmtKind::Unpack {
                targets,
                value,
                declares: true,
            });
        }
        // An optional `: Type`; omitting it infers the type from `value`.
        let ty = if matches!(self.peek_token()?, Some(Token::Colon)) {
            self.next_token()?; // consume ':'
            Some(self.parse_type()?)
        } else {
            None
        };
        let value = if matches!(self.peek_token()?, Some(Token::Assign)) {
            self.next_token()?;
            self.parse_tuple_display()?
        } else {
            Expr::new(ExprKind::Uninitialized, self.last_span)
        };
        self.expect_stmt_end()?;
        Ok(StmtKind::VarDecl { name, ty, value })
    }

    /// Parse an expression in a statement-RHS position, where a top-level comma
    /// forms a tuple without requiring parentheses (`var pair = 1, "one"`).
    /// Delimited expression lists (call arguments, list elements, and so on) keep
    /// using `parse_expression` so their commas remain delimiters.
    pub(super) fn parse_tuple_display(&mut self) -> Result<Expr, ParseError> {
        let first = self.parse_expression(Precedence::Lowest)?;
        if !matches!(self.peek_token()?, Some(Token::Comma)) {
            return Ok(first);
        }
        let start = first.span.0;
        let mut elements = vec![first];
        while matches!(self.peek_token()?, Some(Token::Comma)) {
            self.next_token()?;
            if matches!(
                self.peek_token()?,
                Some(Token::Newline | Token::Semicolon | Token::Eof) | None
            ) {
                break;
            }
            elements.push(self.parse_expression(Precedence::Lowest)?);
        }
        Ok(self.node(ExprKind::TupleLit(elements), start))
    }

    /// `ref name = expression` — Mojo's explicit reference binding. The AST
    /// preserves the distinction from an owned `var` so later phases cannot
    /// accidentally give it copy semantics.
    pub(super) fn parse_ref_decl(&mut self) -> Result<StmtKind, ParseError> {
        let keyword = self.expect_identifier("Expected 'ref'")?;
        debug_assert_eq!(keyword, "ref");
        let name = self.expect_identifier("Expected a name after 'ref'")?;
        self.expect(Token::Assign, "Expected '=' after the reference name")?;
        let value = self.parse_expression(Precedence::Lowest)?;
        self.expect_stmt_end()?;
        Ok(StmtKind::RefDecl { name, value })
    }

    /// `comptime NAME[: Type] = value` — a compile-time constant.
    /// `comptime`, which introduces one of three forms: a compile-time constant
    /// `comptime NAME[: Type] = expr`, a compile-time conditional `comptime if …`,
    /// or a compile-time (unrolled) loop `comptime for …` (Mojo's modern spellings
    /// — the older `@parameter if`/`@parameter for` are deprecated).
    pub(super) fn parse_comptime(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(Token::Comptime, "Expected 'comptime'")?;
        match self.peek_token()? {
            Some(Token::If) => {
                let (branches, orelse) = self.parse_if_rest()?;
                Ok(StmtKind::ComptimeIf { branches, orelse })
            }
            Some(Token::For) => {
                let (var, binding, iter, body) = self.parse_for_rest()?;
                if binding != LoopBindingMode::Immutable {
                    return Err(ParseError::UnexpectedToken(
                        Token::For,
                        "comptime for cannot use an explicit ref/var binding".to_string(),
                    ));
                }
                Ok(StmtKind::ComptimeFor { var, iter, body })
            }
            _ => {
                let name =
                    self.expect_identifier("Expected a name, 'if', or 'for' after 'comptime'")?;
                // Directive form, e.g. `comptime assert(condition), message`.
                if name == "assert" {
                    let mut args = if matches!(self.peek_token()?, Some(Token::LParen)) {
                        self.next_token()?;
                        let (args, _) = self.parse_call_args()?;
                        self.expect(Token::RParen, "Expected ')' after comptime directive")?;
                        args
                    } else {
                        vec![self.parse_expression(Precedence::Lowest)?]
                    };
                    if matches!(self.peek_token()?, Some(Token::Comma)) {
                        self.next_token()?;
                        args.push(self.parse_expression(Precedence::Lowest)?);
                    }
                    self.expect_stmt_end()?;
                    return Ok(StmtKind::Expr(Expr::new(
                        ExprKind::Call {
                            name,
                            param_args: Vec::new(),
                            args,
                            kwargs: Vec::new(),
                        },
                        self.last_span,
                    )));
                }
                let type_params = if matches!(self.peek_token()?, Some(Token::LBracket)) {
                    self.parse_type_params()?
                } else {
                    Vec::new()
                };
                // Mojo permits an optional annotation (`comptime N: Int = 1`).
                // Retain it with the declaration so checked constraints and future
                // generic-alias expansion never need to reconstruct source syntax.
                let ty = if matches!(self.peek_token()?, Some(Token::Colon)) {
                    self.next_token()?; // consume ':'
                    Some(self.parse_type()?)
                } else {
                    None
                };
                let where_clauses = self.parse_where_clauses()?;
                self.expect(
                    Token::Assign,
                    "Expected '=' after the comptime constant name (or its ': Type')",
                )?;
                let value = self.parse_expression(Precedence::Lowest)?;
                self.expect_stmt_end()?;
                Ok(StmtKind::Comptime {
                    name,
                    type_params,
                    ty,
                    where_clauses,
                    value,
                })
            }
        }
    }

    /// `def name(params) -> ret: <block>`
    /// Parses one or more decorators, each on its own line: `@` followed by a
    /// dotted name and optional call arguments. A general grammar — any name is
    /// accepted (only `@fieldwise_init` on a struct is acted on).
    pub(super) fn parse_decorators(&mut self) -> Result<Vec<Decorator>, ParseError> {
        let mut decorators = Vec::new();
        while matches!(self.peek_token()?, Some(Token::At)) {
            self.next_token()?; // consume '@'
            let mut path = vec![self.expect_identifier("Expected a decorator name after '@'")?];
            while matches!(self.peek_token()?, Some(Token::Dot)) {
                self.next_token()?; // consume '.'
                path.push(self.expect_identifier("Expected a name after '.' in a decorator")?);
            }
            let (args, kwargs) = if matches!(self.peek_token()?, Some(Token::LParen)) {
                self.next_token()?; // consume '('
                let call = self.parse_call_args()?;
                self.expect(Token::RParen, "Expected ')' after decorator arguments")?;
                call
            } else {
                (Vec::new(), Vec::new())
            };
            self.expect_stmt_end()?;
            decorators.push(Decorator { path, args, kwargs });
        }
        Ok(decorators)
    }

    pub(super) fn parse_def(&mut self, decorators: Vec<Decorator>) -> Result<StmtKind, ParseError> {
        self.expect(Token::Def, "Expected 'def'")?;
        let name = self.expect_identifier("Expected function name after 'def'")?;
        let type_params = self.parse_type_params()?;

        self.expect(Token::LParen, "Expected '(' after function name")?;
        let ParamList {
            params,
            positional_only,
            keyword_only,
        } = self.parse_params()?;
        self.expect(Token::RParen, "Expected ')' after parameters")?;

        // Current Mojo removed the `unified` keyword; the capture list is a
        // bare `{...}` after the effects clause.
        if matches!(self.peek_token()?, Some(Token::Identifier(word)) if word == "unified") {
            return Err(ParseError::UnexpectedToken(
                Token::Identifier("unified".to_string()),
                "the removed 'unified {...}' capture spelling is not accepted; \
                 write the capture list after the effects clause, e.g. 'def f() {mut x}:'"
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

        if matches!(self.peek_token()?, Some(Token::LBrace)) {
            self.next_token()?;
            while !matches!(self.peek_token()?, Some(Token::RBrace) | None) {
                self.next_token()?;
            }
            self.expect(Token::RBrace, "Expected '}' after function effects")?;
        }

        let where_clauses = self.parse_where_clauses()?;

        self.expect(Token::Colon, "Expected ':' before the function body")?;
        let body = self.parse_suite()?;

        Ok(StmtKind::Def {
            name,
            decorators,
            type_params,
            params,
            positional_only,
            keyword_only,
            captures,
            raises,
            raises_type,
            ret,
            where_clauses,
            body,
        })
    }

    /// Parse the trailing declaration constraints, one per `where` clause; each
    /// clause is retained independently. `where` remains contextual rather than
    /// becoming a lexer keyword so future syntax can continue to use the
    /// identifier in ordinary positions.
    pub(super) fn parse_where_clauses(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut clauses = Vec::new();
        while matches!(self.peek_token()?, Some(Token::Identifier(word)) if word == "where") {
            self.next_token()?;
            clauses.push(self.parse_expression(Precedence::Lowest)?);
        }
        Ok(clauses)
    }

    /// Parse a closure capture list, spelled as a bare `{...}` after effects.
    pub(super) fn parse_capture_list(&mut self) -> Result<Option<CaptureList>, ParseError> {
        if matches!(self.peek_token()?, Some(Token::LBrace)) {
            self.next_token()?;
        } else {
            return Ok(None);
        }
        let mut entries = Vec::new();
        let mut default = None;
        while !matches!(self.peek_token()?, Some(Token::RBrace)) {
            let convention = match self.peek_token()? {
                Some(Token::Var) => {
                    self.next_token()?;
                    Some(CaptureKind::Copy)
                }
                Some(Token::Identifier(word)) if word == "imm" => {
                    self.next_token()?;
                    Some(CaptureKind::Imm)
                }
                Some(Token::Identifier(word)) if word == "read" => {
                    let word = word.clone();
                    return Err(removed_convention_error(&word).expect("read is removed"));
                }
                Some(Token::Identifier(word)) if word == "mut" => {
                    self.next_token()?;
                    Some(CaptureKind::Mut)
                }
                Some(Token::Identifier(word)) if word == "ref" => {
                    self.next_token()?;
                    Some(CaptureKind::Ref)
                }
                _ => None,
            };
            let has_name = matches!(self.peek_token()?, Some(Token::Identifier(_)));
            let name = if has_name {
                Some(self.expect_identifier("Expected a captured name")?)
            } else if convention.is_none() {
                return Err(ParseError::UnexpectedToken(
                    self.next_token()?,
                    "Expected a capture convention or captured name".to_string(),
                ));
            } else {
                None
            };
            let moved = matches!(self.peek_token()?, Some(Token::Caret));
            if moved {
                self.next_token()?;
            }
            if moved && !matches!(convention, None | Some(CaptureKind::Copy)) {
                return Err(ParseError::UnexpectedToken(
                    Token::Caret,
                    "'^' requires the 'var' capture convention".to_string(),
                ));
            }
            let kind = if moved {
                CaptureKind::Move
            } else {
                convention.unwrap_or(CaptureKind::Imm)
            };
            if let Some(name) = name {
                if entries.iter().any(|capture: &Capture| capture.name == name) {
                    return Err(ParseError::UnexpectedToken(
                        Token::Identifier(name.clone()),
                        format!("duplicate capture '{name}'"),
                    ));
                }
                entries.push(Capture { name, kind });
            } else {
                if default.replace(kind).is_some() {
                    return Err(ParseError::UnexpectedToken(
                        Token::Identifier("capture convention".to_string()),
                        "default capture convention was already specified".to_string(),
                    ));
                }
            }
            if matches!(self.peek_token()?, Some(Token::Comma)) {
                self.next_token()?;
            } else if !matches!(self.peek_token()?, Some(Token::RBrace)) {
                return Err(ParseError::UnexpectedToken(
                    self.next_token()?,
                    "Expected ',' or '}' in capture list".to_string(),
                ));
            }
        }
        self.next_token()?;
        Ok(Some(CaptureList { entries, default }))
    }

    /// Parses an optional `raises` effect after a function's parameter list. An
    /// error type may follow (`raises ValidationError`).
    pub(super) fn parse_raises_effect(&mut self) -> Result<(bool, Option<Type>), ParseError> {
        if !matches!(self.peek_token()?, Some(Token::Raises)) {
            return Ok((false, None));
        }
        // An optional error type follows, unless the next token ends the header
        // (a contextual `where` starts the declaration's constraint clauses,
        // never an error type).
        self.next_token()?; // consume 'raises'
        let next_is_effect = matches!(
            self.peek_token()?,
            Some(Token::Identifier(word))
                if matches!(word.as_str(), "capturing" | "thin" | "abi" | "where")
        );
        let error = if !next_is_effect
            && !matches!(
                self.peek_token()?,
                Some(Token::Arrow | Token::Colon | Token::LBrace)
            ) {
            Some(self.parse_type()?)
        } else {
            None
        };
        Ok((true, error))
    }

    /// Parse declaration effects in their source order. Current Mojo permits
    /// `capturing raises`, `raises capturing`, and the other ABI-only effects in
    /// one sequence; only the raising contract survives in the checked AST.
    pub(super) fn parse_callable_effects(&mut self) -> Result<(bool, Option<Type>), ParseError> {
        let mut raises = false;
        let mut raises_type = None;
        loop {
            match self.peek_token()? {
                Some(Token::Raises) if !raises => {
                    let (present, error) = self.parse_raises_effect()?;
                    raises = present;
                    raises_type = error;
                }
                Some(Token::Identifier(effect))
                    if matches!(effect.as_str(), "capturing" | "thin" | "abi") =>
                {
                    self.parse_erased_callable_effects()?;
                }
                _ => break,
            }
        }
        Ok((raises, raises_type))
    }

    /// `raise expr`
    pub(super) fn parse_raise(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(Token::Raise, "Expected 'raise'")?;
        let value = self.parse_expression(Precedence::Lowest)?;
        self.expect_stmt_end()?;
        Ok(StmtKind::Raise(value))
    }

    /// `import a.b.c [as alias]`
    pub(super) fn parse_import(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(Token::Import, "Expected 'import'")?;
        let path = self.parse_dotted_name()?;
        let alias = self.parse_import_alias()?;
        self.expect_stmt_end()?;
        Ok(StmtKind::Import { path, alias })
    }

    /// `from [.]*module import <targets>`
    pub(super) fn parse_from_import(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(Token::From, "Expected 'from'")?;
        // Leading dots make the import relative. The lexer tokenizes `...` as one
        // ellipsis, which here counts as three dots.
        let mut level = 0usize;
        loop {
            match self.peek_token()? {
                Some(Token::Dot) => {
                    self.next_token()?;
                    level += 1;
                }
                Some(Token::Ellipsis) => {
                    self.next_token()?;
                    level += 3;
                }
                _ => break,
            }
        }
        // The module path is optional for a dots-only relative import (`from .`).
        let path = if matches!(self.peek_token()?, Some(Token::Identifier(_))) {
            self.parse_dotted_name()?
        } else {
            Vec::new()
        };
        if level == 0 && path.is_empty() {
            return Err(ParseError::UnexpectedToken(
                Token::From,
                "expected a module name after 'from'".into(),
            ));
        }
        self.expect(Token::Import, "Expected 'import' after the module name")?;

        let parenthesized = matches!(self.peek_token()?, Some(Token::LParen));
        if parenthesized {
            self.next_token()?;
        }
        let names = if matches!(self.peek_token()?, Some(Token::Star)) {
            self.next_token()?; // consume '*'
            crate::ast::ImportNames::Wildcard
        } else {
            let mut targets = Vec::new();
            while !parenthesized || !matches!(self.peek_token()?, Some(Token::RParen)) {
                let name = self.expect_identifier("Expected an imported name")?;
                let alias = self.parse_import_alias()?;
                targets.push(crate::ast::ImportName { name, alias });
                if matches!(self.peek_token()?, Some(Token::Comma)) {
                    self.next_token()?; // consume ','
                } else {
                    break;
                }
            }
            crate::ast::ImportNames::Names(targets)
        };
        if parenthesized {
            self.expect(Token::RParen, "Expected ')' after imported names")?;
        }
        self.expect_stmt_end()?;
        Ok(StmtKind::FromImport { level, path, names })
    }

    /// Parses a dotted module name `NAME ('.' NAME)*` into its segments.
    pub(super) fn parse_dotted_name(&mut self) -> Result<Vec<String>, ParseError> {
        let mut segments = vec![self.expect_identifier("Expected a module name")?];
        while matches!(self.peek_token()?, Some(Token::Dot)) {
            self.next_token()?; // consume '.'
            segments.push(self.expect_identifier("Expected a name after '.'")?);
        }
        Ok(segments)
    }

    /// Parses an optional `as NAME` alias.
    pub(super) fn parse_import_alias(&mut self) -> Result<Option<String>, ParseError> {
        if matches!(self.peek_token()?, Some(Token::As)) {
            self.next_token()?; // consume 'as'
            Ok(Some(
                self.expect_identifier("Expected an alias name after 'as'")?,
            ))
        } else {
            Ok(None)
        }
    }

    /// `try: <block> [except [NAME]: <block>] [else: <block>] [finally: <block>]`
    /// `with item (',' item)*: <block>`, where each `item` is
    /// `expression ['as' NAME]`. Multiple comma-separated managers are allowed;
    /// the `as` binding is optional. The parenthesized / tuple-target forms aren't
    /// in the Mojo docs, so they aren't parsed (strict-subset).
    pub(super) fn parse_with(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(Token::With, "Expected 'with'")?;
        let mut items = Vec::new();
        loop {
            let context = self.parse_expression(Precedence::Lowest)?;
            let var = if matches!(self.peek_token()?, Some(Token::As)) {
                self.next_token()?; // consume 'as'
                Some(self.expect_identifier("Expected a name after 'as' in a 'with' item")?)
            } else {
                None
            };
            items.push(WithItem { context, var });
            if matches!(self.peek_token()?, Some(Token::Comma)) {
                self.next_token()?; // consume ',' and parse the next manager
            } else {
                break;
            }
        }
        self.expect(Token::Colon, "Expected ':' after the 'with' items")?;
        let body = self.parse_suite()?;
        Ok(StmtKind::With { items, body })
    }

    pub(super) fn parse_try(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(Token::Try, "Expected 'try'")?;
        self.expect(Token::Colon, "Expected ':' after 'try'")?;
        let body = self.parse_suite()?;

        let except = if matches!(self.peek_token()?, Some(Token::Except)) {
            // An optional name binds the caught error.
            self.next_token()?; // consume 'except'
            let name = if matches!(self.peek_token()?, Some(Token::Identifier(_))) {
                Some(self.expect_identifier("unreachable")?)
            } else {
                None
            };
            self.expect(Token::Colon, "Expected ':' after 'except'")?;
            Some((name, self.parse_suite()?))
        } else {
            None
        };

        let orelse = if matches!(self.peek_token()?, Some(Token::Else)) {
            self.next_token()?; // consume 'else'
            self.expect(Token::Colon, "Expected ':' after 'else'")?;
            Some(self.parse_suite()?)
        } else {
            None
        };

        let finalbody = if matches!(self.peek_token()?, Some(Token::Finally)) {
            self.next_token()?; // consume 'finally'
            self.expect(Token::Colon, "Expected ':' after 'finally'")?;
            Some(self.parse_suite()?)
        } else {
            None
        };

        if except.is_none() && finalbody.is_none() {
            return Err(ParseError::UnexpectedToken(
                Token::Try,
                "a 'try' needs at least one of 'except' or 'finally'".into(),
            ));
        }
        Ok(StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        })
    }

    /// Parses a `cond ':' NEWLINE block` clause shared by `if`/`elif`/`while`.
    /// The leading keyword has already been consumed.
    pub(super) fn parse_condition_block(
        &mut self,
        ctx: &str,
    ) -> Result<(Expr, Vec<Stmt>), ParseError> {
        let cond = self.parse_expression(Precedence::Lowest)?;
        self.expect(Token::Colon, ctx)?;
        let body = self.parse_suite()?;
        Ok((cond, body))
    }

    /// `if cond: <block> (elif cond: <block>)* (else: <block>)?`
    pub(super) fn parse_if(&mut self) -> Result<StmtKind, ParseError> {
        let (branches, orelse) = self.parse_if_rest()?;
        Ok(StmtKind::If { branches, orelse })
    }

    /// Parses an `if`/`elif`/`else` chain — the current token must be `if`. Shared
    /// by the runtime `if` and the compile-time `comptime if` (which differ only in
    /// the wrapping `Stmt` variant).
    pub(super) fn parse_if_rest(&mut self) -> Result<IfChain, ParseError> {
        self.expect(Token::If, "Expected 'if'")?;
        let mut branches = vec![self.parse_condition_block("Expected ':' after the if condition")?];

        while matches!(self.peek_token()?, Some(Token::Elif)) {
            self.next_token()?; // consume 'elif'
            branches.push(self.parse_condition_block("Expected ':' after the elif condition")?);
        }

        let orelse = if matches!(self.peek_token()?, Some(Token::Else)) {
            self.next_token()?; // consume 'else'
            self.expect(Token::Colon, "Expected ':' after 'else'")?;
            Some(self.parse_suite()?)
        } else {
            None
        };

        Ok((branches, orelse))
    }

    /// `while cond: <block>`
    pub(super) fn parse_while(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(Token::While, "Expected 'while'")?;
        let (cond, body) = self.parse_condition_block("Expected ':' after the while condition")?;
        let orelse = self.parse_loop_else()?;
        Ok(StmtKind::While { cond, body, orelse })
    }

    /// `for [ref | var] name in iter: <block>`
    pub(super) fn parse_for(&mut self) -> Result<StmtKind, ParseError> {
        let (var, binding, iter, body) = self.parse_for_rest()?;
        let orelse = self.parse_loop_else()?;
        Ok(StmtKind::For {
            var,
            binding,
            iter,
            body,
            orelse,
        })
    }

    pub(super) fn parse_loop_else(&mut self) -> Result<Option<Vec<Stmt>>, ParseError> {
        if !matches!(self.peek_token()?, Some(Token::Else)) {
            return Ok(None);
        }
        self.next_token()?;
        self.expect(Token::Colon, "Expected ':' after loop 'else'")?;
        Ok(Some(self.parse_suite()?))
    }

    pub(super) fn parse_loop_binding_mode(&mut self) -> Result<LoopBindingMode, ParseError> {
        match self.peek_token()? {
            Some(Token::Identifier(word)) if word == "ref" => {
                self.next_token()?;
                Ok(LoopBindingMode::Ref)
            }
            Some(Token::Var) => {
                self.next_token()?;
                Ok(LoopBindingMode::Var)
            }
            _ => Ok(LoopBindingMode::Immutable),
        }
    }

    /// Parses a `for [ref | var] name in iter: <block>` — the current token
    /// must be `for`. Shared by runtime and compile-time loops.
    pub(super) fn parse_for_rest(
        &mut self,
    ) -> Result<(String, LoopBindingMode, Expr, Vec<Stmt>), ParseError> {
        self.expect(Token::For, "Expected 'for'")?;
        let binding = self.parse_loop_binding_mode()?;
        let var = self.expect_identifier("Expected a loop variable name after 'for'")?;
        self.expect(Token::In, "Expected 'in' after the loop variable")?;
        let iter = self.parse_expression(Precedence::Lowest)?;
        self.expect(Token::Colon, "Expected ':' after the for-loop iterable")?;
        let body = self.parse_suite()?;
        Ok((var, binding, iter, body))
    }

    /// `return` or `return expr`
    pub(super) fn parse_return(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(Token::Return, "Expected 'return'")?;
        let value = match self.peek_token()? {
            Some(Token::Newline) | Some(Token::Eof) | None => None,
            _ => Some(self.parse_tuple_display()?),
        };
        self.expect_stmt_end()?;
        Ok(StmtKind::Return(value))
    }

    /// Parses an indented block: `INDENT statement+ DEDENT`.
    pub(super) fn parse_block(&mut self) -> Result<Vec<Stmt>, ParseError> {
        self.expect(Token::Indent, "Expected an indented block")?;
        self.parse_block_body()
    }

    /// Parses a Mojo/Python suite after `:`: either a newline followed by an
    /// indented block, or one simple statement on the same physical line.
    pub(super) fn parse_suite(&mut self) -> Result<Vec<Stmt>, ParseError> {
        if matches!(self.peek_token()?, Some(Token::Newline)) {
            self.next_token()?; // consume the newline after ':'
            self.parse_block()
        } else {
            let mut body = Vec::new();
            loop {
                body.push(self.parse_statement()?);
                if !self.last_stmt_ended_with_semicolon {
                    break;
                }
                if matches!(self.peek_token()?, Some(Token::Newline)) {
                    self.next_token()?; // consume the logical line after trailing ';'
                    self.last_stmt_ended_with_semicolon = false;
                    break;
                }
                if matches!(self.peek_token()?, Some(Token::Eof) | None) {
                    self.last_stmt_ended_with_semicolon = false;
                    break;
                }
            }
            Ok(body)
        }
    }

    /// Parses a trait method body, preserving `...` as a pure requirement.
    pub(super) fn parse_trait_method_body(&mut self) -> Result<Option<Vec<Stmt>>, ParseError> {
        if matches!(self.peek_token()?, Some(Token::Ellipsis)) {
            self.next_token()?; // consume same-line '...'
            self.expect_stmt_end()?;
            return Ok(None);
        }

        if matches!(self.peek_token()?, Some(Token::Newline)) {
            self.next_token()?; // consume the newline after ':'
            self.expect(Token::Indent, "Expected an indented trait-method body")?;
            if matches!(self.peek_token()?, Some(Token::Ellipsis)) {
                self.next_token()?; // consume indented '...'
                self.expect_stmt_end()?;
                self.expect(
                    Token::Dedent,
                    "Expected the trait-method body to end after '...'",
                )?;
                Ok(None)
            } else {
                Ok(Some(self.parse_block_body()?))
            }
        } else {
            Ok(Some(vec![self.parse_statement()?]))
        }
    }

    /// The statements of a block up to (and consuming) the closing `DEDENT`; the
    /// opening `INDENT` must already have been consumed. Split out so a trait
    /// method's default body can be parsed after peeking past the `INDENT` for `...`.
    pub(super) fn parse_block_body(&mut self) -> Result<Vec<Stmt>, ParseError> {
        let mut body = Vec::new();
        while let Some(token) = self.peek_token()? {
            match token {
                Token::Dedent => {
                    self.next_token()?; // consume the dedent to end the block
                    break;
                }
                Token::Newline => {
                    self.next_token()?; // skip blank lines inside the block
                }
                _ => body.push(self.parse_statement()?),
            }
        }
        Ok(body)
    }
}
