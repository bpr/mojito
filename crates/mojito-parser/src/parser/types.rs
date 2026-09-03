//! Type expression, parameter-argument, and type-parameter parsing.

use super::*;

impl<I: Iterator<Item = Result<(Token, Span), LexError>>> Parser<I> {
    /// Parses a type annotation: a scalar keyword, `Self.T`, associated lookups
    /// like `C.Element`, or a named type optionally applied to type arguments
    /// (`Pair[Int]`).
    pub(super) fn parse_type(&mut self) -> Result<Type, ParseError> {
        let ty = match self.next_token()? {
            // A variadic type-pack reference in `*args: *ArgTypes`.
            Token::Star => {
                let name = self.expect_identifier("Expected a type-pack name after '*'")?;
                Ok(Type::Named(format!("*{name}"), Vec::new()))
            }
            // A function type: `def(types) [effects] -> ret`.
            Token::Def => self.parse_function_type_tail(),
            Token::None => Ok(Type::None),
            Token::Identifier(id) => match id.as_str() {
                "Int" => Ok(Type::Int),
                "UInt" => Ok(Type::UInt),
                "Bool" => Ok(Type::Bool),
                "StringLiteral" => Ok(Type::StringLiteral),
                "Float64" => Ok(Type::Float64),
                // `Self.T` references one of the enclosing struct's type
                // parameters; bare `Self` is the enclosing struct/trait type.
                "Self" if matches!(self.peek_token()?, Some(Token::Dot)) => {
                    self.next_token()?; // consume '.'
                    let param =
                        self.expect_identifier("Expected a type parameter name after 'Self.'")?;
                    Ok(Type::SelfParam(param))
                }
                "Self" => Ok(Type::SelfType),
                // `ref [origin] T` — a reference type (parametric mutability). The
                // origin specifier is parsed and discarded; the referent follows.
                // (`ref` is contextual — a following `[` or type token, not `.`/end.)
                "ref"
                    if matches!(
                        self.peek_token()?,
                        Some(Token::LBracket | Token::Identifier(_) | Token::Def | Token::None)
                    ) =>
                {
                    let origin = self.parse_optional_origin_specifier()?;
                    Ok(Type::Ref {
                        referent: Box::new(self.parse_type()?),
                        origin,
                    })
                }
                // Any other identifier names a struct type or an in-scope type
                // parameter (the checker decides), optionally with parameter args.
                _ => {
                    let args = if matches!(self.peek_token()?, Some(Token::LBracket)) {
                        self.parse_param_args()?
                    } else {
                        Vec::new()
                    };
                    Ok(Type::Named(id, args))
                }
            },
            token => Err(ParseError::UnexpectedToken(
                token,
                "Expected a type name".into(),
            )),
        }?;

        self.parse_type_assoc_tail(ty)
    }

    /// Parse zero or more structured projections after a type atom. Associated
    /// lookup and dependent indexing deliberately remain distinct AST nodes so
    /// later phases never have to recover either operation from a flattened
    /// source spelling.
    pub(super) fn parse_type_assoc_tail(&mut self, mut ty: Type) -> Result<Type, ParseError> {
        loop {
            if matches!(self.peek_token()?, Some(Token::Dot)) {
                self.next_token()?; // consume '.'
                let name = self.expect_identifier("Expected an associated type name after '.'")?;
                ty = Type::Assoc {
                    base: Box::new(ty),
                    name,
                    args: Vec::new(),
                };
                continue;
            }
            if matches!(self.peek_token()?, Some(Token::LBracket)) {
                let arguments = self.parse_param_args()?;
                let [mojito_ast::ast::ParamArg::Value(index)] = arguments.as_slice() else {
                    return Err(ParseError::UnexpectedToken(
                        Token::RBracket,
                        "a dependent type projection requires exactly one compile-time value index"
                            .to_string(),
                    ));
                };
                ty = Type::IndexedProjection {
                    base: Box::new(ty),
                    index: Box::new(index.clone()),
                };
                continue;
            }
            break;
        }
        Ok(ty)
    }

