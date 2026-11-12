//! Type expression, parameter-argument, and type-parameter parsing.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl<I: Iterator<Item = Result<(Token, Span), LexError>>> Parser<I> {
    /// Parses a type annotation: `Bool`, `None`, `Self.T`, associated lookups
    /// like `C.Element`, or a named type optionally applied to type arguments
    /// (`Pair[Int]`). The atoms `None`, `Self`, and `Self.<member>` are read
    /// by the expression grammar and converted in the annotation's type
    /// context ([`Expr::annotation_type`]), as upstream parses an annotation.
    pub(super) fn parse_type(&mut self) -> Result<Type, ParseError> {
        if matches!(self.peek_token()?, Some(Token::None))
            || matches!(self.peek_token()?, Some(Token::Identifier(id)) if id == "Self")
        {
            let start = self.peek_start();
            let atom = self.parse_member_chain()?;
            let ty = atom.annotation_type().ok_or_else(|| {
                ParseError::Message("expected a type, found an expression".to_string())
            })?;
            return self.parse_type_assoc_tail(ty, start);
        }
        let token = self.next_token()?;
        let start = self.last_span.0;
        let ty = match token {
            // A variadic type-pack reference in `*args: *ArgTypes`. Inside a
            // struct, current Mojo spells the struct's own pack `*Self.Ts`;
            // the qualified spread keeps its own node so the elaborator can
            // tell it from a bare `*Ts`, which a struct member may not use.
            Token::Star => {
                if !matches!(self.peek_token()?, Some(Token::Identifier(_))) {
                    let name = self.expect_identifier("Expected a type-pack name after '*'")?;
                    return Ok(Type::Named(format!("*{name}"), Vec::new()));
                }
                let mut operand = self.parse_prefix()?;
                if matches!(&operand.kind, ExprKind::Identifier(id) if id == "Self")
                    && matches!(self.peek_token()?, Some(Token::Dot))
                {
                    operand = self.parse_infix(operand)?;
                    return pack_spread_type(&operand).ok_or_else(|| {
                        ParseError::Message("Expected a type-pack name after '*Self.'".to_string())
                    });
                }
                pack_spread_type(&operand).ok_or_else(|| {
                    ParseError::Message("Expected a type-pack name after '*'".to_string())
                })
            }
            // A function type: `def(types) [effects] -> ret`.
            Token::Def => self.parse_function_type_tail(),
            // A generator type: `__generator_type [params] result`.
            Token::GeneratorType => self.parse_generator_type_tail(),
            Token::Identifier(id) => match id.as_str() {
                // Upstream's `type_of(x)`: the type of a compile-time
                // expression, retained as its operand.
                "type_of" if matches!(self.peek_token()?, Some(Token::LParen)) => {
                    self.parse_type_of_operand()
                }
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
                _ => self.parse_named_type(id, start),
            },
            token => Err(ParseError::UnexpectedToken(
                token,
                "Expected a type name".into(),
            )),
        }?;

        self.parse_type_assoc_tail(ty, start)
    }

    /// Parses an annotation atom through the expression grammar: a primary
    /// followed by its `.NAME` members, leaving a following `[` or `(` to the
    /// annotation's projection tail.
    fn parse_member_chain(&mut self) -> Result<Expr, ParseError> {
        let mut atom = self.parse_prefix()?;
        while matches!(self.peek_token()?, Some(Token::Dot)) {
            atom = self.parse_infix(atom)?;
        }
        Ok(atom)
    }

    /// Parses the rest of a named type whose name `id` (starting at `start`)
    /// was consumed: its optional parameter arguments, and a following
    /// argument list, which makes it a compile-time value (`Pair[Int](1, 2)`).
    fn parse_named_type(&mut self, id: String, start: usize) -> Result<Type, ParseError> {
        let args = if matches!(self.peek_token()?, Some(Token::LBracket)) {
            self.parse_param_args()?
        } else {
            Vec::new()
        };
        if !matches!(self.peek_token()?, Some(Token::LParen)) {
            return Ok(Type::Named(id, args));
        }
        self.next_token()?;
        let (call_args, kwargs) = self.parse_call_args()?;
        self.expect(&Token::RParen, "Expected ')' after arguments")?;
        Ok(Type::Value(Box::new(Expr::new(
            ExprKind::Call {
                name: id,
                param_args: args,
                args: call_args,
                kwargs,
            },
            (start, self.last_span.1),
        ))))
    }

    /// Parse zero or more structured projections after a type atom. Associated
    /// lookup and dependent indexing deliberately remain distinct AST nodes so
    /// later phases never have to recover either operation from a flattened
    /// source spelling.
    pub(super) fn parse_type_assoc_tail(
        &mut self,
        mut ty: Type,
        start: usize,
    ) -> Result<Type, ParseError> {
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
                // A member's bracket list a call follows is its application
                // (`M.Of[Int]()`), whatever its one operand parsed as.
                let applied = matches!(self.peek_token()?, Some(Token::LParen))
                    && matches!(&ty, Type::Assoc { args, .. } if args.is_empty());
                let index = match (arguments.as_slice(), &mut ty) {
                    ([mojito_ast::ast::ParamArg::Value(index)], _) if !applied => index,
                    // A parameterized member applied to its bracket list
                    // (`S[Int].Of[Bool]`, `M.Pair[1, 2]`).
                    (_, Type::Assoc { args, .. }) if args.is_empty() => {
                        *args = arguments;
                        continue;
                    }
                    _ => {
                        return Err(ParseError::UnexpectedToken(
                            Token::RBracket,
                            "a dependent type projection requires exactly one compile-time \
                             value index"
                                .to_string(),
                        ));
                    }
                };
                ty = Type::IndexedProjection {
                    base: Box::new(ty),
                    index: Box::new(index.clone()),
                };
                continue;
            }
            // A construction through an applied member (`TypeList.of[Trait=
            // AnyType, Bool, Int]()`), the value a following subscript reads.
            if let Type::Assoc { base, name, args } = &ty
                && !args.is_empty()
                && matches!(self.peek_token()?, Some(Token::LParen))
                && let Some(receiver) = base.expression_spelling((start, self.last_span.1))
            {
                let callee = Expr::new(
                    ExprKind::Member {
                        object: Box::new(receiver),
                        field: name.clone(),
                    },
                    (start, self.last_span.1),
                );
                let param_args = args.clone();
                self.next_token()?;
                let (call_args, kwargs) = self.parse_call_args()?;
                self.expect(&Token::RParen, "Expected ')' after arguments")?;
                ty = Type::Value(Box::new(Expr::new(
                    ExprKind::Invoke {
                        callee: Box::new(callee),
                        param_args,
                        args: call_args,
                        kwargs,
                    },
                    (start, self.last_span.1),
                )));
                continue;
            }
            break;
        }
        Ok(ty)
    }

    /// Parses a generator type after its leading `__generator_type` has been
    /// consumed, as upstream's `parseGeneratorType` does: an optional
    /// parameter list, then the result type expression. A `&` composition
    /// of traits is that expression whole (`__generator_type[T: Copyable]
    /// Copyable & Defaultable`), recorded as a `$trait_composition`.
    pub(super) fn parse_generator_type_tail(&mut self) -> Result<Type, ParseError> {
        let params = if matches!(self.peek_token()?, Some(Token::LBracket)) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };
        let mut members = vec![self.parse_type()?];
        while matches!(self.peek_token()?, Some(Token::Amp)) {
            self.next_token()?;
            members.push(self.parse_type()?);
        }
        let result = if members.len() == 1 {
            members.pop().expect("one generator result")
        } else {
            Type::Named(
                "$trait_composition".to_string(),
                members
                    .into_iter()
                    .map(mojito_ast::ast::ParamArg::Type)
                    .collect(),
            )
        };
        Ok(Type::Generator {
            params,
            result: Box::new(result),
        })
    }

    /// Parses a function type after its leading `def` has been consumed:
    /// `'(' [type (',' type)*] ')' effects ['->' type]`. Effects between `)` and
    /// the optional return are `thin`, `raises`, and `abi(...)` (the last
    /// parsed and discarded); the removed `capturing` is an error. An omitted
    /// return is `None`.
    pub(super) fn parse_function_type_tail(&mut self) -> Result<Type, ParseError> {
        // Function signatures may themselves be parameterized, e.g.
        // `def[origin: Origin](ref[origin] Int) -> ref[origin] Int`.
        let type_params = if matches!(self.peek_token()?, Some(Token::LBracket)) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };
        self.expect(&Token::LParen, "Expected '(' in a function type")?;
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
                        &Token::Colon,
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
        self.expect(
            &Token::RParen,
            "Expected ')' after function-type parameters",
        )?;

        // Effects: `thin` / `raises` / `abi("…")` in any order, until `->`.
        let mut thin = false;
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
                    self.expect(&Token::LParen, "Expected '(' after 'abi'")?;
                    // Discard the abi specifier's contents.
                    while !matches!(self.peek_token()?, Some(Token::RParen) | None) {
                        self.next_token()?;
                    }
                    self.expect(&Token::RParen, "Expected ')' to close 'abi(...)'")?;
                }
                Some(Token::Identifier(id)) if id == "capturing" => {
                    return Err(ParseError::Message(
                        super::REMOVED_CAPTURING_EFFECT.to_string(),
                    ));
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
        self.expect(
            &Token::LBracket,
            "Expected '[' to begin parameter arguments",
        )?;
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
        self.expect(&Token::RBracket, "Expected ']' after parameter arguments")?;
        Ok(args)
    }

    /// Parses a single parameter argument. An operand only the type grammar
    /// spells (a function type, `ref`, a `*` pack) is a `Type`. Every other
    /// operand is one expression (`Int`, `Foo[Int]`, `L[0] * 2`, `Self.T`,
    /// and `None`, which is the `None` value), which the checker classifies
    /// as a type or a value by what it denotes.
    pub(super) fn parse_param_arg(&mut self) -> Result<mojito_ast::ast::ParamArg, ParseError> {
        use mojito_ast::ast::ParamArg;
        // A spread: of a pack (`*Ts`, `*Self.Ts`), which stays the pack's own
        // type node, or of any other type-list expression
        // (`*Ts.reverse()`, `*TypeList._concat[A.values, B.values]()`).
        if matches!(self.peek_token()?, Some(Token::Star)) {
            let start = self.peek_start();
            self.next_token()?; // consume '*'
            if !matches!(self.peek_token()?, Some(Token::Identifier(_))) {
                let name = self.expect_identifier("Expected a type-pack name after '*'")?;
                return Ok(ParamArg::Type(Type::Named(format!("*{name}"), Vec::new())));
            }
            let operand = self.parse_expression(Precedence::Lowest)?;
            return Ok(match pack_spread_type(&operand) {
                Some(ty) => ParamArg::Type(ty),
                None => ParamArg::Value(self.node(ExprKind::Spread(Box::new(operand)), start)),
            });
        }
        if self.peek_starts_type()? {
            let ty = self.parse_type()?;
            // Upstream reads `ref[o]` here as an expression followed by a stray
            // type: an origin-bearing reference is never a type argument.
            if matches!(
                ty,
                Type::Ref {
                    origin: Some(_),
                    ..
                }
            ) {
                return Err(ParseError::Message(
                    "expected ']' in call argument list".to_string(),
                ));
            }
            return Ok(ParamArg::Type(ty));
        }
        Ok(ParamArg::Value(self.parse_expression(Precedence::Lowest)?))
    }

    /// Whether the next token begins an operand only the type grammar
    /// parses (a function type, `ref`, or a `*` pack) — used to classify a
    /// parameter argument.
    pub(super) fn peek_starts_type(&mut self) -> Result<bool, ParseError> {
        Ok(match self.peek_token()? {
            Some(Token::Def | Token::Star) => true,
            Some(Token::Identifier(id)) => id == "ref",
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
            if self.parse_type_param_marker(&mut params)? {
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
                &Token::Colon,
                "A type parameter requires a ': bound' (e.g. 'T: Copyable')",
            )?;
            let mut value_type = None;
            let (first_bound, callable_bound, spelled_bound) =
                if matches!(self.peek_token()?, Some(Token::Def)) {
                    (
                        "<function type>".to_string(),
                        Some(self.parse_type()?),
                        String::new(),
                    )
                // `G: __generator_type[Idx: Int] Int` is a value parameter
                // typed by a generator type, recorded as for `type_of`.
                } else if matches!(self.peek_token()?, Some(Token::GeneratorType)) {
                    value_type = Some(self.parse_type()?);
                    (
                        "__generator_type".to_string(),
                        None,
                        "__generator_type".to_string(),
                    )
                } else {
                    let bound = self.expect_identifier(
                        "Expected a trait or type in the type-parameter bound",
                    )?;
                    (bound.clone(), None, bound)
                };
            // Origin parameters use `Origin[mut=<bool expression>]`. Preserve the
            // Origin classification and parse the mutability expression; semantic
            // origin parameters are deliberately deferred. An alias of an origin
            // type (`ImmOrigin`, `MutOrigin`) records as spelled, and
            // preparation resolves it.
            // `Tr: type_of(AnyType)` is a value parameter typed by the trait
            // metatype; the spelled head stays the bound, as for a qualified
            // value type.
            let first_bound = if first_bound == "type_of"
                && callable_bound.is_none()
                && matches!(self.peek_token()?, Some(Token::LParen))
            {
                value_type = Some(self.parse_type_of_operand()?);
                first_bound
            // `v: Self.T` is a value parameter typed by the enclosing
            // struct's type parameter.
            } else if first_bound == "Self"
                && callable_bound.is_none()
                && matches!(self.peek_token()?, Some(Token::Dot))
            {
                self.next_token()?;
                let param = self.expect_identifier("Expected a parameter name after 'Self.'")?;
                let spelled = format!("Self.{param}");
                value_type = Some(Type::SelfParam(param));
                spelled
            } else if callable_bound.is_none() && matches!(self.peek_token()?, Some(Token::Dot)) {
                // `x: M.A` or `raw: _MLIR.KGENParamListType[t]` is a value
                // parameter typed by a qualified type.
                let start = self.last_span.0;
                let qualified = self
                    .parse_type_assoc_tail(Type::Named(spelled_bound.clone(), Vec::new()), start)?;
                value_type = Some(qualified);
                spelled_bound
            } else {
                first_bound
            };
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
                    self.expect(&Token::Assign, "Expected '=' after 'mut' in Origin")?;
                    let mutability = self.parse_expression(Precedence::Lowest)?;
                    self.expect(&Token::RBracket, "Expected ']' after Origin mutability")?;
                    Some(mutability)
                } else if matches!(self.peek_token()?, Some(Token::LBracket)) {
                    let args = self.parse_param_args()?;
                    value_type = Some(Type::Named(first_bound.clone(), args));
                    None
                } else {
                    None
                };
            let mut bounds = vec![first_bound];
            let mut callable_bound = callable_bound;
            while matches!(self.peek_token()?, Some(Token::Amp)) {
                // A function type ends the composition: upstream reads its
                // result as an expression, which takes any later `& Trait`.
                if callable_bound.is_some() {
                    return Err(ParseError::UnexpectedToken(
                        self.next_token()?,
                        "a function type ends a type-parameter bound; compose its traits before it"
                            .into(),
                    ));
                }
                self.next_token()?; // consume '&'
                // A function type composes with the traits as one more
                // requirement of the binder (`F: ImplicitlyCopyable &
                // def(Int) -> Int`).
                if matches!(self.peek_token()?, Some(Token::Def)) {
                    callable_bound = Some(self.parse_type()?);
                    bounds.push("<function type>".to_string());
                    continue;
                }
                let bound = self.expect_identifier("Expected a trait name after '&'")?;
                bounds.push(bound);
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
        self.expect(&Token::RBracket, "Expected ']' after type parameters")?;
        Ok(params)
    }

    /// Consume a parameter-list marker and the `,` after it, reporting
    /// whether one was there. Mojo's `//` ends the infer-only prefix, so
    /// every parameter before it is inferred; the `/` positional-only
    /// marker, as the bare `*` keyword-only one, the list does not record
    /// yet (roadmap R1735). A marker is followed by `,` or the closing `]`.
    fn parse_type_param_marker(
        &mut self,
        params: &mut [mojito_ast::ast::TypeParam],
    ) -> Result<bool, ParseError> {
        match self.peek_token()? {
            Some(Token::DoubleSlash) => {
                for parameter in params {
                    parameter.infer_only = true;
                }
            }
            Some(Token::Slash) => {}
            _ => return Ok(false),
        }
        self.next_token()?;
        match self.peek_token()? {
            Some(Token::Comma) => {
                self.next_token()?;
            }
            Some(Token::RBracket) => {}
            _ => {
                return Err(ParseError::UnexpectedToken(
                    self.next_token()?,
                    "expected ']' for parameter list".into(),
                ));
            }
        }
        Ok(true)
    }

    /// The parenthesized operand of `type_of`, whose name was just read.
    fn parse_type_of_operand(&mut self) -> Result<Type, ParseError> {
        self.expect(&Token::LParen, "Expected '(' after 'type_of'")?;
        let operand = self.parse_expression(Precedence::Lowest)?;
        self.expect(&Token::RParen, "Expected ')' after the 'type_of' operand")?;
        Ok(Type::TypeOf(Box::new(operand)))
    }

    // --- Expressions (Pratt parser) ---
}

/// The pack type a `*` spread's operand names: a bare pack (`*Ts`) or the
/// enclosing struct's own pack (`*Self.Ts`). `None` for any other type-list
/// expression, which spreads as a value.
fn pack_spread_type(operand: &Expr) -> Option<Type> {
    match &operand.kind {
        ExprKind::Identifier(name) => Some(Type::Named(format!("*{name}"), Vec::new())),
        _ => operand
            .self_member()
            .map(|field| Type::SelfParam(format!("*{field}"))),
    }
}
