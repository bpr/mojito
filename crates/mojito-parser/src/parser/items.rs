//! Item parsing: parameter lists, structs, traits, and methods.

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
                        params.push(self.finish_param(
                            name,
                            ParamKind::Regular,
                            Some(ArgConvention::Var),
                            None,
                        )?);
                    }
                }
                // A regular parameter, with an optional convention prefix.
                _ => {
                    let (convention, origin, name) = self.parse_convention_and_name()?;
                    params.push(self.finish_param(name, ParamKind::Regular, convention, origin)?);
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
        self.expect(Token::RBracket, "Expected ']' after the origin specifier")?;
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
        self.expect(Token::Colon, "Parameters require a type annotation")?;
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

    /// `[@fieldwise_init] struct Name: <fields and methods>`
    pub(super) fn parse_struct(
        &mut self,
        decorators: Vec<Decorator>,
    ) -> Result<StmtKind, ParseError> {
        // `@fieldwise_init` (the one modeled decorator) generates the constructor.
        let fieldwise_init = decorators
            .iter()
            .any(|d| d.path.len() == 1 && d.path[0] == "fieldwise_init");

        self.expect(Token::Struct, "Expected 'struct'")?;
        let name = self.expect_identifier("Expected a struct name after 'struct'")?;
        let type_params = self.parse_type_params()?;
        let (conforms, conformance_conditions, callable_conformance) =
            self.parse_struct_conformance()?;
        let where_clauses = self.parse_where_clauses()?;
        self.expect(Token::Colon, "Expected ':' after the struct name")?;
        self.expect_stmt_end()?;

        // Body: an indented block of `var` fields, `comptime` associated facts,
        // and `def` methods.
        self.expect(Token::Indent, "Expected an indented struct body")?;
        let mut fields = Vec::new();
        let mut associated = Vec::new();
        let mut methods = Vec::new();
        while let Some(token) = self.peek_token()? {
            match token {
                Token::Dedent => {
                    self.next_token()?;
                    break;
                }
                Token::Newline => {
                    self.next_token()?;
                }
                Token::TripleStringLiteral(_) => {
                    self.next_token()?;
                    self.expect_stmt_end()?;
                }
                Token::Var => {
                    self.expect(Token::Var, "Expected 'var'")?;
                    let fname = self.expect_identifier("Expected a field name")?;
                    self.expect(Token::Colon, "Fields require a type annotation")?;
                    let ty = self.parse_type()?;
                    if matches!(self.peek_token()?, Some(Token::Assign)) {
                        self.next_token()?;
                        self.parse_expression(Precedence::Lowest)?;
                    }
                    self.expect_stmt_end()?;
                    fields.push(Param { name: fname, ty });
                }
                Token::Pass => {
                    self.next_token()?;
                    self.expect_stmt_end()?;
                }
                Token::Comptime => associated.push(self.parse_struct_comptime()?),
                Token::Def => methods.push(self.parse_method(Vec::new())?),
                // Decorators before a method (`@staticmethod`, …).
                Token::At => {
                    let decos = self.parse_decorators()?;
                    if !matches!(self.peek_token()?, Some(Token::Def)) {
                        return Err(ParseError::UnexpectedToken(
                            self.peek_token()?.cloned().unwrap_or(Token::Eof),
                            "a decorator in a struct body must precede a 'def' method".into(),
                        ));
                    }
                    methods.push(self.parse_method(decos)?);
                }
                other => {
                    return Err(ParseError::UnexpectedToken(
                        other.clone(),
                        "struct body may only contain 'var' fields, 'comptime' associated facts, and 'def' methods".into(),
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
        })
    }

    /// `comptime NAME = expr` — an associated compile-time fact inside a struct.
    pub(super) fn parse_struct_comptime(
        &mut self,
    ) -> Result<mojito_ast::ast::StructComptime, ParseError> {
        self.expect(Token::Comptime, "Expected 'comptime'")?;
        let name = self.expect_identifier("Expected a name after 'comptime'")?;
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
        self.expect(Token::Assign, "Expected '=' after the comptime member name")?;
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
            traits.push(mojito_ast::ast::canonical_trait_name(&trait_name).to_string());
            if matches!(self.peek_token()?, Some(Token::Comma)) {
                self.next_token()?; // consume ','
                if matches!(self.peek_token()?, Some(Token::RParen)) {
                    break;
                }
            } else {
                break;
            }
        }
        self.expect(Token::RParen, "Expected ')' after the conformance list")?;
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
            let trait_name = {
                let spelled =
                    self.expect_identifier("Expected a trait name in the conformance list")?;
                mojito_ast::ast::canonical_trait_name(&spelled).to_string()
            };
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
        self.expect(Token::RParen, "Expected ')' after the conformance list")?;
        Ok((traits, conditions, callable))
    }

    /// `trait Name[(Super, …)]: <members>` — a trait, optionally **refining**
    /// super-traits (`trait Bird(Animal):`, reusing the conformance-list parser).
    /// The body holds `def` method requirements (`...`) or default methods (a real
    /// body), and `comptime NAME: Type` member requirements. (Generic traits
    /// `trait T[U]:` are not valid current Mojo, so no `[type_params]` is parsed.)
    pub(super) fn parse_trait(&mut self) -> Result<StmtKind, ParseError> {
        self.expect(Token::Trait, "Expected 'trait'")?;
        let name = self.expect_identifier("Expected a trait name after 'trait'")?;
        let refines = self.parse_conformance()?;
        self.expect(Token::Colon, "Expected ':' after the trait name")?;
        self.expect_stmt_end()?;

        self.expect(Token::Indent, "Expected an indented trait body")?;
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
                Token::TripleStringLiteral(_) => {
                    self.next_token()?;
                    self.expect_stmt_end()?;
                }
                Token::Def => methods.push(self.parse_trait_method()?),
                Token::Comptime => comptime_members.push(self.parse_trait_comptime()?),
                other => {
                    return Err(ParseError::UnexpectedToken(
                        other.clone(),
                        "a trait body may only contain 'def' methods or 'comptime' members".into(),
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

    /// `comptime NAME: Type` — a compile-time member requirement inside a trait.
    pub(super) fn parse_trait_comptime(
        &mut self,
    ) -> Result<mojito_ast::ast::TraitComptime, ParseError> {
        self.expect(Token::Comptime, "Expected 'comptime'")?;
        let name = self.expect_identifier("Expected a name after 'comptime'")?;
        // A parameterized associated type requirement:
        // `comptime IteratorType[iterable_mut: Bool, //, iterable_origin:
        // Origin[mut=iterable_mut]]: Iterator`.
        let params = if matches!(self.peek_token()?, Some(Token::LBracket)) {
            self.parse_type_params()?
        } else {
            Vec::new()
        };
        self.expect(Token::Colon, "Expected ':' after the comptime member name")?;
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
        self.expect_stmt_end()?;
        Ok(mojito_ast::ast::TraitComptime {
            name,
            params,
            ty,
            where_clauses,
        })
    }

    /// `def name([convention] self [, params]) -> ret:` followed by an indented
    /// body that is either `...` (a pure requirement) or real statements (a
    /// **default implementation**, stored in `default_body`).
    pub(super) fn parse_trait_method(
        &mut self,
    ) -> Result<mojito_ast::ast::TraitMethod, ParseError> {
        self.expect(Token::Def, "Expected 'def'")?;
        let name = {
            let spelled = self.expect_identifier("Expected a method name after 'def'")?;
            mojito_ast::ast::canonical_destructor_name(&spelled).to_string()
        };
        let type_params = self.parse_type_params()?;

        self.expect(Token::LParen, "Expected '(' after the method name")?;
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
        self.expect(Token::RParen, "Expected ')' after the parameters")?;

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
            self.expect(Token::RBrace, "Expected '}' after method effects")?;
        }
        let where_clauses = self.parse_where_clauses()?;
        self.expect(Token::Colon, "Expected ':' before the method body")?;
        // A body of exactly `...` is a pure requirement; anything else is a
        // default implementation (parsed, flagged unsupported by the checker).
        let default_body = self.parse_trait_method_body()?;

        Ok(mojito_ast::ast::TraitMethod {
            name,
            type_params,
            self_convention,
            self_origin,
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
        self.expect(Token::Def, "Expected 'def'")?;
        let name = {
            let spelled = self.expect_identifier("Expected a method name after 'def'")?;
            mojito_ast::ast::canonical_destructor_name(&spelled).to_string()
        };
        let type_params = self.parse_type_params()?;

        self.expect(Token::LParen, "Expected '(' after the method name")?;
        let is_static = decorators
            .iter()
            .any(|decorator| decorator.path.as_slice() == ["staticmethod"]);
        // Detect the receiver. An instance method starts with `self`, optionally
        // carrying a convention (`mut self`, `out self`, `var self`, `imm self`
        // — convention words are contextual identifiers). A `@staticmethod` has
        // no `self`, so its parameters start immediately even when the first one
        // has a convention, notably canonical `var **kwargs`.
        let first_is_self =
            matches!(self.peek_token()?, Some(Token::Identifier(id)) if id == "self");
        let first_is_convention = matches!(self.peek_token()?, Some(Token::Identifier(id)) if convention_word(id).is_some())
            || matches!(self.peek_token()?, Some(Token::Var));
        let (has_self, self_convention, self_origin) = if is_static {
            (false, None, None)
        } else if first_is_self {
            self.next_token()?; // consume 'self'
            (true, None, None)
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
            let self_name =
                self.expect_identifier("Expected 'self' after the receiver convention")?;
            if self_name != "self" {
                return Err(ParseError::UnexpectedToken(
                    Token::Identifier(self_name),
                    "a receiver convention must be followed by 'self'".into(),
                ));
            }
            (true, conv, origin)
        } else {
            // No receiver — a static method.
            (false, None, None)
        };
        // Parameters: for an instance method they follow an optional comma after
        // `self`; for a static method they are the whole list.
        let ParamList {
            params,
            positional_only,
            keyword_only,
        } = if has_self {
            if matches!(self.peek_token()?, Some(Token::Comma)) {
                self.next_token()?; // consume ','
                self.parse_params()?
            } else {
                ParamList {
                    params: Vec::new(),
                    positional_only: None,
                    keyword_only: None,
                }
            }
        } else {
            self.parse_params()?
        };
        self.expect(Token::RParen, "Expected ')' after the parameters")?;

        let (raises, raises_type) = self.parse_callable_effects()?;
        let ret = if matches!(self.peek_token()?, Some(Token::Arrow)) {
            self.next_token()?;
            Some(self.parse_type()?)
        } else {
            None
        };

        let where_clauses = self.parse_where_clauses()?;

        self.expect(Token::Colon, "Expected ':' before the method body")?;
        let body = self.parse_suite()?;

        Ok(Method {
            name,
            type_params,
            has_self,
            self_convention,
            self_origin,
            decorators,
            params,
            positional_only,
            keyword_only,
            raises,
            raises_type,
            ret,
            where_clauses,
            body,
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
                        self.expect(Token::RBracket, "Expected ']' after capturing origins")?;
                    }
                }
                "abi" => {
                    self.next_token()?;
                    self.expect(Token::LParen, "Expected '(' after abi")?;
                    while !matches!(self.peek_token()?, Some(Token::RParen) | None) {
                        self.next_token()?;
                    }
                    self.expect(Token::RParen, "Expected ')' after abi")?;
                }
                _ => break,
            }
        }
        Ok(())
    }
}