    /// Parses a function type after its leading `def` has been consumed:
    /// `'(' [type (',' type)*] ')' effects ['->' type]`. Effects between `)` and
    /// the optional return are `thin`, `capturing[origins]`, `raises`, and
    /// `abi(...)` (the last parsed and discarded). An omitted return is `None`.
    pub(super) fn parse_function_type_tail(&mut self) -> Result<Type, ParseError> {
        // Function signatures may themselves be parameterized, e.g.
        // `def[origin: Origin](ref[origin] Int) -> ref[origin] Int`.
        let type_params = if matches!(self.peek_token()?, Some(Token::LBracket)) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };
        self.expect(Token::LParen, "Expected '(' in a function type")?;
        let mut params = Vec::new();
        if !matches!(self.peek_token()?, Some(Token::RParen)) {
            loop {
                if matches!(self.peek_token()?, Some(Token::Slash | Token::DoubleSlash)) {
                    self.next_token()?;
                    if matches!(self.peek_token()?, Some(Token::Comma)) {
                        self.next_token()?;
                    }
                    continue;
                }
                let mut kind = ParamKind::Regular;
                let convention = match self.peek_token()?.cloned() {
                    Some(Token::Var) => {
                        self.next_token()?;
                        if matches!(self.peek_token()?, Some(Token::DoubleStar)) {
                            self.next_token()?;
                            kind = ParamKind::KwVariadic;
                        }
                        Some(ArgConvention::Var)
                    }
                    Some(Token::DoubleStar) => {
                        return Err(ParseError::UnexpectedToken(
                            self.next_token()?,
                            "a keyword-variadic function-type parameter must be spelled \
                             'var **name: Type'"
                                .to_string(),
                        ));
                    }
                    Some(Token::Identifier(word)) if convention_word(&word).is_some() => {
                        self.next_token()?;
                        convention_word(&word)
                    }
                    Some(Token::Identifier(word)) if word == "read" => {
                        let word = word.clone();
                        return Err(removed_convention_error(&word).expect("read is removed"));
                    }
                    _ => None,
                };
                let origin = if convention == Some(ArgConvention::Ref) {
                    self.parse_optional_origin_specifier()?
                } else {
                    None
                };
                let (name, ty) = if kind == ParamKind::KwVariadic {
                    let name = self.expect_identifier("Expected a name after 'var **'")?;
                    self.expect(
                        Token::Colon,
                        "Expected ':' after a keyword-variadic function-type parameter",
                    )?;
                    (Some(name), self.parse_type()?)
                } else {
                    let first = self.parse_type()?;
                    if matches!(self.peek_token()?, Some(Token::Colon)) {
                        let Type::Named(name, arguments) = first else {
                            return Err(ParseError::UnexpectedToken(
                                Token::Colon,
                                "a function-type parameter name must be an identifier".to_string(),
                            ));
                        };
                        if !arguments.is_empty() {
                            return Err(ParseError::UnexpectedToken(
                                Token::Colon,
                                "a function-type parameter name cannot have type arguments"
                                    .to_string(),
                            ));
                        }
                        self.next_token()?;
                        (Some(name), self.parse_type()?)
                    } else {
                        (None, first)
                    }
                };
                params.push(FunctionTypeParam {
                    name,
                    kind,
                    convention,
                    origin,
                    ty,
                });
                if matches!(self.peek_token()?, Some(Token::Comma)) {
                    self.next_token()?; // consume ','
                } else {
                    break;
                }
            }
        }
        self.expect(Token::RParen, "Expected ')' after function-type parameters")?;

