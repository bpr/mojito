//! Type expression, parameter-argument, and type-parameter parsing.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl<I: Iterator<Item = Result<(Token, Span), LexError>>> Parser<I> {
    /// Parses a type annotation and converts it in the annotation's type
    /// context ([`Expr::annotation_type`]), for a slot that still stores the
    /// converted type: a function type's parameters, result, and typed error,
    /// a `ref` referent, and a bracket's type operand.
    pub(super) fn parse_type(&mut self) -> Result<Type, ParseError> {
        let annotation = self.parse_annotation()?;
        annotation_type(&annotation)
    }

    /// Parses a type annotation as the expression upstream keeps: one whole
    /// expression at ternary level (`Bool`, `Self.T`, `Pair[Int]`,
    /// `type_of(x).T`, `def(Int) -> Int`, `__generator_type[T: AnyType] T`,
    /// `Int if c else Float64`), which converts in the annotation's type
    /// context ([`Expr::annotation_type`]); the checker evaluates one no
    /// spelling converts. Only a `*` pack and a contextual `ref` reference
    /// type keep their own forms, each stored as the
    /// [`ExprKind::TypeValue`] of its type.
    pub(super) fn parse_annotation(&mut self) -> Result<Expr, ParseError> {
        let start = self.peek_start();
        match self.peek_token()? {
            // A variadic type-pack reference in `*args: *ArgTypes`. Inside a
            // struct, current Mojo spells the struct's own pack `*Self.Ts`;
            // the qualified spread keeps its own node so the elaborator can
            // tell it from a bare `*Ts`, which a struct member may not use.
            Some(Token::Star) => {
                self.next_token()?;
                if !matches!(self.peek_token()?, Some(Token::Identifier(_))) {
                    let name = self.expect_identifier("Expected a type-pack name after '*'")?;
                    let ty = Type::Named(format!("*{name}"), Vec::new());
                    return Ok(self.node(ExprKind::TypeValue(ty), start));
                }
                let mut operand = self.parse_prefix()?;
                let message = if matches!(&operand.kind, ExprKind::Identifier(id) if id == "Self")
                    && matches!(self.peek_token()?, Some(Token::Dot))
                {
                    operand = self.parse_infix(operand)?;
                    "Expected a type-pack name after '*Self.'"
                } else {
                    "Expected a type-pack name after '*'"
                };
                let ty = pack_spread_type(&operand)
                    .ok_or_else(|| ParseError::Message(message.to_string()))?;
                Ok(self.node(ExprKind::TypeValue(ty), start))
            }
            Some(Token::Identifier(id)) if id == "ref" => {
                self.next_token()?;
                // `ref` is contextual: a following `[` or referent makes a
                // reference type; otherwise it is an ordinary name.
                if !matches!(
                    self.peek_token()?,
                    Some(
                        Token::LBracket
                            | Token::Identifier(_)
                            | Token::Def
                            | Token::GeneratorType
                            | Token::None
                            | Token::LParen
                    )
                ) {
                    let head = self.node(ExprKind::Identifier("ref".to_string()), start);
                    return self.annotation_from(head);
                }
                // `ref [origin] T` — a reference type (parametric
                // mutability); the referent follows the origin specifier.
                let origin = self.parse_optional_origin_specifier()?;
                let ty = Type::Ref {
                    referent: Box::new(self.parse_type()?),
                    origin,
                };
                Ok(self.node(ExprKind::TypeValue(ty), start))
            }
            _ => {
                let head = self.parse_prefix()?;
                self.annotation_from(head)
            }
        }
    }

    /// Parses a generator type after its leading `__generator_type` has been
    /// consumed, as upstream's `parseGeneratorType` does: an optional
    /// parameter list, then the result, one whole expression. A trait
    /// composition (`__generator_type[T: Copyable] Copyable & Defaultable`)
    /// is the `&` the expression grammar parses, converted by
    /// [`Expr::composition_type`].
    pub(super) fn parse_generator_type_tail(&mut self) -> Result<Type, ParseError> {
        let params = if matches!(self.peek_token()?, Some(Token::LBracket)) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };
        let expr = self.parse_expression(Precedence::Walrus)?;
        let result = expr.composition_type().ok_or_else(|| {
            ParseError::Message("expected a type, found an expression".to_string()).at(expr.span)
        })?;
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

        // The result is one whole expression, as upstream reads it.
        let ret = if matches!(self.peek_token()?, Some(Token::Arrow)) {
            self.next_token()?;
            self.parse_type()?
        } else {
            Type::None
        };
        // Trailing `where` clauses bind to the innermost function type: a
        // function-type result that is itself a function type consumes them
        // through its own tail above, matching upstream's rule that a
        // declaration-level `where` after a function-type result needs the
        // result parenthesized.
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
        if matches!(self.peek_token()?, Some(Token::RBracket)) {
            self.next_token()?;
            return Ok(args);
        }
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
    /// not `[`. Each parameter must carry a `: bound` — Mojo has no
    /// unconstrained type parameters. The bound is one expression, as upstream
    /// parses it, converted in the type context and classified by
    /// [`type_param_bound`].
    pub(super) fn parse_type_params(
        &mut self,
    ) -> Result<Vec<mojito_ast::ast::TypeParam>, ParseError> {
        if !matches!(self.peek_token()?, Some(Token::LBracket)) {
            return Ok(Vec::new());
        }
        self.next_token()?; // consume '['
        let mut params: Vec<mojito_ast::ast::TypeParam> = Vec::new();
        let mut keyword_only = false;
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
                    keyword_only = true;
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
            let bound = type_param_bound(&self.parse_expression(Precedence::Lowest)?)?;
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
                bounds: bound.bounds,
                value_type: bound.value_type,
                callable_bound: bound.callable_bound,
                origin_mutability: bound.origin_mutability,
                infer_only: false,
                keyword_only,
                default,
            });
            keyword_only |= variadic;
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
    /// marker the list does not record yet (roadmap R1735). The bare `*`
    /// keyword-only marker is read beside a pack's `*` by the caller. A marker is followed by `,` or the closing `]`.
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

    /// Reads the rest of an annotation, one whole expression, from the head
    /// just parsed, rejecting one that converts to no type in the
    /// annotation's type context.
    fn annotation_from(&mut self, head: Expr) -> Result<Expr, ParseError> {
        let expr = self.parse_expression_from(head, Precedence::Walrus)?;
        annotation_type(&expr)?;
        Ok(expr)
    }

    // --- Expressions (Pratt parser) ---
}

