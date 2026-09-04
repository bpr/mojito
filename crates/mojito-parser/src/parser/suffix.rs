//! Postfix parsing: call/index/member bracket suffixes, slices,
//! comparisons, and call arguments.

use super::*;

impl<I: Iterator<Item = Result<(Token, Span), LexError>>> Parser<I> {
    pub(super) fn parse_bracket_suffix(
        &mut self,
        object: Expr,
        start: usize,
    ) -> Result<Expr, ParseError> {
        self.expect(Token::LBracket, "Expected '['")?;
        if matches!(self.peek_token()?, Some(Token::RBracket)) {
            self.next_token()?;
            // Empty brackets are the pointer-dereference subscript `p[]`. The
            // marker is distinct from `ExprKind::None` so `p[None]` stays an
            // ordinary (rejected) index expression.
            return Ok(self.node(
                ExprKind::Index {
                    object: Box::new(object),
                    index: Box::new(Expr::new(ExprKind::EmptySubscript, self.last_span)),
                },
                start,
            ));
        }

        let mut items = Vec::new();
        loop {
            if matches!(self.peek_token()?, Some(Token::Colon)) {
                let (upper, step, explicit_step) = self.parse_slice_components()?;
                items.push(ParsedBracketItem::Slice {
                    lower: None,
                    upper,
                    step,
                    explicit_step,
                });
            } else {
                let mut argument = self.parse_param_arg()?;
                if matches!(self.peek_token()?, Some(Token::Assign)) {
                    let name = param_argument_name(&argument)?;
                    self.next_token()?;
                    // A ':' directly after '=' is a keyword slice with an
                    // omitted lower bound (`s[byte=:b]`).
                    if matches!(self.peek_token()?, Some(Token::Colon)) {
                        let (upper, step, explicit_step) = self.parse_slice_components()?;
                        items.push(ParsedBracketItem::KeywordSlice {
                            name,
                            lower: None,
                            upper,
                            step,
                            explicit_step,
                        });
                        if !matches!(self.peek_token()?, Some(Token::Comma)) {
                            break;
                        }
                        self.next_token()?;
                        if matches!(self.peek_token()?, Some(Token::RBracket)) {
                            break;
                        }
                        continue;
                    }
                    argument = mojito_ast::ast::ParamArg::Named {
                        name,
                        value: Box::new(mojito_ast::ast::ParamArg::Value(
                            self.parse_expression(Precedence::Lowest)?,
                        )),
                    };
                }
                if matches!(self.peek_token()?, Some(Token::Colon)) {
                    match argument {
                        mojito_ast::ast::ParamArg::Value(value) => {
                            let (upper, step, explicit_step) = self.parse_slice_components()?;
                            items.push(ParsedBracketItem::Slice {
                                lower: Some(Box::new(value)),
                                upper,
                                step,
                                explicit_step,
                            });
                        }
                        // `s[byte=a:b]` — a keyword slice whose lower bound
                        // was parsed as the keyword argument's value.
                        mojito_ast::ast::ParamArg::Named { name, value }
                            if matches!(value.as_ref(), mojito_ast::ast::ParamArg::Value(_)) =>
                        {
                            let mojito_ast::ast::ParamArg::Value(lower) = *value else {
                                unreachable!("guard established a value argument");
                            };
                            let (upper, step, explicit_step) = self.parse_slice_components()?;
                            items.push(ParsedBracketItem::KeywordSlice {
                                name,
                                lower: Some(Box::new(lower)),
                                upper,
                                step,
                                explicit_step,
                            });
                        }
                        _ => {
                            return Err(ParseError::UnexpectedToken(
                                Token::Colon,
                                "a slice bound must be an expression".into(),
                            ));
                        }
                    }
                } else {
                    items.push(ParsedBracketItem::Param(argument));
                }
            }

            if !matches!(self.peek_token()?, Some(Token::Comma)) {
                break;
            }
            self.next_token()?;
            if matches!(self.peek_token()?, Some(Token::RBracket)) {
                break;
            }
        }
        self.expect(Token::RBracket, "Expected ']' after a subscript")?;

        let contains_slice = items.iter().any(|item| {
            matches!(
                item,
                ParsedBracketItem::Slice { .. } | ParsedBracketItem::KeywordSlice { .. }
            )
        });
        if matches!(self.peek_token()?, Some(Token::LParen)) {
            if contains_slice {
                return Err(ParseError::UnexpectedToken(
                    Token::LParen,
                    "slice expressions cannot be compile-time call parameters".into(),
                ));
            }
            let param_args = items
                .into_iter()
                .map(|item| match item {
                    ParsedBracketItem::Param(argument) => argument,
                    ParsedBracketItem::Slice { .. } | ParsedBracketItem::KeywordSlice { .. } => {
                        unreachable!()
                    }
                })
                .collect();
            self.next_token()?;
            let (args, kwargs) = self.parse_call_args()?;
            self.expect(Token::RParen, "Expected ')' after arguments")?;
            let kind = match object.kind {
                ExprKind::Identifier(name) => ExprKind::Call {
                    name,
                    param_args,
                    args,
                    kwargs,
                },
                _ => ExprKind::Invoke {
                    callee: Box::new(object),
                    param_args,
                    args,
                    kwargs,
                },
            };
            return Ok(self.node(kind, start));
        }

        if contains_slice {
            let mut arguments = Vec::with_capacity(items.len());
            for item in items {
                arguments.push(match item {
                    ParsedBracketItem::Param(mojito_ast::ast::ParamArg::Value(value)) => {
                        SubscriptArg::Index(value)
                    }
                    ParsedBracketItem::Param(_) => {
                        return Err(ParseError::UnexpectedToken(
                            Token::RBracket,
                            "a mixed subscript argument must be an expression".into(),
                        ));
                    }
                    ParsedBracketItem::Slice {
                        lower,
                        upper,
                        step,
                        explicit_step,
                    } => SubscriptArg::Slice {
                        lower,
                        upper,
                        step,
                        explicit_step,
                    },
                    ParsedBracketItem::KeywordSlice {
                        name,
                        lower,
                        upper,
                        step,
                        explicit_step,
                    } => SubscriptArg::KeywordSlice {
                        name,
                        lower,
                        upper,
                        step,
                        explicit_step,
                    },
                });
            }
            if let [
                SubscriptArg::Slice {
                    lower,
                    upper,
                    step,
                    explicit_step,
                },
            ] = arguments.as_slice()
            {
                return Ok(self.node(
                    ExprKind::Slice {
                        object: Box::new(object),
                        lower: lower.clone(),
                        upper: upper.clone(),
                        step: step.clone(),
                        explicit_step: *explicit_step,
                    },
                    start,
                ));
            }
            return Ok(self.node(
                ExprKind::MultiIndex {
                    object: Box::new(object),
                    args: arguments,
                },
                start,
            ));
        }

        let param_args: Vec<_> = items
            .into_iter()
            .map(|item| match item {
                ParsedBracketItem::Param(argument) => argument,
                ParsedBracketItem::Slice { .. } | ParsedBracketItem::KeywordSlice { .. } => {
                    unreachable!()
                }
            })
            .collect();

        // `reflect[T]` is the current Mojo reflection handle.  Unlike an
        // ordinary lower-case expression followed by one subscript, its
        // brackets carry a compile-time type argument and the resulting handle
        // is used directly (`reflect[T].field["name"]`).  Recognize the builtin
        // here so user-defined types such as `Point` do not make the expression
        // look like a runtime `reflect[Point]` index operation.
        if matches!(&object.kind, ExprKind::Identifier(name) if name == "reflect") {
            return Ok(self.node(
                ExprKind::TypeApply {
                    name: "reflect".to_string(),
                    args: param_args,
                },
                start,
            ));
        }

        // A named bracket argument over a lowercase (value) base is a keyword
        // subscript (`s[byte=i]`); over a capitalized type name
        // (`Origin[mut=True]`) it stays compile-time parameter application
        // via the TypeApply fallback below.
        if param_args
            .iter()
            .any(|argument| matches!(argument, mojito_ast::ast::ParamArg::Named { .. }))
            && param_args.iter().all(|argument| match argument {
                mojito_ast::ast::ParamArg::Named { value, .. } => {
                    matches!(value.as_ref(), mojito_ast::ast::ParamArg::Value(_))
                }
                mojito_ast::ast::ParamArg::Value(_) => true,
                mojito_ast::ast::ParamArg::Type(_) => false,
            })
            && expression_name_starts_lowercase(&object)
        {
            let args = param_args
                .into_iter()
                .map(|argument| match argument {
                    mojito_ast::ast::ParamArg::Named { name, value } => {
                        let mojito_ast::ast::ParamArg::Value(value) = *value else {
                            unreachable!("guard admits only value keyword arguments");
                        };
                        SubscriptArg::Keyword { name, value }
                    }
                    mojito_ast::ast::ParamArg::Value(value) => SubscriptArg::Index(value),
                    _ => unreachable!(),
                })
                .collect();
            return Ok(self.node(
                ExprKind::MultiIndex {
                    object: Box::new(object),
                    args,
                },
                start,
            ));
        }
        match <[_; 1]>::try_from(param_args) {
            // A bare value argument normally means runtime indexing, but
            // `origin_of(place)` is itself a compile-time Origin value.  Mojo
            // uses that spelling to specialize a function value without
            // immediately calling it (`var f = borrow[origin_of(value)]`).
            // Keep ordinary `values[index]` syntax on the Index path while
            // preserving this compiler-known origin argument as TypeApply.
            Ok([mojito_ast::ast::ParamArg::Value(origin)])
                if is_explicit_origin_argument(&origin)
                    && matches!(object.kind, ExprKind::Identifier(_)) =>
            {
                Ok(self.node(
                    ExprKind::TypeApply {
                        name: call_name(object)?,
                        args: vec![mojito_ast::ast::ParamArg::Value(origin)],
                    },
                    start,
                ))
            }
            Ok([mojito_ast::ast::ParamArg::Value(index)]) => Ok(self.node(
                ExprKind::Index {
                    object: Box::new(object),
                    index: Box::new(index),
                },
                start,
            )),
            // `None` parses as a type in a bracket, but over a lowercase
            // (value) base it is the `None` value (`table[None]` on a
            // `Dict[Optional[Int], _]`), not a parameter application.
            Ok([mojito_ast::ast::ParamArg::Type(Type::None)])
                if expression_name_starts_lowercase(&object) =>
            {
                let index = Expr::new(ExprKind::None, (start, self.last_span.1));
                Ok(self.node(
                    ExprKind::Index {
                        object: Box::new(object),
                        index: Box::new(index),
                    },
                    start,
                ))
            }
            // A `Self.o`-rooted binder (possibly projected —
            // `Self.o._get_owned_interior["element"]`) as the sole bracket
            // argument of a member subscript
            // (`Origin[mut=False].cast_from[...]`): the qualified struct
            // binder parses as a type/assoc chain, but a member object has no
            // parameterized-application form — re-materialize the chain as
            // the index expression.
            Ok([mojito_ast::ast::ParamArg::Type(binder_ty)])
                if !matches!(object.kind, ExprKind::Identifier(_))
                    && self_param_rooted_expression(&binder_ty).is_some() =>
            {
                let binder = self
                    .rematerialize_self_param_chain(&binder_ty, start)
                    .expect("guarded by self_param_rooted_expression");
                Ok(self.node(
                    ExprKind::Index {
                        object: Box::new(object),
                        index: Box::new(binder),
                    },
                    start,
                ))
            }
            Ok([other]) => Ok(self.node(
                ExprKind::TypeApply {
                    name: call_name(object)?,
                    args: vec![other],
                },
                start,
            )),
            // `origin_of(...)` has no runtime value, so a bracket list that
            // contains one cannot be a multi-dimensional subscript. This is
            // the multi-argument counterpart of the single-origin case above:
            // `choose[origin_of(left), origin_of(right)]` specializes a
            // callable, while `grid[row, column]` remains a runtime index.
            Err(param_args)
                if param_args.iter().any(|argument| {
                    matches!(argument, mojito_ast::ast::ParamArg::Value(value) if is_explicit_origin_argument(value))
                }) =>
            {
                Ok(self.node(
                    ExprKind::TypeApply {
                        name: call_name(object)?,
                        args: param_args,
                    },
                    start,
                ))
            }
            Err(param_args)
                if param_args.iter().all(|argument| {
                    matches!(
                        argument,
                        mojito_ast::ast::ParamArg::Value(_)
                            | mojito_ast::ast::ParamArg::Type(Type::None)
                    )
                }) && expression_name_starts_lowercase(&object) =>
            {
                let none_span = (start, self.last_span.1);
                let args = param_args
                    .into_iter()
                    .map(|argument| match argument {
                        mojito_ast::ast::ParamArg::Value(value) => SubscriptArg::Index(value),
                        mojito_ast::ast::ParamArg::Type(Type::None) => {
                            SubscriptArg::Index(Expr::new(ExprKind::None, none_span))
                        }
                        _ => unreachable!(),
                    })
                    .collect();
                Ok(self.node(
                    ExprKind::MultiIndex {
                        object: Box::new(object),
                        args,
                    },
                    start,
                ))
            }
            Err(param_args) => Ok(self.node(
                ExprKind::TypeApply {
                    name: call_name(object)?,
                    args: param_args,
                },
                start,
            )),
        }
    }