        // Effects: `thin` / `raises` / `abi("…")` in any order, until `->`.
        let mut thin = false;
        let mut capturing = None;
        let mut raises = false;
        let mut raises_type = None;
        loop {
            match self.peek_token()? {
                Some(Token::Identifier(id)) if id == "thin" => {
                    self.next_token()?;
                    thin = true;
                }
                Some(Token::Raises) => {
                    self.next_token()?;
                    raises = true;
                    let next = self.peek_token()?.cloned();
                    let next_is_effect = matches!(
                        next.as_ref(),
                        Some(Token::Identifier(effect))
                            if matches!(effect.as_str(), "capturing" | "thin" | "abi")
                    );
                    let next_ends_type = matches!(
                        next,
                        None | Some(
                            Token::Arrow
                                | Token::RBracket
                                | Token::RParen
                                | Token::Comma
                                | Token::Assign
                                | Token::Colon
                                | Token::Amp
                                | Token::Newline
                                | Token::Dedent
                                | Token::Eof
                        )
                    );
                    if !next_is_effect && !next_ends_type {
                        raises_type = Some(Box::new(self.parse_type()?));
                    }
                }
                Some(Token::Identifier(id)) if id == "abi" => {
                    self.next_token()?; // consume 'abi'
                    self.expect(Token::LParen, "Expected '(' after 'abi'")?;
                    // Discard the abi specifier's contents.
                    while !matches!(self.peek_token()?, Some(Token::RParen) | None) {
                        self.next_token()?;
                    }
                    self.expect(Token::RParen, "Expected ')' to close 'abi(...)'")?;
                }
                Some(Token::Identifier(id)) if id == "capturing" => {
                    self.next_token()?;
                    capturing = Some(self.parse_optional_origin_specifier()?.unwrap_or_default());
                }
                _ => break,
            }
        }