/// An annotation's type, converted in its type context; the `...` and
/// contextual sentinels are no annotation.
fn annotation_type(annotation: &Expr) -> Result<Type, ParseError> {
    annotation.annotation_type().ok_or_else(|| {
        ParseError::Message("expected a type, found an expression".to_string()).at(annotation.span)
    })
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

/// A type-parameter bound's expression, classified into the fields of
/// [`mojito_ast::ast::TypeParam`].
struct TypeParamBound {
    bounds: Vec<String>,
    value_type: Option<Expr>,
    callable_bound: Option<Expr>,
    origin_mutability: Option<Expr>,
}

/// Classifies a type-parameter bound's expression, converted in the type
/// context:
///
/// - an `&` composition is a list of trait names with at most one function
///   type (the callable bound), in any position;
/// - a lone function type is the callable bound;
/// - a lone name (a trait, `Self`, an origin alias) is the one bound;
/// - `Origin[mut=e]` is an origin binder of mutability `e`;
/// - any other type is the parameter's value type, beside its spelled head.
fn type_param_bound(expr: &Expr) -> Result<TypeParamBound, ParseError> {
    const FUNCTION_TYPE: &str = "<function type>";
    let mut members = Vec::new();
    bound_members(expr, &mut members);
    let mut bound = TypeParamBound {
        bounds: Vec::new(),
        value_type: None,
        callable_bound: None,
        origin_mutability: None,
    };
    if let [member] = members.as_slice() {
        let ty = bound_member_type(member)?;
        match &ty {
            Type::Func { .. } => {
                bound.bounds.push(FUNCTION_TYPE.to_string());
                bound.callable_bound = Some((*member).clone());
            }
            Type::Named(name, args) if args.is_empty() => bound.bounds.push(name.clone()),
            Type::SelfType => bound.bounds.push("Self".to_string()),
            Type::Named(name, args)
                if name == "Origin"
                    && let [mojito_ast::ast::ParamArg::Named { name: key, value }] =
                        args.as_slice()
                    && key == "mut"
                    && let mojito_ast::ast::ParamArg::Value(mutability) = value.as_ref() =>
            {
                bound.bounds.push(name.clone());
                bound.origin_mutability = Some(mutability.clone());
            }
            _ => {
                bound.bounds.push(bound_spelling(member));
                bound.value_type = Some((*member).clone());
            }
        }
        return Ok(bound);
    }
    for member in members {
        match bound_member_type(member)? {
            Type::Named(name, args) if args.is_empty() => bound.bounds.push(name),
            Type::Func { .. } if bound.callable_bound.is_none() => {
                bound.bounds.push(FUNCTION_TYPE.to_string());
                bound.callable_bound = Some(member.clone());
            }
            Type::Func { .. } => {
                return Err(ParseError::Message(
                    "trait composition must not contain more than one closure type".to_string(),
                )
                .at(member.span));
            }
            _ => {
                return Err(ParseError::Message(
                    "expected a trait name in a type-parameter bound composition".to_string(),
                )
                .at(member.span));
            }
        }
    }
    Ok(bound)
}

/// The members of an `&` chain, however nested, in source order; any other
/// expression is its own one member.
fn bound_members<'a>(expr: &'a Expr, members: &mut Vec<&'a Expr>) {
    if let ExprKind::Infix(InfixOp::BitAnd, left, right) = &expr.kind {
        bound_members(left, members);
        bound_members(right, members);
    } else {
        members.push(expr);
    }
}

/// One bound member converted in the type context.
fn bound_member_type(member: &Expr) -> Result<Type, ParseError> {
    member.annotation_type().ok_or_else(|| {
        ParseError::Message("expected a type, found an expression".to_string()).at(member.span)
    })
}

/// The spelled head recorded beside a value type: `Self.NAME`, the root
/// name of a qualified or applied type, `type_of`, `__generator_type`, or
/// `None`.
fn bound_spelling(expr: &Expr) -> String {
    if let Some(field) = expr.self_member() {
        return format!("Self.{field}");
    }
    match &expr.kind {
        ExprKind::Identifier(name) | ExprKind::Call { name, .. } => name.clone(),
        ExprKind::Member { object, .. }
        | ExprKind::Index { object, .. }
        | ExprKind::MultiIndex { object, .. } => bound_spelling(object),
        ExprKind::TypeValue(Type::Generator { .. }) => "__generator_type".to_string(),
        ExprKind::None => "None".to_string(),
        _ => "<type expression>".to_string(),
    }
}