    /// Parse the tail after the first `:` of one slice item. Comma ends the item
    /// for a multi-dimensional subscript; a second colon is retained even when
    /// its step expression is omitted.
    pub(super) fn parse_slice_components(&mut self) -> Result<ParsedSliceTail, ParseError> {
        self.expect(Token::Colon, "Expected ':' in a slice")?;
        let upper = if matches!(
            self.peek_token()?,
            Some(Token::Colon | Token::Comma | Token::RBracket)
        ) {
            None
        } else {
            Some(Box::new(self.parse_expression(Precedence::Lowest)?))
        };
        let explicit_step = matches!(self.peek_token()?, Some(Token::Colon));
        let step = if explicit_step {
            self.next_token()?;
            if matches!(self.peek_token()?, Some(Token::Comma | Token::RBracket)) {
                None
            } else {
                Some(Box::new(self.parse_expression(Precedence::Lowest)?))
            }
        } else {
            None
        };
        Ok((upper, step, explicit_step))
    }

    /// Whether the next token begins a comparison operator (`== != < > <= >=`,
    /// `in`, `is`, or `not` — which in infix position can only start `not in`).
    pub(super) fn peek_is_comparison(&mut self) -> Result<bool, ParseError> {
        Ok(matches!(
            self.peek_token()?,
            Some(
                Token::EqEq
                    | Token::NotEq
                    | Token::Lt
                    | Token::Gt
                    | Token::Le
                    | Token::Ge
                    | Token::In
                    | Token::Is
                    | Token::Not
            )
        ))
    }