        let ret = if matches!(self.peek_token()?, Some(Token::Arrow)) {
            self.next_token()?;
            self.parse_type()?
        } else {
            Type::None
        };
        // Trailing `where` clauses bind to the innermost function type: a
        // function-type RETURN type consumes them here through its own
        // `parse_type` above, matching upstream's rule that a declaration-
        // level `where` after a function-type result needs the result
        // parenthesized (Mojito has no parenthesized types, so that spelling
        // is simply unavailable).
        let where_clauses = self.parse_where_clauses()?;
        Ok(Type::Func {
            type_params,
            params,
            ret: Box::new(ret),
            thin,
            capturing,
            raises,
            raises_type,
            where_clauses,
        })
    }

    /// Parses a parameter-argument list `'[' param_arg (',' param_arg)* ']'`. The
    /// next token must be `[`. Used for `Pair[Int]` / `FixedBuffer[8]`.
    pub(super) fn parse_param_args(
        &mut self,
    ) -> Result<Vec<mojito_ast::ast::ParamArg>, ParseError> {
        self.expect(Token::LBracket, "Expected '[' to begin parameter arguments")?;
        let mut args = Vec::new();
        loop {
            let mut arg = self.parse_param_arg()?;
            if matches!(self.peek_token()?, Some(Token::Assign)) {
                let name = match &arg {
                    mojito_ast::ast::ParamArg::Value(Expr {
                        kind: ExprKind::Identifier(name),
                        ..
                    }) => name.clone(),
                    _ => {
                        return Err(ParseError::UnexpectedToken(
                            Token::Assign,
                            "a compile-time keyword argument requires a name".into(),
                        ));
                    }
                };
                self.next_token()?;
                arg = mojito_ast::ast::ParamArg::Named {
                    name,
                    value: Box::new(mojito_ast::ast::ParamArg::Value(
                        self.parse_expression(Precedence::Lowest)?,
                    )),
                };
            }
            args.push(arg);
            if matches!(self.peek_token()?, Some(Token::Comma)) {
                self.next_token()?; // consume ','
                if matches!(self.peek_token()?, Some(Token::RBracket)) {
                    break;
                }
            } else {
                break;
            }
        }
        self.expect(Token::RBracket, "Expected ']' after parameter arguments")?;
        Ok(args)
    }

    /// Parses a single parameter argument: a `Type` (for a type parameter) or a
    /// comptime value `Expr` (for a value parameter). A leading type keyword,
    /// `None`, or `Self` is unambiguously a type. A bare identifier followed by
    /// `[` is a parameterized type (`Foo[Int]`); otherwise an identifier starts a
    /// value expression (a lone identifier is left for the checker to reinterpret
    /// as a type when the parameter is a type one). Anything else is a value.
    pub(super) fn parse_param_arg(&mut self) -> Result<mojito_ast::ast::ParamArg, ParseError> {
        use mojito_ast::ast::ParamArg;
        if self.peek_starts_type()? {
            let start = self.peek_start();
            let ty = self.parse_type()?;
            if matches!(self.peek_token()?, Some(Token::LParen)) {
                let name = match &ty {
                    Type::Int => Some("Int"),
                    Type::UInt => Some("UInt"),
                    Type::Bool => Some("Bool"),
                    Type::StringLiteral => Some("StringLiteral"),
                    Type::Float64 => Some("Float64"),
                    _ => None,
                };
                if let Some(name) = name {
                    let atom = Expr::new(
                        ExprKind::Identifier(name.to_string()),
                        (start, self.last_span.1),
                    );
                    return Ok(ParamArg::Value(
                        self.parse_expression_from(atom, Precedence::Lowest)?,
                    ));
                }
            }
            // A projection chained off the qualified struct binder
            // (`Self.o._get_owned_interior[...]` in an origin slot) is a value
            // expression: the type parse stops at `Self.o`, so a following
            // `.` re-parses the binder as an expression atom and continues.
            if let Type::SelfParam(param) = &ty
                && matches!(self.peek_token()?, Some(Token::Dot))
            {
                let atom = Expr::new(
                    ExprKind::Member {
                        object: Box::new(Expr::new(
                            ExprKind::Identifier("Self".to_string()),
                            (start, self.last_span.1),
                        )),
                        field: param.clone(),
                    },
                    (start, self.last_span.1),
                );
                return Ok(ParamArg::Value(
                    self.parse_expression_from(atom, Precedence::Lowest)?,
                ));
            }
            return Ok(ParamArg::Type(ty));
        }
        if let Some(Token::Identifier(_)) = self.peek_token()? {
            let id = self.expect_identifier("unreachable: peeked identifier")?;
            let id_span = self.last_span;
            if matches!(self.peek_token()?, Some(Token::LBracket)) {
                // Mojo type names and type parameters are conventionally
                // uppercase. A lowercase binding followed by brackets inside
                // another bracket list is therefore a runtime indexed value
                // (`mapping[indexes[i]]`), not a nested parameterized type.
                // Parsing from the identifier atom preserves the complete
                // postfix expression and lets the outer bracket choose Index.
                if id
                    .chars()
                    .next()
                    .is_some_and(|character| character.is_lowercase())
                {
                    let atom = Expr::new(ExprKind::Identifier(id), id_span);
                    return Ok(ParamArg::Value(
                        self.parse_expression_from(atom, Precedence::Lowest)?,
                    ));
                }
                let args = self.parse_param_args()?;
                if matches!(self.peek_token()?, Some(Token::LParen)) {
                    self.next_token()?;
                    let (call_args, kwargs) = self.parse_call_args()?;
                    self.expect(Token::RParen, "Expected ')' after arguments")?;
                    return Ok(ParamArg::Value(Expr::new(
                        ExprKind::Call {
                            name: id,
                            param_args: args,
                            args: call_args,
                            kwargs,
                        },
                        (id_span.0, self.last_span.1),
                    )));
                }
                return Ok(ParamArg::Type(Type::Named(id, args)));
            }
            // A value expression whose first atom is this identifier.
            let atom = Expr::new(ExprKind::Identifier(id), id_span);
            let expr = self.parse_expression_from(atom, Precedence::Lowest)?;
            return Ok(ParamArg::Value(expr));
        }
        Ok(ParamArg::Value(self.parse_expression(Precedence::Lowest)?))
    }

    /// Whether the next token unambiguously begins a *type* (a scalar keyword,
    /// `None`, `Self`, a function type, or Mojito's reference-type extension) —
    /// used to classify a parameter argument.
    pub(super) fn peek_starts_type(&mut self) -> Result<bool, ParseError> {
        Ok(match self.peek_token()? {
            Some(Token::None | Token::Def | Token::Star) => true,
            Some(Token::Identifier(id)) => {
                matches!(
                    id.as_str(),
                    "Int" | "UInt" | "Bool" | "StringLiteral" | "Float64" | "Self" | "ref"
                ) || mojito_ast::ast::Dtype::from_scalar_alias(id).is_some()
            }
            _ => false,
        })
    }

    /// Parses an optional type-parameter list `'[' type_param (',' type_param)* ']'`
    /// following a `struct`/`def` name. Returns an empty list if the next token is
    /// not `[`. Each parameter must carry a `: bound` (one or more trait names
    /// joined by `&`) — Mojo has no unconstrained type parameters.
    pub(super) fn parse_type_params(
        &mut self,
    ) -> Result<Vec<mojito_ast::ast::TypeParam>, ParseError> {
        if !matches!(self.peek_token()?, Some(Token::LBracket)) {
            return Ok(Vec::new());
        }
        self.next_token()?; // consume '['
        let mut params: Vec<mojito_ast::ast::TypeParam> = Vec::new();
        loop {
            // Mojo's `//` marker ends the infer-only prefix. Parameters before it
            // are inferred; parameters after it may be supplied explicitly.
            if matches!(self.peek_token()?, Some(Token::DoubleSlash)) {
                self.next_token()?;
                for parameter in &mut params {
                    parameter.infer_only = true;
                }
                if matches!(self.peek_token()?, Some(Token::Comma)) {
                    self.next_token()?;
                }
                if matches!(self.peek_token()?, Some(Token::RBracket)) {
                    break;
                }
                continue;
            }
            // Variadic compile-time parameter pack marker.
            let variadic = if matches!(self.peek_token()?, Some(Token::Star)) {
                self.next_token()?;
                if matches!(self.peek_token()?, Some(Token::Comma)) {
                    self.next_token()?;
                    continue;
                }
                true
            } else {
                false
            };
            let mut name = self.expect_identifier("Expected a type-parameter name")?;
            if variadic {
                name.insert(0, '*');
            }
            self.expect(
                Token::Colon,
                "A type parameter requires a ': bound' (e.g. 'T: Copyable')",
            )?;
            let (first_bound, callable_bound) = if matches!(self.peek_token()?, Some(Token::Def)) {
                ("<function type>".to_string(), Some(self.parse_type()?))
            } else {
                let bound =
                    self.expect_identifier("Expected a trait or type in the type-parameter bound")?;
                (
                    mojito_ast::ast::canonical_trait_name(&bound).to_string(),
                    None,
                )
            };
            // Origin parameters use `Origin[mut=<bool expression>]`. Preserve the
            // Origin classification and parse the mutability expression; semantic
            // origin parameters are deliberately deferred.
            let mut value_type = None;
            let origin_mutability =
                if first_bound == "Origin" && matches!(self.peek_token()?, Some(Token::LBracket)) {
                    self.next_token()?;
                    let key = self.expect_identifier("Expected 'mut' in Origin[mut=...]")?;
                    if key != "mut" {
                        return Err(ParseError::UnexpectedToken(
                            Token::Identifier(key),
                            "expected 'mut' in Origin[mut=...]".into(),
                        ));
                    }
                    self.expect(Token::Assign, "Expected '=' after 'mut' in Origin")?;
                    let mutability = self.parse_expression(Precedence::Lowest)?;
                    self.expect(Token::RBracket, "Expected ']' after Origin mutability")?;
                    Some(mutability)
                } else if matches!(self.peek_token()?, Some(Token::LBracket)) {
                    let args = self.parse_param_args()?;
                    value_type = Some(Type::Named(first_bound.clone(), args));
                    None
                } else {
                    None
                };
            let mut bounds = vec![first_bound];
            while matches!(self.peek_token()?, Some(Token::Amp)) {
                self.next_token()?; // consume '&'
                let bound = self.expect_identifier("Expected a trait name after '&'")?;
                bounds.push(mojito_ast::ast::canonical_trait_name(&bound).to_string());
            }
            if matches!(self.peek_token()?, Some(Token::Identifier(word)) if word == "where") {
                return Err(ParseError::UnexpectedToken(
                    self.next_token()?,
                    "parameter-list 'where' clauses were removed; place the constraint after the function return type"
                        .into(),
                ));
            }
            let default = if matches!(self.peek_token()?, Some(Token::Assign)) {
                self.next_token()?;
                Some(self.parse_expression(Precedence::Lowest)?)
            } else {
                None
            };
            params.push(mojito_ast::ast::TypeParam {
                name,
                bounds,
                value_type,
                callable_bound,
                origin_mutability,
                infer_only: false,
                default,
                constraints: Vec::new(),
            });
            if matches!(self.peek_token()?, Some(Token::Comma)) {
                self.next_token()?; // consume ','
                if matches!(self.peek_token()?, Some(Token::RBracket)) {
                    break;
                }
            } else {
                break;
            }
        }
        self.expect(Token::RBracket, "Expected ']' after type parameters")?;
        Ok(params)
    }

    // --- Expressions (Pratt parser) ---
}
