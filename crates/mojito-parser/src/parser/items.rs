//! Item parsing: parameter lists, structs, traits, and methods.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl<I: Iterator<Item = Result<(Token, Span), LexError>>> Parser<I> {
    /// Parses a (possibly empty) comma-separated parameter list. The opening
    /// `(` has been consumed; stops at the closing `)` without consuming it.
    /// Parses a parameter list (after the `(`), returning the parameters plus the
    /// positions of the `/` (positional-only) and bare `*` (keyword-only) markers.
    /// Supports every Mojo parameter form — conventions, defaults, `*args`,
    /// `var **kwargs`, and the `/`/`*` markers — all **parsed** (the checker flags the
    /// advanced ones as unsupported). Parsing is lenient about argument ordering.
    pub(super) fn parse_params(&mut self) -> Result<ParamList, ParseError> {
        self.parse_param_list(false, false)
    }

    /// The optional argument convention (`read`/`mut`/`var`/`out`) prefixing a
    /// regular parameter, plus its name. A convention word is only a convention
    /// when followed by the parameter name (another identifier); if it is followed
    /// by `:` it *is* the name (so `read` remains usable as a parameter name).
    pub(super) fn parse_convention_and_name(
        &mut self,
    ) -> Result<
        (
            Option<ArgConvention>,
            Option<mojito_ast::ast::OriginSpec>,
            String,
        ),
        ParseError,
    > {
        let word = if matches!(self.peek_token()?, Some(Token::Var)) {
            self.next_token()?;
            "var".to_string()
        } else {
            self.expect_identifier("Expected a parameter name")?
        };
        // `word :` → `word` is the parameter name, no convention.
        if matches!(self.peek_token()?, Some(Token::Colon)) {
            return Ok((None, None, word));
        }
        let Some(convention) = (if word == "var" {
            Some(ArgConvention::Var)
        } else {
            convention_word(&word)
        }) else {
            if let Some(error) = removed_convention_error(&word) {
                return Err(error);
            }
            return Err(ParseError::UnexpectedToken(
                Token::Identifier(word),
                "expected a parameter name (or a convention: imm/mut/var/out/ref)".into(),
            ));
        };
        // A `ref` convention may carry an origin specifier: `ref[origin] name`.
        let origin = if convention == ArgConvention::Ref {
            self.parse_optional_origin_specifier()?
        } else {
            None
        };
        let name = self.expect_identifier("Expected a parameter name after the convention")?;
        Ok((Some(convention), origin, name))
    }

    /// An optional `[origin]` origin specifier following `ref` (in a `ref[origin]`
    /// argument convention or `ref[origin] T` return type). The specifier is a
    /// comma-separated list of origin expressions (an arbitrary expression, a named
    /// origin, or `_`); it is retained for semantic resolution by the checker.
    pub(super) fn parse_optional_origin_specifier(
        &mut self,
    ) -> Result<Option<mojito_ast::ast::OriginSpec>, ParseError> {
        if !matches!(self.peek_token()?, Some(Token::LBracket)) {
            return Ok(None);
        }
        self.next_token()?; // consume '['
        let mut origins = Vec::new();
        loop {
            origins.push(self.parse_expression(Precedence::Lowest)?);
            if matches!(self.peek_token()?, Some(Token::Comma)) {
                self.next_token()?;
            } else {
                break;
            }
        }
        self.expect(&Token::RBracket, "Expected ']' after the origin specifier")?;
        Ok(Some(origins))
    }

    /// Finishes a parameter after its name: `: type [= default]`.
    pub(super) fn finish_param(
        &mut self,
        name: String,
        kind: ParamKind,
        convention: Option<ArgConvention>,
        origin: Option<mojito_ast::ast::OriginSpec>,
    ) -> Result<FnParam, ParseError> {
        self.expect(&Token::Colon, "Parameters require a type annotation")?;
        let ty = self.parse_type()?;
        let default = if matches!(self.peek_token()?, Some(Token::Assign)) {
            self.next_token()?; // consume '='
            Some(self.parse_expression(Precedence::Lowest)?)
        } else {
            None
        };
        Ok(FnParam {
            name,
            ty,
            default,
            kind,
            convention,
            origin,
        })
    }

    /// `[@fieldwise_init] struct Name: <fields and methods>`, or `enum
    /// Name: <cases and methods>`, an ordinary struct whose members the
    /// compiler generates from its cases (upstream's `parseStructStmt`
    /// parses both, telling them apart by `isEnum`).
    pub(super) fn parse_struct(
        &mut self,
        decorators: Vec<Decorator>,
    ) -> Result<StmtKind, ParseError> {
        // `@fieldwise_init` generates the constructor; the checker validates
        // every decorator.
        let fieldwise_init = decorators
            .iter()
            .any(|d| d.path.len() == 1 && d.path[0] == "fieldwise_init");
        let is_enum = matches!(self.peek_token()?, Some(Token::Enum));
        let start = self.peek_start();
        if is_enum && fieldwise_init {
            return Err(ParseError::Message(
                "'@fieldwise_init' cannot be applied to an enum".to_string(),
            )
            .at((start, start)));
        }

        // The lexer emits `enum` only before a name.
        let name = if is_enum {
            self.next_token()?;
            self.expect_identifier("expected enum name")?
        } else {
            self.expect(&Token::Struct, "Expected 'struct'")?;
            self.expect_identifier("Expected a struct name after 'struct'")?
        };
        let type_params = self.parse_type_params()?;
        let (conforms, conformance_conditions, callable_conformance) =
            self.parse_struct_conformance()?;
        let where_clauses = self.parse_where_clauses()?;
        if is_enum {
            if !matches!(self.peek_token()?, Some(Token::Colon)) {
                return Err(
                    ParseError::Message("expected ':' in enum definition".to_string())
                        .at((start, self.last_span.1)),
                );
            }
            self.next_token()?;
        } else {
            self.expect(&Token::Colon, "Expected ':' after the struct name")?;
        }
        self.expect_stmt_end()?;

        // Body: an indented block of `var` fields (a struct) or `case`
        // declarations (an enum), `comptime` associated facts, and `def`
        // methods.
        self.expect(&Token::Indent, "Expected an indented struct body")?;
        let mut fields = Vec::new();
        let mut associated = Vec::new();
        let mut methods = Vec::new();
        let mut cases = Vec::new();
        while let Some(token) = self.peek_token()? {
            match token {
                Token::Dedent => {
                    self.next_token()?;
                    break;
                }
                Token::Newline => {
                    self.next_token()?;
                }
                Token::TripleStringLiteral(..) => {
                    self.next_token()?;
                    self.expect_stmt_end()?;
                }
                Token::Var if is_enum => {
                    let at = self.peek_start();
                    return Err(ParseError::Message(
                        "enums do not support 'var' fields; store state in a case payload"
                            .to_string(),
                    )
                    .at((at, at)));
                }
                Token::Var => {
                    self.expect(&Token::Var, "Expected 'var'")?;
                    let fname = self.expect_identifier("Expected a field name")?;
                    self.expect(&Token::Colon, "Fields require a type annotation")?;
                    let ty = self.parse_type()?;
                    if matches!(self.peek_token()?, Some(Token::Assign)) {
                        self.next_token()?;
                        self.parse_expression(Precedence::Lowest)?;
                    }
                    self.expect_stmt_end()?;
                    fields.push(Param { name: fname, ty });
                }
                Token::Case if is_enum => cases.push(self.parse_enum_case()?),
                Token::Enum => {
                    let at = self.peek_start();
                    return Err(
                        ParseError::Message("nested enum not supported here".to_string())
                            .at((at, at)),
                    );
                }
                Token::Pass => {
                    self.next_token()?;
                    self.expect_stmt_end()?;
                }
                Token::Comptime => associated.push(self.parse_struct_comptime()?),
                Token::Def => methods.push(self.parse_member_method(Vec::new(), is_enum)?),
                // Decorators before a method (`@staticmethod`, …).
                Token::At => {
                    let at = self.peek_start();
                    let decos = self.parse_decorators()?;
                    match self.peek_token()? {
                        Some(Token::Def) => {
                            methods.push(self.parse_member_method(decos, is_enum)?);
                        }
                        Some(Token::Case) if is_enum => {
                            return Err(ParseError::Message(
                                "'case' statement does not support decorators; remove the decorator"
                                    .to_string(),
                            )
                            .at((at, at)));
                        }
                        other => {
                            return Err(ParseError::UnexpectedToken(
                                other.cloned().unwrap_or(Token::Eof),
                                "a decorator in a struct body must precede a 'def' method".into(),
                            ));
                        }
                    }
                }
                other => {
                    return Err(ParseError::UnexpectedToken(
                        other.clone(),
                        if is_enum {
                            "enum body may only contain 'case' declarations, 'comptime' associated facts, and 'def' methods"
                        } else {
                            "struct body may only contain 'var' fields, 'comptime' associated facts, and 'def' methods"
                        }
                        .into(),
                    ));
                }
            }
        }

        Ok(StmtKind::Struct {
            name,
            decorators,
            type_params,
            conforms,
            callable_conformance,
            conformance_conditions,
            where_clauses,
            fields,
            associated,
            methods,
            fieldwise_init,
            enum_cases: is_enum.then_some(cases),
        })
    }

    /// `comptime NAME = expr` — an associated compile-time fact inside a struct.
    pub(super) fn parse_struct_comptime(
        &mut self,
    ) -> Result<mojito_ast::ast::StructComptime, ParseError> {
        self.expect(&Token::Comptime, "Expected 'comptime'")?;
        let name = self.expect_identifier("Expected a name after 'comptime'")?;
        if name == "assert" {
            return Err(comptime_assert_outside_function());
        }
        // A parameterized associated type: `comptime IteratorType[params] = ...`.
        let params = if matches!(self.peek_token()?, Some(Token::LBracket)) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };
        let ty = if matches!(self.peek_token()?, Some(Token::Colon)) {
            self.next_token()?;
            Some(self.parse_type()?)
        } else {
            None
        };
        let where_clauses = self.parse_where_clauses()?;
        self.expect(
            &Token::Assign,
            "Expected '=' after the comptime member name",
        )?;
        let value = self.parse_expression(Precedence::Lowest)?;
        self.expect_stmt_end()?;
        Ok(mojito_ast::ast::StructComptime {
            name,
            params,
            ty,
            where_clauses,
            value,
        })
    }

    /// Parses an optional trait-conformance list `'(' NAME (',' NAME)* ')'`
    /// following a `struct` name. Returns an empty list if the next token is not
    /// `(`. Used for `struct Duck(Copyable, Quackable):`.
    pub(super) fn parse_conformance(&mut self) -> Result<Vec<String>, ParseError> {
        if !matches!(self.peek_token()?, Some(Token::LParen)) {
            return Ok(Vec::new());
        }
        self.next_token()?; // consume '('
        let mut traits = Vec::new();
        loop {
            let trait_name =
                self.expect_identifier("Expected a trait name in the conformance list")?;
            traits.push(trait_name);
            if matches!(self.peek_token()?, Some(Token::Comma)) {
                self.next_token()?; // consume ','
                if matches!(self.peek_token()?, Some(Token::RParen)) {
                    break;
                }
            } else {
                break;
            }
        }
        self.expect(&Token::RParen, "Expected ')' after the conformance list")?;
        Ok(traits)
    }

    /// Current Mojo permits a predicate after an individual struct conformance:
    /// `Trait where conforms_to(T, Trait)`. Conditions are retained separately
    /// while the nominal trait-name list remains compatible with existing passes.
    pub(super) fn parse_struct_conformance(&mut self) -> Result<StructConformanceList, ParseError> {
        if !matches!(self.peek_token()?, Some(Token::LParen)) {
            return Ok((Vec::new(), Vec::new(), None));
        }
        self.next_token()?;
        let mut traits = Vec::new();
        let mut conditions = Vec::new();
        let mut callable = None;
        loop {
            if matches!(self.peek_token()?, Some(Token::Def)) {
                if callable.is_some() {
                    return Err(ParseError::UnexpectedToken(
                        Token::Def,
                        "a struct may declare only one def(...) callable conformance".into(),
                    ));
                }
                callable = Some(self.parse_type()?);
                if matches!(self.peek_token()?, Some(Token::Comma)) {
                    self.next_token()?;
                    if matches!(self.peek_token()?, Some(Token::RParen)) {
                        break;
                    }
                    continue;
                }
                break;
            }
            let trait_name =
                self.expect_identifier("Expected a trait name in the conformance list")?;
            traits.push(trait_name.clone());
            if matches!(self.peek_token()?, Some(Token::Identifier(word)) if word == "where") {
                self.next_token()?;
                let condition = self.parse_expression(Precedence::Lowest)?;
                conditions.push((trait_name, condition));
            }
            if matches!(self.peek_token()?, Some(Token::Comma)) {
                self.next_token()?;
                if matches!(self.peek_token()?, Some(Token::RParen)) {
                    break;
                }
            } else {
                break;
            }
        }
        self.expect(&Token::RParen, "Expected ')' after the conformance list")?;
        Ok((traits, conditions, callable))
    }

    /// `trait Name[(Super, …)]: <members>` — a trait, optionally **refining**
    /// super-traits (`trait Bird(Animal):`, reusing the conformance-list parser).
    /// The body holds `def` method requirements (`...`) or default methods (a real
    /// body), and `comptime NAME: Type [= default]` member requirements. (Generic
    /// traits `trait T[U]:` are not valid current Mojo, so no `[type_params]` is
    /// parsed.)
    pub(super) fn parse_trait(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(&Token::Trait, "Expected 'trait'")?;
        let name = self.expect_identifier("Expected a trait name after 'trait'")?;
        let refines = self.parse_conformance()?;
        self.expect(&Token::Colon, "Expected ':' after the trait name")?;
        self.expect_stmt_end()?;

        self.expect(&Token::Indent, "Expected an indented trait body")?;
        let mut methods = Vec::new();
        let mut comptime_members = Vec::new();
        while let Some(token) = self.peek_token()? {
            match token {
                Token::Dedent => {
                    self.next_token()?;
                    break;
                }
                Token::Newline => {
                    self.next_token()?;
                }
                Token::TripleStringLiteral(..) | Token::Pass => {
                    self.next_token()?;
                    self.expect_stmt_end()?;
                }
                Token::Def => methods.push(self.parse_trait_method(Vec::new())?),
                Token::At => {
                    let decorators = self.parse_decorators()?;
                    if !matches!(self.peek_token()?, Some(Token::Def)) {
                        return Err(ParseError::UnexpectedToken(
                            self.peek_token()?.cloned().unwrap_or(Token::Eof),
                            "a decorator in a trait body must precede a 'def' method".into(),
                        ));
                    }
                    methods.push(self.parse_trait_method(decorators)?);
                }
                Token::Comptime => comptime_members.push(self.parse_trait_comptime()?),
                Token::Enum => {
                    let at = self.peek_start();
                    return Err(ParseError::Message(
                        "nested enum in a trait not supported here".to_string(),
                    )
                    .at((at, at)));
                }
                other => {
                    return Err(ParseError::UnexpectedToken(
                        other.clone(),
                        "a trait body may only contain 'def' methods, 'comptime' members, or 'pass'"
                            .into(),
                    ));
                }
            }
        }
        Ok(StmtKind::Trait {
            name,
            refines,
            methods,
            comptime_members,
        })
    }

    /// `comptime NAME: Type [= default]` — a compile-time member requirement
    /// inside a trait.
    pub(super) fn parse_trait_comptime(
        &mut self,
    ) -> Result<mojito_ast::ast::TraitComptime, ParseError> {
        self.expect(&Token::Comptime, "Expected 'comptime'")?;
        let name = self.expect_identifier("Expected a name after 'comptime'")?;
        if name == "assert" {
            return Err(comptime_assert_outside_function());
        }
        // A parameterized associated type requirement:
        // `comptime IteratorType[iterable_mut: Bool, //, iterable_origin:
        // Origin[mut=iterable_mut]]: Iterator`.
        let params = if matches!(self.peek_token()?, Some(Token::LBracket)) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };
        self.expect(&Token::Colon, "Expected ':' after the comptime member name")?;
        let first = self.parse_type()?;
        let mut bounds = vec![first];
        while matches!(self.peek_token()?, Some(Token::Amp)) {
            self.next_token()?;
            bounds.push(self.parse_type()?);
        }
        let ty = if bounds.len() == 1 {
            bounds.pop().expect("one associated member annotation")
        } else {
            mojito_ast::ast::Type::Named(
                "$trait_composition".to_string(),
                bounds
                    .into_iter()
                    .map(mojito_ast::ast::ParamArg::Type)
                    .collect(),
            )
        };
        let where_clauses = self.parse_where_clauses()?;
        let default = if matches!(self.peek_token()?, Some(Token::Assign)) {
            self.next_token()?;
            Some(self.parse_expression(Precedence::Lowest)?)
        } else {
            None
        };
        self.expect_stmt_end()?;
        Ok(mojito_ast::ast::TraitComptime {
            name,
            params,
            ty,
            where_clauses,
            default,
        })
    }

    /// `def name([convention] self [, params]) -> ret:` followed by an indented
    /// body that is either `...` (a pure requirement) or real statements (a
    /// **default implementation**, stored in `default_body`), after the
    /// `decorators` already parsed before it.
    pub(super) fn parse_trait_method(
        &mut self,
        decorators: Vec<Decorator>,
    ) -> Result<mojito_ast::ast::TraitMethod, ParseError> {
        self.expect(&Token::Def, "Expected 'def'")?;
        let name = {
            let spelled = self.expect_member_name("Expected a method name after 'def'")?;
            mojito_ast::ast::canonical_destructor_name(&spelled).to_string()
        };
        let type_params = self.parse_type_params()?;

        self.expect(&Token::LParen, "Expected '(' after the method name")?;
        let first = if matches!(self.peek_token()?, Some(Token::Var)) {
            self.next_token()?;
            "var".to_string()
        } else {
            self.expect_identifier("A method's first parameter must be 'self'")?
        };
        let explicit = if first == "var" {
            Some(ArgConvention::Var)
        } else {
            convention_word(&first)
        };
        if explicit.is_none()
            && let Some(error) = removed_convention_error(&first)
        {
            return Err(error);
        }
        let (self_name, self_convention, self_origin) = if let Some(conv) = explicit {
            let origin = if conv == ArgConvention::Ref {
                self.parse_optional_origin_specifier()?
            } else {
                None
            };
            (
                self.expect_identifier("Expected 'self' after the receiver convention")?,
                Some(conv),
                origin,
            )
        } else {
            (first, None, None)
        };
        if self_name != "self" {
            return Err(ParseError::UnexpectedToken(
                Token::Identifier(self_name),
                "a method's first parameter must be 'self'".into(),
            ));
        }
        let self_type = self.parse_optional_receiver_type()?;
        let ParamList {
            params,
            positional_only,
            keyword_only,
        } = if matches!(self.peek_token()?, Some(Token::Comma)) {
            self.next_token()?; // consume ','
            self.parse_params()?
        } else {
            ParamList {
                params: Vec::new(),
                positional_only: None,
                keyword_only: None,
            }
        };
        self.expect(&Token::RParen, "Expected ')' after the parameters")?;

        let (raises, raises_type) = self.parse_callable_effects()?;
        let ret = if matches!(self.peek_token()?, Some(Token::Arrow)) {
            self.next_token()?;
            Some(self.parse_type()?)
        } else {
            None
        };

        if matches!(self.peek_token()?, Some(Token::LBrace)) {
            self.next_token()?;
            while !matches!(self.peek_token()?, Some(Token::RBrace) | None) {
                self.next_token()?;
            }
            self.expect(&Token::RBrace, "Expected '}' after method effects")?;
        }
        let where_clauses = self.parse_where_clauses()?;
        self.expect(&Token::Colon, "Expected ':' before the method body")?;
        // A body of exactly `...` is a pure requirement; anything else is a
        // default implementation (parsed, flagged unsupported by the checker).
        let default_body = self.parse_trait_method_body()?;

        Ok(mojito_ast::ast::TraitMethod {
            name,
            decorators,
            type_params,
            self_convention,
            self_origin,
            self_type,
            params,
            positional_only,
            keyword_only,
            raises,
            raises_type,
            ret,
            where_clauses,
            default_body,
        })
    }

    /// `def name([convention] self [, params]) -> ret: <block>` inside a struct.
    pub(super) fn parse_method(
        &mut self,
        decorators: Vec<Decorator>,
    ) -> Result<Method, ParseError> {
        self.expect(&Token::Def, "Expected 'def'")?;
        let name = {
            let spelled = self.expect_member_name("Expected a method name after 'def'")?;
            mojito_ast::ast::canonical_destructor_name(&spelled).to_string()
        };
        let type_params = self.parse_type_params()?;

        self.expect(&Token::LParen, "Expected '(' after the method name")?;
        // Every `__init__` is static, its `out self` the result, so
        // `@staticmethod` on one changes nothing.
        let is_static = name != "__init__"
            && decorators
                .iter()
                .any(|decorator| decorator.path.as_slice() == ["staticmethod"]);
        // Detect the receiver. An instance method starts with `self`, optionally
        // carrying a convention (`mut self`, `out self`, `var self`, `imm self`
        // — convention words are contextual identifiers). A `@staticmethod` has
        // no receiver, so its parameters start immediately even when the first
        // one has a convention, notably canonical `var **kwargs`; a leading
        // `self` there is an ordinary parameter.
        let first_is_self =
            matches!(self.peek_token()?, Some(Token::Identifier(id)) if id == "self");
        let first_is_convention = matches!(self.peek_token()?, Some(Token::Identifier(id)) if convention_word(id).is_some())
            || matches!(self.peek_token()?, Some(Token::Var));
        let is_initializer = name == "__init__";
        let (receiver, list) = if is_static {
            (None, self.parse_param_list(true, false)?)
        } else if first_is_self {
            self.next_token()?; // consume 'self'
            self.parse_leading_receiver(None, None)?
        } else if first_is_convention {
            let conv = match self.peek_token()? {
                Some(Token::Identifier(id)) => convention_word(id),
                Some(Token::Var) => Some(ArgConvention::Var),
                _ => None,
            };
            // `ref self` may carry an origin specifier: `ref[origin] self`.
            self.next_token()?; // consume the convention word
            let origin = if conv == Some(ArgConvention::Ref) {
                self.parse_optional_origin_specifier()?
            } else {
                None
            };
            let first_name =
                self.expect_identifier("Expected 'self' after the receiver convention")?;
            if first_name == "self" {
                self.parse_leading_receiver(conv, origin)?
            } else if is_initializer {
                // An initializer's `out self` may follow other arguments,
                // whose names its annotation then reads.
                let leading = self.finish_param(first_name, ParamKind::Regular, conv, origin)?;
                let mut list = if matches!(self.peek_token()?, Some(Token::Comma)) {
                    self.next_token()?; // consume ','
                    self.parse_param_list(false, true)?
                } else {
                    ParamList {
                        params: Vec::new(),
                        positional_only: None,
                        keyword_only: None,
                    }
                };
                list.params.insert(0, leading);
                list.positional_only = list.positional_only.map(|index| index + 1);
                list.keyword_only = list.keyword_only.map(|index| index + 1);
                split_initializer_receiver(list)?
            } else {
                return Err(ParseError::UnexpectedToken(
                    Token::Identifier(first_name),
                    "a receiver convention must be followed by 'self'".into(),
                ));
            }
        } else if is_initializer {
            let list = self.parse_param_list(false, true)?;
            split_initializer_receiver(list)?
        } else {
            // No receiver — a static method.
            (None, self.parse_param_list(false, false)?)
        };
        let ParamList {
            params,
            positional_only,
            keyword_only,
        } = list;
        let (has_self, self_convention, self_origin, self_type, receiver_index) =
            receiver.map_or((false, None, None, None, 0), |receiver| {
                (
                    true,
                    receiver.convention,
                    receiver.origin,
                    receiver.ty,
                    receiver.index,
                )
            });
        self.expect(&Token::RParen, "Expected ')' after the parameters")?;

        let (raises, raises_type) = self.parse_callable_effects()?;
        let ret = if matches!(self.peek_token()?, Some(Token::Arrow)) {
            self.next_token()?;
            Some(self.parse_type()?)
        } else {
            None
        };

        let where_clauses = self.parse_where_clauses()?;

        self.expect(&Token::Colon, "Expected ':' before the method body")?;
        let body = self.parse_suite()?;

        Ok(Method {
            name,
            type_params,
            has_self,
            self_convention,
            self_origin,
            self_type,
            receiver_index,
            decorators,
            params,
            positional_only,
            keyword_only,
            raises,
            raises_type,
            ret,
            where_clauses,
            body,
            provenance: mojito_ast::ast::MethodProvenance::Source,
        })
    }

    /// Parse callable ABI effects whose execution meaning is already represented
    /// by the selected callable type/environment. `raises` is retained separately;
    /// `capturing`, `thin`, and `abi(...)` need no additional declaration field.
    pub(super) fn parse_erased_callable_effects(&mut self) -> Result<(), ParseError> {
        while let Some(Token::Identifier(effect)) = self.peek_token()?.cloned() {
            match effect.as_str() {
                "capturing" | "thin" => {
                    self.next_token()?;
                    if effect == "capturing" && matches!(self.peek_token()?, Some(Token::LBracket))
                    {
                        self.next_token()?;
                        while !matches!(self.peek_token()?, Some(Token::RBracket) | None) {
                            self.next_token()?;
                        }
                        self.expect(&Token::RBracket, "Expected ']' after capturing origins")?;
                    }
                }
                "abi" => {
                    self.next_token()?;
                    self.expect(&Token::LParen, "Expected '(' after abi")?;
                    while !matches!(self.peek_token()?, Some(Token::RParen) | None) {
                        self.next_token()?;
                    }
                    self.expect(&Token::RParen, "Expected ')' after abi")?;
                }
                _ => break,
            }
        }
        Ok(())
    }
}