    /// Consume one comparison operator, resolving `not in` (two words).
    pub(super) fn parse_comparison_op(&mut self) -> Result<InfixOp, ParseError> {
        let op = match self.next_token()? {
            Token::EqEq => InfixOp::Eq,
            Token::NotEq => InfixOp::Ne,
            Token::Lt => InfixOp::Lt,
            Token::Gt => InfixOp::Gt,
            Token::Le => InfixOp::Le,
            Token::Ge => InfixOp::Ge,
            Token::In => InfixOp::In,
            // `is` / `is not` (two words): identity comparison dispatching to
            // `__is__` / `__isnot__` on the left operand.
            Token::Is => {
                if matches!(self.peek_token()?, Some(Token::Not)) {
                    self.next_token()?;
                    InfixOp::IsNot
                } else {
                    InfixOp::Is
                }
            }
            Token::Not => {
                self.expect(Token::In, "Expected 'in' after 'not' in a membership test")?;
                InfixOp::NotIn
            }
            other => {
                return Err(ParseError::UnexpectedToken(
                    other,
                    "Expected a comparison operator".into(),
                ));
            }
        };
        Ok(op)
    }

    /// Precedence of whatever operator is next, or `Lowest` if the next token
    /// does not continue an expression (so the climbing loop stops).
    pub(super) fn peek_precedence(&mut self) -> Result<Precedence, ParseError> {
        let prec = match self.peek_token()? {
            Some(Token::ColonEq) => Precedence::Walrus,
            // `if` in infix position (after an operand) begins a ternary.
            Some(Token::If) => Precedence::Conditional,
            Some(Token::Or) => Precedence::Or,
            Some(Token::And) => Precedence::And,
            Some(Token::EqEq | Token::NotEq | Token::Lt | Token::Gt | Token::Le | Token::Ge) => {
                Precedence::Comparison
            }
            // Membership `in` / `not in` and identity `is` / `is not` share
            // comparison precedence. In infix position (after an operand)
            // `not` can only start `not in`.
            Some(Token::In | Token::Not | Token::Is) => Precedence::Comparison,
            Some(
                Token::Plus
                | Token::Minus
                | Token::Shl
                | Token::Shr
                | Token::Amp
                | Token::Pipe
                | Token::Caret,
            ) => Precedence::Sum,
            Some(Token::Star | Token::Slash | Token::DoubleSlash | Token::Percent | Token::At) => {
                Precedence::Product
            }
            Some(Token::DoubleStar) => Precedence::Power,
            // `[` begins an explicit compile-time parameter list on a call.
            Some(Token::LParen | Token::LBracket | Token::Dot) => Precedence::Call,
            _ => Precedence::Lowest,
        };
        Ok(prec)
    }

    /// Parses call arguments: positional expressions and keyword arguments. Both
    /// the older Python-like `name=value` spelling and Mojo's `name: value`
    /// spelling are accepted and represented as [`KwArg`]. A positional argument
    /// may not follow a keyword one.
    pub(super) fn parse_call_args(&mut self) -> Result<(Vec<Expr>, Vec<KwArg>), ParseError> {
        let mut args = Vec::new();
        let mut kwargs = Vec::new();
        if matches!(self.peek_token()?, Some(Token::RParen)) {
            return Ok((args, kwargs));
        }
        loop {
            if matches!(self.peek_token()?, Some(Token::DoubleStar)) {
                self.next_token()?;
                let value = self.parse_expression(Precedence::Lowest)?;
                if !matches!(&value.kind, ExprKind::Transfer(_)) {
                    return Err(ParseError::UnexpectedToken(
                        Token::DoubleStar,
                        "keyword forwarding requires a transferred StringDict (`**kwargs^`)".into(),
                    ));
                }
                kwargs.push(KwArg {
                    name: mojito_ast::ast::FORWARDED_KWARGS_NAME.to_string(),
                    value,
                });
                if matches!(self.peek_token()?, Some(Token::Comma)) {
                    self.next_token()?;
                    if !matches!(self.peek_token()?, Some(Token::RParen)) {
                        return Err(ParseError::UnexpectedToken(
                            Token::Comma,
                            "`**kwargs^` must be the final call argument".into(),
                        ));
                    }
                }
                break;
            }
            let expr = if matches!(self.peek_token()?, Some(Token::Star)) {
                let start = self.peek_start();
                self.next_token()?;
                let value = self.parse_expression(Precedence::Lowest)?;
                self.node(ExprKind::Spread(Box::new(value)), start)
            } else {
                self.parse_expression(Precedence::Lowest)?
            };
            if let ExprKind::Identifier(name) = &expr.kind
                && matches!(self.peek_token()?, Some(Token::Assign) | Some(Token::Colon))
            {
                self.next_token()?; // consume '=' or ':'
                let value = self.parse_expression(Precedence::Lowest)?;
                kwargs.push(KwArg {
                    name: name.clone(),
                    value,
                });
            } else {
                if !kwargs.is_empty() {
                    return Err(ParseError::UnexpectedToken(
                        Token::Comma,
                        "a positional argument cannot follow a keyword argument".into(),
                    ));
                }
                args.push(expr);
            }
            if matches!(self.peek_token()?, Some(Token::Comma)) {
                self.next_token()?; // consume ','
                if matches!(self.peek_token()?, Some(Token::RParen)) {
                    break; // trailing comma
                }
            } else {
                break;
            }
        }
        Ok((args, kwargs))
    }

    /// Ensures a statement is cleanly terminated by a Newline or EOF
    pub(super) fn expect_stmt_end(&mut self) -> Result<(), ParseError> {
        let token = self.next_token()?;
        match token {
            Token::Semicolon => {
                self.last_stmt_ended_with_semicolon = true;
                Ok(())
            }
            Token::Newline | Token::Eof => {
                self.last_stmt_ended_with_semicolon = false;
                Ok(())
            }
            _ => Err(ParseError::UnexpectedToken(
                token,
                "Expected newline, ';', or EOF at the end of statement".into(),
            )),
        }
    }
}