impl<I: Iterator<Item = Result<(Token, Span), LexError>>> Parser<I> {
    /// A parameter list whose leading `self` may omit its annotation, as a
    /// `@staticmethod`'s may: upstream types a method's first `self`
    /// argument `Self` whether or not it is a receiver. An `out self` names
    /// the result, so it still needs one outside an `__init__`; with
    /// `initializer`, an `out self` anywhere in the list may omit it.
    fn parse_param_list(
        &mut self,
        self_typed_leading: bool,
        initializer: bool,
    ) -> Result<ParamList, ParseError> {
        let mut params = Vec::new();
        let mut positional_only = None;
        let mut keyword_only = None;
        if matches!(self.peek_token()?, Some(Token::RParen)) {
            return Ok(ParamList {
                params,
                positional_only,
                keyword_only,
            });
        }
        loop {
            match self.peek_token()? {
                // `/` — positional-only marker (not a parameter).
                Some(Token::Slash) => {
                    self.next_token()?;
                    positional_only = Some(params.len());
                }
                // Current Mojo requires the consuming `var **name: T` spelling
                // for a keyword-variadic collector.
                Some(Token::DoubleStar) => {
                    return Err(ParseError::UnexpectedToken(
                        self.next_token()?,
                        "a keyword-variadic parameter must be spelled 'var **name: Type'"
                            .to_string(),
                    ));
                }
                Some(Token::Star) => {
                    self.next_token()?;
                    if matches!(self.peek_token()?, Some(Token::Identifier(_))) {
                        // `*name: T` — positional variadic.
                        let name = self.expect_identifier("Expected a name after '*'")?;
                        params.push(self.finish_param(name, ParamKind::Variadic, None, None)?);
                    } else {
                        // bare `*` — keyword-only marker (not a parameter).
                        keyword_only = Some(params.len());
                    }
                }
                // Current Mojo places the ownership convention before the pack
                // marker: `var *args: *Ts` (not `*var args`).
                Some(Token::Var) => {
                    self.next_token()?;
                    if matches!(self.peek_token()?, Some(Token::DoubleStar)) {
                        self.next_token()?;
                        let name = self.expect_identifier("Expected a name after 'var **'")?;
                        params.push(self.finish_param(
                            name,
                            ParamKind::KwVariadic,
                            Some(ArgConvention::Var),
                            None,
                        )?);
                    } else if matches!(self.peek_token()?, Some(Token::Star)) {
                        self.next_token()?;
                        let name = self.expect_identifier("Expected a name after 'var *'")?;
                        params.push(self.finish_param(
                            name,
                            ParamKind::Variadic,
                            Some(ArgConvention::Var),
                            None,
                        )?);
                    } else {
                        let name = self
                            .expect_identifier("Expected a parameter name after the convention")?;
                        let self_typed = self_typed_leading && params.is_empty();
                        params.push(self.finish_leading_param(
                            name,
                            Some(ArgConvention::Var),
                            None,
                            self_typed,
                        )?);
                    }
                }
                // A regular parameter, with an optional convention prefix.
                _ => {
                    let self_typed = self_typed_leading && params.is_empty();
                    let (convention, origin, name) = if self_typed
                        && matches!(self.peek_token()?, Some(Token::Identifier(id)) if id == "self")
                    {
                        (None, None, self.expect_identifier("Expected 'self'")?)
                    } else {
                        self.parse_convention_and_name()?
                    };
                    // An unannotated `out self` is typed `Self`.
                    if initializer
                        && name == "self"
                        && convention == Some(ArgConvention::Out)
                        && !matches!(self.peek_token()?, Some(Token::Colon))
                    {
                        params.push(FnParam {
                            name,
                            ty: Type::SelfType,
                            default: None,
                            kind: ParamKind::Regular,
                            convention,
                            origin,
                        });
                    } else {
                        params
                            .push(self.finish_leading_param(name, convention, origin, self_typed)?);
                    }
                }
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
        Ok(ParamList {
            params,
            positional_only,
            keyword_only,
        })
    }

    /// Finishes a regular parameter, typing an unannotated leading `self`
    /// `Self` when `self_typed` allows it.
    fn finish_leading_param(
        &mut self,
        name: String,
        convention: Option<ArgConvention>,
        origin: Option<mojito_ast::ast::OriginSpec>,
        self_typed: bool,
    ) -> Result<FnParam, ParseError> {
        if self_typed
            && name == "self"
            && convention != Some(ArgConvention::Out)
            && !matches!(self.peek_token()?, Some(Token::Colon))
        {
            return Ok(FnParam {
                name,
                ty: Type::SelfType,
                default: None,
                kind: ParamKind::Regular,
                convention,
                origin,
            });
        }
        self.finish_param(name, ParamKind::Regular, convention, origin)
    }

    /// The optional `: type` annotating a method's `self` receiver.
    fn parse_optional_receiver_type(&mut self) -> Result<Option<Type>, ParseError> {
        if !matches!(self.peek_token()?, Some(Token::Colon)) {
            return Ok(None);
        }
        self.next_token()?;
        self.parse_type().map(Some)
    }

    /// The rest of a method's argument list after its leading receiver
    /// (`[convention] self`, already consumed): the receiver's optional
    /// annotation and the parameters following an optional comma.
    fn parse_leading_receiver(
        &mut self,
        convention: Option<ArgConvention>,
        origin: Option<mojito_ast::ast::OriginSpec>,
    ) -> Result<(Option<Receiver>, ParamList), ParseError> {
        let ty = self.parse_optional_receiver_type()?;
        let list = if matches!(self.peek_token()?, Some(Token::Comma)) {
            self.next_token()?; // consume ','
            self.parse_params()?
        } else {
            ParamList {
                params: Vec::new(),
                positional_only: None,
                keyword_only: None,
            }
        };
        let receiver = Receiver {
            convention,
            origin,
            ty,
            index: 0,
        };
        Ok((Some(receiver), list))
    }

    /// A method of a struct or enum body. An enum method whose body is only
    /// `...` is rejected as upstream rejects it, since `...` is reserved
    /// there; an `@unavailable` method keeps the placeholder.
    fn parse_member_method(
        &mut self,
        decorators: Vec<Decorator>,
        is_enum: bool,
    ) -> Result<Method, ParseError> {
        let start = self.peek_start();
        let unavailable = decorators
            .iter()
            .any(|decorator| decorator.path.as_slice() == ["unavailable"]);
        let method = self.parse_method(decorators)?;
        if is_enum && !unavailable && self.body_is_placeholder(&method.body) {
            return Err(ParseError::Message(
                "enum methods must have a body; '...' is reserved".to_string(),
            )
            .at((start, self.last_span.1)));
        }
        Ok(method)
    }

    /// Whether a body holds a `...` and nothing else but docstrings.
    fn body_is_placeholder(&self, body: &[Stmt]) -> bool {
        body.iter()
            .any(|statement| self.placeholder_statements.contains(&statement.span.0))
            && body.iter().all(|statement| {
                self.placeholder_statements.contains(&statement.span.0)
                    || self.docstring_statements.contains(&statement.span.0)
            })
    }

    /// `case name` or `case name(T, ...)`, the only two forms upstream's
    /// `parseEnumCaseStmt` accepts. A labeled payload has its own
    /// diagnostic; every other spelling after `case` is one malformed case.
    fn parse_enum_case(&mut self) -> Result<EnumCase, ParseError> {
        let start = self.peek_start();
        self.expect(&Token::Case, "Expected 'case'")?;
        let malformed = |end: usize| {
            ParseError::Message(
                "enum case must have the form 'case name' or 'case name(T, ...)'".to_string(),
            )
            .at((start, end))
        };
        let labeled = |end: usize| {
            ParseError::Message(
                "labeled payloads in enum cases are not supported; use positional types, e.g. 'case C(T, U)'"
                    .to_string(),
            )
            .at((start, end))
        };
        let name = match self.peek_token()? {
            Some(Token::Identifier(name)) => name.clone(),
            _ => return Err(malformed(self.last_span.1)),
        };
        self.next_token()?;
        let mut payload = Vec::new();
        if matches!(self.peek_token()?, Some(Token::LParen)) {
            self.next_token()?;
            if matches!(self.peek_token()?, Some(Token::RParen)) {
                return Err(malformed(self.last_span.1));
            }
            loop {
                payload.push(self.parse_type()?);
                match self.peek_token()? {
                    Some(Token::Colon | Token::Assign) => return Err(labeled(self.last_span.1)),
                    Some(Token::Comma) => {
                        self.next_token()?;
                        if matches!(self.peek_token()?, Some(Token::RParen)) {
                            self.next_token()?;
                            break;
                        }
                    }
                    Some(Token::RParen) => {
                        self.next_token()?;
                        break;
                    }
                    _ => return Err(malformed(self.last_span.1)),
                }
            }
        }
        if !matches!(
            self.peek_token()?,
            Some(Token::Newline | Token::Semicolon | Token::Eof | Token::Dedent) | None
        ) {
            return Err(malformed(self.last_span.1));
        }
        if !matches!(self.peek_token()?, Some(Token::Dedent) | None) {
            self.expect_stmt_end()?;
        }
        Ok(EnumCase {
            name,
            payload,
            span: (start, self.last_significant_end),
        })
    }
}

/// A method's receiver: its convention, `ref` origin, annotation, and the
/// number of parameters declared before it.
struct Receiver {
    convention: Option<ArgConvention>,
    origin: Option<mojito_ast::ast::OriginSpec>,
    ty: Option<Type>,
    index: usize,
}

/// Takes an initializer's `self` out of its parameter list, wherever it is
/// declared: upstream's `out self` is the result, which binds no call
/// argument, so its position only scopes the arguments its annotation reads.
/// A receiver declared after other arguments must be `out self`, as at
/// the pin.
fn split_initializer_receiver(
    mut list: ParamList,
) -> Result<(Option<Receiver>, ParamList), ParseError> {
    let Some(index) = list.params.iter().position(|param| param.name == "self") else {
        return Ok((None, list));
    };
    let param = list.params.remove(index);
    if param.convention != Some(ArgConvention::Out) {
        return Err(ParseError::UnexpectedToken(
            Token::Identifier(param.name),
            "__init__ method must return Self type with 'out' argument".into(),
        ));
    }
    if list.params.iter().any(|other| other.name == "self") {
        return Err(ParseError::UnexpectedToken(
            Token::Identifier(param.name),
            "an initializer declares one 'self'".into(),
        ));
    }
    let shift = |marker: usize| if marker > index { marker - 1 } else { marker };
    list.positional_only = list.positional_only.map(shift);
    list.keyword_only = list.keyword_only.map(shift);
    let receiver = Receiver {
        convention: param.convention,
        origin: param.origin,
        ty: Some(param.ty),
        index,
    };
    Ok((Some(receiver), list))
}
