//! `match` statements: the subject is anchored once, each case's pattern
//! compiles to the commands upstream's `PatternMatchBuilder` builds (tests
//! and bindings), and the plan is recorded for the checked program.
//!
//! The check synthesizes the syntax its commands run on (`desugar.rs`'s
//! `Synth`): `ref $match<id> = subject` anchors the subject with the
//! ordinary reference-binding rules (a place keeps its storage, a temporary
//! gets a hidden slot for the statement), a test is the comparison
//! `$match<id> == value` through the selected `__eq__`, a bare binding is
//! `ref name = $match<id>` made immutable, a `var` binding is
//! `var name = $match<id>` (the implicit-copy rule), a `ref` or `as` binding
//! is `ref name = $match<id>` with the subject's own capability, and a guard
//! is a `Bool` condition checked once the case's bindings exist.
//!
//! A sequence pattern `[p0, p1, ...]` over a `Sized` subject first tests the
//! length through upstream's library function
//! (`list_length_matches($match<id>, arity, True)`), then compiles each
//! element's pattern over `$match<id>[i]`. An element is read once per case
//! (and per `|` alternative), at the first command that needs it, into a
//! hidden `$elem<id>`: `ref` over a reference result, `var` over an owned
//! one, which a `var` binding then takes by transfer. `_` reads nothing.
//! A star `[p, *q, r]` makes the test a minimum
//! (`list_length_matches($match<id>, 2, False)`), reads an element after it
//! at `$match<id>[$match<id>.__len__() - k]`, and runs its pattern `q` over
//! the slice `$match<id>[1:$match<id>.__len__() - k]`, read like an element,
//! so `*_` slices nothing.

use super::desugar::Synth;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::{BindingMode, MatchCase, MatchPattern, MatchPatternKind, SequencePattern};
use mojito_checked::checked::{MatchPlan, PlannedCommand, Warning};

impl Checker {
    /// Check `match subject:` with its `cases`: anchor the subject, compile
    /// each case's pattern to commands in a scope of its own, check its
    /// body there, and record the plan at the statement.
    pub(super) fn check_match(
        &mut self,
        stmt: &Stmt,
        subject: &Expr,
        cases: &[MatchCase],
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<(), TypeError> {
        let synth = Synth::for_statement(stmt, subject.source.clone());
        let name = format!("$match{}", synth.parent().0);
        let read_only = !cases.iter().any(|case| binds_by_reference(&case.pattern));
        // The hidden subject lives in a scope of its own around the cases.
        self.push_scope();
        let checked = self
            .anchor_subject(&synth, &name, subject, read_only, ret, in_loop)
            .and_then(|(anchor, ty, temporary)| {
                let subject = MatchSubject::root(&synth, &name, ty, temporary);
                self.check_match_cases(stmt, &subject, anchor, cases, ret, in_loop)
            });
        self.pop_scope();
        checked
    }

    /// Check the `case pattern = value` condition of an `if`, `elif`, or
    /// `while` branch whose body is `body`: anchor the value, compile the
    /// pattern's commands, and check the body in the scope that holds the
    /// bindings — a sibling branch or the statement after sees none of
    /// them. The plan is recorded at the pattern.
    pub(super) fn check_case_condition(
        &mut self,
        pattern: &MatchPattern,
        value: &Expr,
        body: &[Stmt],
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<(), TypeError> {
        let synth = Synth::for_pattern(pattern, value.source.clone());
        let name = format!("$case{}", synth.parent().0);
        let read_only = !binds_by_reference(pattern);
        self.push_scope();
        let checked = self
            .anchor_subject(&synth, &name, value, read_only, ret, in_loop)
            .and_then(|(anchor, ty, temporary)| {
                let subject = MatchSubject::root(&synth, &name, ty, temporary);
                let mut commands = Commands::default();
                self.compile_pattern(pattern, &subject, Join::None, &mut commands)?;
                self.check_block(body, ret, in_loop)?;
                self.record_at(pattern.source_span()).match_plan = Some(MatchPlan {
                    anchor,
                    cases: vec![commands.into_program()],
                    concluded: false,
                });
                Ok(())
            });
        self.pop_scope();
        checked
    }

    /// Declare the hidden `name` over `subject` in the current scope: a
    /// place subject is borrowed where it lies (`ref name = place`), to
    /// read when `read_only` — then the guards and bodies may read the
    /// place themselves — and with the place's capability otherwise; any
    /// other subject is a temporary materialized into one owner that lasts
    /// through the statement (`var name = value`), which no `ref` binding of
    /// a register-passable value may name. The anchor statement, the
    /// subject's type (a reference stripped), and whether it is a temporary.
    pub(super) fn anchor_subject(
        &mut self,
        synth: &Synth,
        name: &str,
        subject: &Expr,
        read_only: bool,
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<(Stmt, Ty, bool), TypeError> {
        let temporary = !(super::places::is_place_expr(subject)
            || matches!(self.infer(subject)?.kind(), TyKind::Ref(_)));
        let anchor = synth.stmt(if temporary {
            StmtKind::VarDecl {
                name: name.to_string(),
                ty: None,
                value: subject.clone(),
            }
        } else {
            StmtKind::RefDecl {
                name: name.to_string(),
                value: subject.clone(),
            }
        });
        self.check_stmt(&anchor, ret, in_loop)?;
        if !temporary && read_only {
            self.make_reference_immutable(name, &anchor, subject.source_span());
        }
        let ty = self.lookup(name).cloned().ok_or_else(|| {
            TypeError::InvariantViolation("the match subject's anchor was not declared".into())
        })?;
        Ok((anchor, dereferenced(ty), temporary))
    }

    /// Make the reference binding `name`, declared by `declaration` over
    /// the value at `value_span`, read its referent whatever the place's
    /// capability: the binding, its type, and its borrow are immutable.
    pub(super) fn make_reference_immutable(
        &mut self,
        name: &str,
        declaration: &Stmt,
        value_span: SourceSpan,
    ) {
        self.set_binding_mutable(name, false);
        self.record_at(value_span).reference_value_use = Some(false);
        if let Some(ty) = self.lookup(name).cloned()
            && let TyKind::Ref(reference) = ty.kind()
        {
            let mut reference = reference.clone();
            reference.mutability = mojito_types::origin::Mutability::Immutable;
            let ty = Ty::new(TyKind::Ref(reference));
            self.set_binding_type(name, ty.clone());
            self.record_at(declaration.source_span()).binding_ty = Some(ty);
        }
    }

    /// Whether the check proved the `match` statement `stmt` covers every
    /// value of its subject, so that nothing falls through it.
    pub(super) fn match_concluded(&self, stmt: &Stmt) -> bool {
        self.has_fact(&stmt.source_span(), |facts| {
            facts.match_plan.as_ref().is_some_and(|plan| plan.concluded)
        })
    }

    /// Require `ty` to declare `Sized`, the gate upstream's list patterns
    /// and list-pattern stores share, with its two diagnostics: an unknown
    /// conformance of a parameter cannot be proved, any other type does not
    /// conform.
    pub(super) fn require_sized(&self, ty: &Ty) -> Result<(), TypeError> {
        if self.conforms_to(ty, "Sized") {
            return Ok(());
        }
        Err(TypeError::MatchPattern(
            if matches!(ty.kind(), TyKind::Param { .. }) {
                format!("cannot prove that value of type '{ty}' conforms to 'Sized'")
            } else {
                format!("value of type '{ty}' does not conform to 'Sized'")
            },
        ))
    }

    fn check_match_cases(
        &mut self,
        stmt: &Stmt,
        subject: &MatchSubject,
        anchor: Stmt,
        cases: &[MatchCase],
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<(), TypeError> {
        let before = self.uninitialized.borrow().clone();
        let mut exits = Vec::new();
        let mut plan = MatchPlan {
            anchor,
            cases: Vec::new(),
            concluded: false,
        };
        let mut coverage = Coverage::new(&subject.ty);
        let mut warnings = Vec::new();
        for case in cases {
            (*self.uninitialized.borrow_mut()).clone_from(&before);
            self.push_scope();
            let commands = self.check_case(case, subject, ret, in_loop);
            self.pop_scope();
            plan.cases.push(commands?);
            // Once the cases cover every value, nothing falls through past
            // them (later cases are unreachable).
            if !plan.concluded {
                exits.push(self.uninitialized.borrow().clone());
            }
            if let Some(message) = coverage.add(case) {
                warnings.push(Warning {
                    message,
                    span: case.pattern.source_span(),
                });
            }
            if !plan.concluded && coverage.complete() {
                plan.concluded = true;
                // The proof made the closing case's tests pass whenever it
                // is reached: it runs without them, so no path falls
                // through. (A `|` that binds keeps its tests, and the
                // fall-through.)
                let closing = plan.cases.last_mut().expect("the case was pushed");
                if closing.iter().all(droppable_test) {
                    closing.retain(|command| {
                        matches!(
                            command,
                            PlannedCommand::Bind(_) | PlannedCommand::PathValue(_)
                        )
                    });
                } else {
                    plan.concluded = false;
                }
            }
        }
        if !plan.concluded {
            exits.push(before);
            if let Some(message) = coverage.missing() {
                warnings.push(Warning {
                    message,
                    span: stmt.source_span(),
                });
            }
        }
        *self.uninitialized.borrow_mut() = exits.into_iter().flatten().collect::<HashSet<_>>();
        let mut facts = self.record_at(stmt.source_span());
        facts.match_plan = Some(plan);
        facts.match_warnings = (!warnings.is_empty()).then_some(warnings);
        Ok(())
    }

    /// One case in its own scope: its pattern's commands, then its body.
    fn check_case(
        &mut self,
        case: &MatchCase,
        subject: &MatchSubject,
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<Vec<PlannedCommand>, TypeError> {
        let mut commands = Commands::default();
        self.compile_pattern(&case.pattern, subject, Join::None, &mut commands)?;
        if let Some(guard) = &case.guard {
            self.register_named_bindings(guard)?;
            self.expect_bool(guard, "case guard")?;
        }
        self.check_block(&case.body, ret, in_loop)?;
        Ok(commands.into_program())
    }

    /// The commands `pattern` compiles to over `subject`: a literal or
    /// `== value` compares, `_` tests nothing, a name binds the subject by
    /// its mode (through `join` inside a `|` alternative), `as` binds it
    /// once its pattern has matched, a tuple or struct pattern compiles
    /// each element's or field's pattern over that projection of the
    /// subject, in order, a sequence pattern tests the length and then
    /// compiles each element's pattern, and `|` compiles its alternatives.
    fn compile_pattern(
        &mut self,
        pattern: &MatchPattern,
        subject: &MatchSubject,
        mut join: Join<'_>,
        commands: &mut Commands,
    ) -> Result<(), TypeError> {
        match &pattern.kind {
            MatchPatternKind::Wildcard => Ok(()),
            MatchPatternKind::Bind { name, mode } => self.bind_subject(
                name,
                Binding::from(*mode),
                pattern.source_span(),
                subject,
                join,
                commands,
            ),
            MatchPatternKind::As {
                pattern: inner,
                name,
            } => {
                self.compile_pattern(inner, subject, join.reborrow(), commands)?;
                self.bind_subject(
                    name,
                    Binding::As,
                    pattern.source_span(),
                    subject,
                    join,
                    commands,
                )
            }
            MatchPatternKind::Literal(value) | MatchPatternKind::Equal(value) => {
                let subject = &self.materialize(subject, commands)?;
                let test = subject.synth.expr(ExprKind::Infix(
                    InfixOp::Eq,
                    Box::new(subject.value()),
                    Box::new(value.clone()),
                ));
                self.register_named_bindings(&test)?;
                self.expect_bool(&test, "case pattern")?;
                commands.tests.push(PlannedCommand::Equal(test));
                Ok(())
            }
            MatchPatternKind::Tuple(items) => {
                self.compile_tuple_pattern(items, subject, join, commands)
            }
            MatchPatternKind::Call { head, args, kwargs } => {
                self.compile_struct_pattern(head, args, kwargs, subject, join, commands)
            }
            MatchPatternKind::Fields(fields) => {
                self.compile_field_patterns(fields, subject, join, commands)
            }
            MatchPatternKind::Or(alternatives) => {
                self.compile_or_pattern(alternatives, subject, join, commands)
            }
            MatchPatternKind::Head(_) => Err(TypeError::Unsupported("enum case patterns".into())),
            MatchPatternKind::Sequence(items) => {
                self.compile_sequence_pattern(items, subject, join, commands)
            }
        }
    }

    /// `p0 | p1 | ...`: each alternative's commands as a program of its
    /// own, the first that matches selecting. Every alternative binds the
    /// same names, the first's declarations being the bindings the later
    /// ones assign, each with the first's type.
    fn compile_or_pattern(
        &mut self,
        alternatives: &[MatchPattern],
        subject: &MatchSubject,
        mut join: Join<'_>,
        commands: &mut Commands,
    ) -> Result<(), TypeError> {
        let mut first: Option<Vec<String>> = None;
        let mut programs = Vec::new();
        for alternative in alternatives {
            // An alternative re-reads an element the commands before the
            // `|` did not.
            let mut program = Commands {
                reads: commands.reads.clone(),
                ..Commands::default()
            };
            let mut bound = Vec::new();
            let inner = if matches!(join, Join::Assign(_)) || first.is_some() {
                Join::Assign(&mut bound)
            } else {
                Join::Declare(&mut bound)
            };
            self.compile_pattern(alternative, subject, inner, &mut program)?;
            if let Some(first) = &first {
                if let Some(missing) = first
                    .iter()
                    .find(|name| !bound.contains(name))
                    .or_else(|| bound.iter().find(|name| !first.contains(name)))
                {
                    return Err(TypeError::MatchPattern(format!(
                        "or-pattern alternatives must bind the same names; '{missing}' is \
                         bound in one alternative but not the other"
                    )));
                }
            } else {
                first = Some(bound);
            }
            programs.push(program.into_program());
        }
        if let Some(names) = first {
            join.record(names);
        }
        commands.tests.push(PlannedCommand::Alternatives(programs));
        Ok(())
    }

    /// `(p0, p1, ...)` over a tuple subject of the same arity: each
    /// element's pattern over `subject[i]`, in element order.
    fn compile_tuple_pattern(
        &mut self,
        items: &[MatchPattern],
        subject: &MatchSubject,
        mut join: Join<'_>,
        commands: &mut Commands,
    ) -> Result<(), TypeError> {
        if subject.element.is_some() {
            return Err(TypeError::MatchPattern(format!(
                "expected a tuple type to match against, got {UNTYPED_ELEMENT}"
            )));
        }
        let Some(elements) = tuple_elements(&subject.ty) else {
            return Err(TypeError::MatchPattern(format!(
                "expected a tuple type to match against, got '{}'",
                subject.ty
            )));
        };
        if elements.len() != items.len() {
            return Err(TypeError::MatchPattern(format!(
                "cannot match value of '{}' of {} elements against a pattern with {} elements",
                subject.ty,
                elements.len(),
                items.len()
            )));
        }
        let elements: Vec<Ty> = elements.into_iter().cloned().collect();
        for (index, (item, element)) in items.iter().zip(elements).enumerate() {
            let child = subject.project(Step::Element(index), element);
            self.compile_pattern(item, &child, join.reborrow(), commands)?;
        }
        Ok(())
    }

    /// `[p0, p1, ...]` over a `Sized` subject: one length test, upstream's
    /// `list_length_matches` against the pattern's arity (a minimum when a
    /// star is present), then each element's pattern over its path, in
    /// element order, each path read at the first command that needs it:
    /// `subject[i]` before the star, `subject[subject.__len__() - k]` after
    /// it, and the star's own pattern over the slice
    /// `subject[i:subject.__len__() - k]`. The subject's declared
    /// conformance decides, not its methods; the getters are the
    /// subscripts' own selections.
    fn compile_sequence_pattern(
        &mut self,
        items: &[SequencePattern],
        subject: &MatchSubject,
        mut join: Join<'_>,
        commands: &mut Commands,
    ) -> Result<(), TypeError> {
        if subject.element.is_some() {
            return Err(TypeError::MatchPattern(format!(
                "{UNTYPED_ELEMENT} does not conform to 'Sized'"
            )));
        }
        if matches!(
            subject.ty.kind(),
            TyKind::Int | TyKind::UInt | TyKind::Float64 | TyKind::Simd { .. }
        ) {
            return Err(TypeError::Unsupported(format!(
                "a sequence pattern over '{}'",
                subject.ty
            )));
        }
        self.require_sized(&subject.ty)?;
        let length_matches = builtin_stub("list_length_matches");
        if self.lookup(&length_matches).is_none() {
            return Err(TypeError::Unsupported(format!(
                "a sequence pattern needs the bundled '{STUBS_MODULE}' module"
            )));
        }
        let synth = subject.synth;
        let count = items.len();
        let star = items
            .iter()
            .position(|item| matches!(item, SequencePattern::Star(_)));
        let test = synth.expr(ExprKind::Call {
            name: length_matches,
            param_args: Vec::new(),
            args: vec![
                subject.value(),
                synth.int(count - usize::from(star.is_some())),
                synth.expr(ExprKind::Bool(star.is_none())),
            ],
            kwargs: Vec::new(),
        });
        self.expect_bool(&test, "case pattern")?;
        commands.tests.push(PlannedCommand::ListLength(test));
        for (index, item) in items.iter().enumerate() {
            let step = match star {
                Some(star) if index == star => Step::SequenceSlice {
                    start: index,
                    end_offset: count - index - 1,
                },
                Some(star) if index > star => Step::SequenceFromEnd(count - index),
                _ => Step::SequenceElement(index),
            };
            let pattern = item.pattern();
            let child = subject.sequence_element(
                step,
                ElementRead {
                    by_reference: binds_by_reference(pattern),
                    moves: root_bindings(pattern) <= 1,
                },
            );
            self.compile_pattern(pattern, &child, join.reborrow(), commands)?;
        }
        Ok(())
    }

    /// The subject a test or binding reads: `subject` itself, or, for a
    /// sequence element, the hidden `$elem<id>` holding its read — made by
    /// this case's (or `|` alternative's) first command that needs it, as
    /// `ref $elem<id> = parent[i]` over a reference result (to read unless
    /// the element binds by reference) and `var $elem<id> = parent[i]` over
    /// an owned one, and reused by every later command.
    fn materialize<'s>(
        &mut self,
        subject: &MatchSubject<'s>,
        commands: &mut Commands,
    ) -> Result<MatchSubject<'s>, TypeError> {
        let Some(element) = subject.element else {
            return Ok(subject.clone());
        };
        let read = if let Some(read) = commands.reads.get(&subject.path) {
            read.clone()
        } else {
            let value = subject.value();
            let value_span = value.source_span();
            let name = format!("$elem{}", value.syntax_id.0);
            self.infer(&value)?;
            let reference = self.infer_reference_value(&value).is_some();
            let declaration = subject.synth.stmt(if reference {
                StmtKind::RefDecl {
                    name: name.clone(),
                    value,
                }
            } else {
                StmtKind::VarDecl {
                    name: name.clone(),
                    ty: None,
                    value,
                }
            });
            self.check_stmt(&declaration, None, false)?;
            if reference && !element.by_reference {
                self.make_reference_immutable(&name, &declaration, value_span);
            }
            let ty = self.lookup(&name).cloned().ok_or_else(|| {
                TypeError::InvariantViolation("a sequence element read was not declared".into())
            })?;
            commands
                .tests
                .push(PlannedCommand::PathValue(Box::new(declaration)));
            let read = Read {
                name,
                ty: dereferenced(ty),
                owned: !reference,
            };
            commands.reads.insert(subject.path.clone(), read.clone());
            read
        };
        Ok(MatchSubject {
            synth: subject.synth,
            name: read.name,
            ty: read.ty,
            temporary: false,
            owned: read.owned.then_some(element.moves),
            element: None,
            path: Vec::new(),
        })
    }

    /// `Name(field=p, ...)` over a subject of the nominal type `Name`: no
    /// positional argument, then the field patterns. The head names the
    /// type; it runs no constructor.
    fn compile_struct_pattern(
        &mut self,
        head: &Expr,
        args: &[MatchPattern],
        kwargs: &[(String, MatchPattern)],
        subject: &MatchSubject,
        join: Join<'_>,
        commands: &mut Commands,
    ) -> Result<(), TypeError> {
        let (ExprKind::Identifier(name) | ExprKind::TypeApply { name, .. }) = &head.kind else {
            return Err(TypeError::Unsupported("enum case patterns".into()));
        };
        if subject.element.is_some() {
            return Err(TypeError::MatchPattern(format!(
                "cannot match {UNTYPED_ELEMENT} against pattern type '{name}'"
            )));
        }
        if !args.is_empty() {
            return Err(TypeError::MatchPattern(
                "struct patterns do not support positional or unpacked arguments".into(),
            ));
        }
        let matches_head = match subject.ty.kind() {
            TyKind::Struct(subject_name, _) => {
                subject_name == name || subject_name.rsplit('$').next() == Some(name)
            }
            _ => false,
        };
        if !matches_head {
            return Err(TypeError::MatchPattern(format!(
                "cannot match value of type '{}' against pattern type '{name}'",
                subject.ty
            )));
        }
        self.compile_field_patterns(kwargs, subject, join, commands)
    }

    /// `{field=p, ...}` over a struct subject: each named field's pattern
    /// over `subject.field`, a field named once; a field the pattern omits
    /// matches anything.
    fn compile_field_patterns(
        &mut self,
        fields: &[(String, MatchPattern)],
        subject: &MatchSubject,
        mut join: Join<'_>,
        commands: &mut Commands,
    ) -> Result<(), TypeError> {
        if subject.element.is_some() {
            return Err(TypeError::MatchPattern(format!(
                "cannot match field patterns against {UNTYPED_ELEMENT}"
            )));
        }
        let mut seen = HashSet::new();
        for (field, pattern) in fields {
            if !seen.insert(field.as_str()) {
                return Err(TypeError::MatchPattern(format!(
                    "keyword argument '{field}' was already used; remove the duplicate"
                )));
            }
            let child = subject.project(Step::Field(field.clone()), Ty::error());
            // The field's checked type; an unknown field is the selection's
            // own rejection.
            let ty = self.infer(&child.value())?;
            let child = MatchSubject {
                ty: dereferenced(ty),
                ..child
            };
            self.compile_pattern(pattern, &child, join.reborrow(), commands)?;
        }
        Ok(())
    }

    /// Bind `name` to the subject: a bare name is an immutable reference
    /// whatever the subject's capability, `ref` (and `as`) a reference with
    /// the subject's own, `var` a copy by the implicit-copy rule, or the
    /// transfer of an owned element read only it binds. Inside a `|`
    /// alternative only `var` binds: the first alternative declares, a
    /// later one assigns the same binding a value of the same type.
    fn bind_subject(
        &mut self,
        name: &str,
        binding: Binding,
        pattern_span: SourceSpan,
        subject: &MatchSubject,
        join: Join<'_>,
        commands: &mut Commands,
    ) -> Result<(), TypeError> {
        let subject = &self.materialize(subject, commands)?;
        match join {
            Join::None => {}
            Join::Declare(_) | Join::Assign(_) if binding != Binding::Var => {
                return Err(TypeError::Unsupported(format!(
                    "a bare or 'ref' binding in a '|' alternative ('{name}')"
                )));
            }
            Join::Declare(bound) => bound.push(name.to_string()),
            Join::Assign(bound) => {
                bound.push(name.to_string());
                let Some(declared) = self.lookup(name).cloned() else {
                    return Err(TypeError::MatchPattern(format!(
                        "or-pattern alternatives must bind the same names; '{name}' is bound \
                         in one alternative but not the other"
                    )));
                };
                let value = subject.bound_value();
                let found = dereferenced(self.infer(&value)?);
                if found != declared {
                    return Err(TypeError::MatchPattern(format!(
                        "or-pattern binding '{name}' has incompatible types across \
                         alternatives: first alternative has type '{declared}', this \
                         alternative has type '{found}'"
                    )));
                }
                let assignment = subject.synth.stmt(StmtKind::Assign {
                    name: name.to_string(),
                    value,
                });
                self.check_stmt(&assignment, None, false)?;
                commands
                    .binds
                    .push(PlannedCommand::Bind(Box::new(assignment)));
                return Ok(());
            }
        }
        // An owned element is an rvalue, which neither `ref` nor `as` may
        // name when it is register-passable; a root temporary has its
        // anchor's slot, which `as` may name.
        let rvalue = match binding {
            Binding::Ref => subject.temporary || subject.owned.is_some(),
            Binding::As => subject.owned.is_some(),
            Binding::Bare | Binding::Var => false,
        };
        if rvalue && self.is_trivial_register_passable(&subject.ty) {
            return Err(TypeError::NoMemoryOrigin {
                ty: subject.ty.to_string(),
                context: "'ref' binding".into(),
            });
        }
        let value = subject.value();
        let value_span = value.source_span();
        let declaration = subject.synth.stmt(match binding {
            Binding::Var => StmtKind::VarDecl {
                name: name.to_string(),
                ty: None,
                value: subject.bound_value(),
            },
            Binding::Bare | Binding::Ref | Binding::As => StmtKind::RefDecl {
                name: name.to_string(),
                value,
            },
        });
        self.check_stmt(&declaration, None, false)?;
        if binding == Binding::Bare {
            self.make_reference_immutable(name, &declaration, value_span);
        }
        self.record_at(pattern_span).binding = self.lookup_owner(name);
        commands
            .binds
            .push(PlannedCommand::Bind(Box::new(declaration)));
        Ok(())
    }
}

/// Whether `pattern` matches every value of its subject's type: a wildcard
/// or a binding, at every position of a tuple or `as`, in some `|`
/// alternative, or at every named field of a struct pattern.
pub fn irrefutable(pattern: &MatchPattern) -> bool {
    match &pattern.kind {
        MatchPatternKind::Wildcard | MatchPatternKind::Bind { .. } => true,
        MatchPatternKind::Tuple(items) => items.iter().all(irrefutable),
        MatchPatternKind::As { pattern, .. } => irrefutable(pattern),
        MatchPatternKind::Or(items) => items.iter().any(irrefutable),
        MatchPatternKind::Call { args, kwargs, .. } => {
            args.iter().all(irrefutable) && kwargs.iter().all(|(_, pattern)| irrefutable(pattern))
        }
        MatchPatternKind::Fields(fields) => fields.iter().all(|(_, pattern)| irrefutable(pattern)),
        MatchPatternKind::Literal(_)
        | MatchPatternKind::Equal(_)
        | MatchPatternKind::Sequence(_)
        | MatchPatternKind::Head(_) => false,
    }
}

/// The subject a pattern applies to: the hidden `$match<id>` reference the
/// anchor declared (or the hidden `$elem<id>` a sequence element was read
/// into), projected by `path` to the element or field the pattern's
/// position names, with the node factory that spells it.
#[derive(Clone)]
struct MatchSubject<'a> {
    synth: &'a Synth,
    name: String,
    /// The projection's type, any reference stripped; unknown for an
    /// unread sequence element, as upstream's list element path is
    /// untyped.
    ty: Ty,
    /// Whether the whole subject is a temporary the anchor materializes
    /// rather than a place it borrows; a projection never is.
    temporary: bool,
    /// For an owned sequence element read, an rvalue: whether a `var`
    /// binding takes it by transfer, being the only binding of it.
    owned: Option<bool>,
    /// For an unread sequence element (the path's last step), how its
    /// read is made.
    element: Option<ElementRead>,
    path: Vec<Step>,
}

impl<'a> MatchSubject<'a> {
    /// The whole subject, the hidden `name` the anchor declared.
    fn root(synth: &'a Synth, name: &str, ty: Ty, temporary: bool) -> Self {
        MatchSubject {
            synth,
            name: name.to_string(),
            ty,
            temporary,
            owned: None,
            element: None,
            path: Vec::new(),
        }
    }

    /// The expression reading this subject: `$match<id>` under the path's
    /// subscripts and field selections.
    fn value(&self) -> Expr {
        self.value_at(self.path.len())
    }

    /// The value a `var` binding or a later `|` alternative's assignment
    /// takes: the subject read, transferred out of an owned element read
    /// it alone binds.
    fn bound_value(&self) -> Expr {
        let value = self.value();
        if self.owned == Some(true) {
            self.synth.expr(ExprKind::Transfer(Box::new(value)))
        } else {
            value
        }
    }

    /// The subject one `step` below this one, of type `ty`.
    fn project(&self, step: Step, ty: Ty) -> Self {
        let mut path = self.path.clone();
        path.push(step);
        MatchSubject {
            synth: self.synth,
            name: self.name.clone(),
            ty,
            temporary: false,
            owned: None,
            element: None,
            path,
        }
    }

    /// The unread element or slice `step` of this sequence subject, read
    /// as `element` says.
    fn sequence_element(&self, step: Step, element: ElementRead) -> Self {
        MatchSubject {
            element: Some(element),
            ..self.project(step, Ty::error())
        }
    }

    /// The expression reading the subject `path[..depth]` names, each
    /// sequence step spelled as upstream's path values are: an element
    /// from the end and a slice's end read the parent's `__len__` afresh.
    fn value_at(&self, depth: usize) -> Expr {
        let synth = self.synth;
        let mut value = synth.identifier(&self.name);
        for (at, step) in self.path[..depth].iter().enumerate() {
            let object = Box::new(value);
            value = match step {
                Step::Element(index) | Step::SequenceElement(index) => {
                    synth.expr(ExprKind::Index {
                        object,
                        index: Box::new(synth.int(*index)),
                    })
                }
                Step::SequenceFromEnd(offset) => synth.expr(ExprKind::Index {
                    object,
                    index: Box::new(self.length_minus(at, *offset)),
                }),
                Step::SequenceSlice { start, end_offset } => synth.expr(ExprKind::Slice {
                    object,
                    lower: Some(Box::new(synth.int(*start))),
                    upper: (*end_offset != 0).then(|| Box::new(self.length_minus(at, *end_offset))),
                    step: None,
                }),
                Step::Field(field) => synth.expr(ExprKind::Member {
                    object,
                    field: field.clone(),
                }),
            };
        }
        value
    }

    /// `parent.__len__() - offset`, the parent being the subject
    /// `path[..depth]` names, read afresh.
    fn length_minus(&self, depth: usize, offset: usize) -> Expr {
        let length = self.synth.expr(ExprKind::MethodCall {
            object: Box::new(self.value_at(depth)),
            method: "__len__".to_string(),
            args: Vec::new(),
            kwargs: Vec::new(),
        });
        self.synth.expr(ExprKind::Infix(
            InfixOp::Sub,
            Box::new(length),
            Box::new(self.synth.int(offset)),
        ))
    }
}

/// A case's commands as they are collected: its tests and element reads,
/// then its bindings, as upstream runs every test of a pattern before any
/// binding materializes (a failed alternative copies nothing), with the
/// element reads made so far, by path, as upstream's per-path values.
#[derive(Default)]
struct Commands {
    tests: Vec<PlannedCommand>,
    binds: Vec<PlannedCommand>,
    reads: HashMap<Vec<Step>, Read>,
}

impl Commands {
    fn into_program(self) -> Vec<PlannedCommand> {
        self.tests.into_iter().chain(self.binds).collect()
    }
}

/// How a pattern binds a name: the source modes, and `as`, which keeps the
/// subject's capability as `ref` does but may name a temporary of any type.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Binding {
    Bare,
    Var,
    Ref,
    As,
}

impl From<BindingMode> for Binding {
    fn from(mode: BindingMode) -> Self {
        match mode {
            BindingMode::Immutable => Self::Bare,
            BindingMode::Var => Self::Var,
            BindingMode::Ref => Self::Ref,
        }
    }
}

/// How a binding joins a `|` pattern's alternatives: outside one it
/// declares; in the first alternative it declares and is noted; in a later
/// alternative it assigns the first's binding and is noted, so the
/// alternatives' names can be compared.
enum Join<'a> {
    None,
    Declare(&'a mut Vec<String>),
    Assign(&'a mut Vec<String>),
}

impl Join<'_> {
    /// The same join for a nested pattern's bindings.
    const fn reborrow(&mut self) -> Join<'_> {
        match self {
            Join::None => Join::None,
            Join::Declare(bound) => Join::Declare(bound),
            Join::Assign(bound) => Join::Assign(bound),
        }
    }

    /// Note `names`, a nested `|`'s joined bindings, as this join's.
    fn record(&mut self, names: Vec<String>) {
        match self {
            Join::None => {}
            Join::Declare(bound) | Join::Assign(bound) => bound.extend(names),
        }
    }
}

/// How an unread sequence element is read: by a reference with the
/// subject's capability when its pattern binds by reference, and, when
/// owned, moved into a `var` binding when its pattern binds it once.
#[derive(Clone, Copy)]
struct ElementRead {
    by_reference: bool,
    moves: bool,
}

/// One element read a case made: the hidden name holding it, its type,
/// and whether it is an owned value rather than a reference.
#[derive(Clone)]
struct Read {
    name: String,
    ty: Ty,
    owned: bool,
}

/// One projection of a subject: a tuple element, a struct field, or one of
/// upstream's sequence paths — an element by index, an element counted
/// from the end, or the slice between a fixed prefix and suffix.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Step {
    Element(usize),
    Field(String),
    SequenceElement(usize),
    SequenceFromEnd(usize),
    SequenceSlice { start: usize, end_offset: usize },
}

/// What an unread sequence element's type is spelled as in a rejection.
const UNTYPED_ELEMENT: &str = "an untyped sequence element";

/// `ty` with a reference stripped to its referent.
fn dereferenced(ty: Ty) -> Ty {
    match ty.kind() {
        TyKind::Ref(reference) => reference.referent.clone(),
        _ => ty,
    }
}

/// Whether `pattern` binds a name by `ref` or `as`, which keep the subject's
/// own capability and so need the subject borrowed as the place allows.
fn binds_by_reference(pattern: &MatchPattern) -> bool {
    match &pattern.kind {
        MatchPatternKind::Bind { mode, .. } => *mode == BindingMode::Ref,
        MatchPatternKind::As { .. } => true,
        MatchPatternKind::Tuple(items) | MatchPatternKind::Or(items) => {
            items.iter().any(binds_by_reference)
        }
        MatchPatternKind::Call { args, kwargs, .. } => {
            args.iter().any(binds_by_reference)
                || kwargs
                    .iter()
                    .any(|(_, pattern)| binds_by_reference(pattern))
        }
        MatchPatternKind::Fields(fields) => fields
            .iter()
            .any(|(_, pattern)| binds_by_reference(pattern)),
        MatchPatternKind::Sequence(items) => {
            items.iter().any(|item| binds_by_reference(item.pattern()))
        }
        MatchPatternKind::Wildcard
        | MatchPatternKind::Literal(_)
        | MatchPatternKind::Equal(_)
        | MatchPatternKind::Head(_) => false,
    }
}

/// How many names `pattern` binds to its subject itself in any one
/// alternative: a name, an `as` and its pattern's, the most of a `|`'s
/// alternatives.
fn root_bindings(pattern: &MatchPattern) -> usize {
    match &pattern.kind {
        MatchPatternKind::Bind { .. } => 1,
        MatchPatternKind::As { pattern, .. } => 1 + root_bindings(pattern),
        MatchPatternKind::Or(alternatives) => {
            alternatives.iter().map(root_bindings).max().unwrap_or(0)
        }
        _ => 0,
    }
}

/// Whether `command` may be dropped from a case whose tests the coverage
/// proof made redundant: a test, or alternatives that only test.
fn droppable_test(command: &PlannedCommand) -> bool {
    match command {
        PlannedCommand::Equal(_)
        | PlannedCommand::ListLength(_)
        | PlannedCommand::Bind(_)
        | PlannedCommand::PathValue(_) => true,
        PlannedCommand::Alternatives(alternatives) => alternatives
            .iter()
            .flatten()
            .all(|command| !matches!(command, PlannedCommand::Bind(_)) && droppable_test(command)),
    }
}

/// The cases' coverage of a subject's value space: the cells of a finite
/// space (`Bool`, a tuple of finite spaces up to 1024 cells) the unguarded
/// cases so far cover, and, over an open space, the root literal spellings
/// they already compared, which a later case repeats unreachably.
struct Coverage {
    space: Space,
    /// Per cell of a finite space, whether a case covers it; empty over an
    /// open space.
    covered: Vec<bool>,
    /// Whether an unguarded case the subject cannot miss has closed the
    /// space, finite or open.
    closed: bool,
    spellings: HashSet<String>,
}

impl Coverage {
    fn new(ty: &Ty) -> Self {
        let space = Space::of(ty);
        let covered = vec![false; space.cells().unwrap_or(0)];
        Self {
            space,
            covered,
            closed: false,
            spellings: HashSet::new(),
        }
    }

    /// Account for `case`, in order: the warning it earns when earlier cases
    /// already cover everything it can match. A guarded case adds nothing.
    fn add(&mut self, case: &MatchCase) -> Option<String> {
        let spelling = root_literal_spelling(&case.pattern);
        let cells = self.space.cover(&case.pattern);
        let warning = if self.complete() {
            Some(
                "case is unreachable; previous cases cover every value of the match subject"
                    .to_string(),
            )
        } else if let Some(spelling) = &spelling
            && self.spellings.contains(spelling)
        {
            Some(format!(
                "case is unreachable; '{spelling}' is already covered by a previous case"
            ))
        } else if !cells.is_empty()
            && cells.iter().any(|cell| *cell)
            && cells
                .iter()
                .zip(&self.covered)
                .all(|(cell, covered)| !cell || *covered)
        {
            Some(match &spelling {
                Some(spelling) => format!(
                    "case is unreachable; '{spelling}' is already covered by a previous case"
                ),
                None => {
                    "case is unreachable; previous cases cover every value of the match subject"
                        .to_string()
                }
            })
        } else {
            None
        };
        if case.guard.is_none() {
            for (covered, cell) in self.covered.iter_mut().zip(&cells) {
                *covered |= *cell;
            }
            self.closed |= irrefutable(&case.pattern);
            if let Some(spelling) = spelling {
                self.spellings.insert(spelling);
            }
        }
        warning
    }

    /// Whether the unguarded cases so far cover every value.
    fn complete(&self) -> bool {
        self.closed || (!self.covered.is_empty() && self.covered.iter().all(|covered| *covered))
    }

    /// The warning for a finite space the cases leave values of: each
    /// uncovered cell, spelled.
    fn missing(&self) -> Option<String> {
        if self.covered.is_empty() {
            return None;
        }
        let missing: Vec<String> = self
            .covered
            .iter()
            .enumerate()
            .filter(|(_, covered)| !**covered)
            .map(|(cell, _)| format!("'{}'", self.space.spell(cell)))
            .collect();
        match missing.as_slice() {
            [] => None,
            [one] => Some(format!("'match' is not exhaustive; missing case for {one}")),
            [init @ .., last] => Some(format!(
                "'match' is not exhaustive; missing cases for {} and {last}",
                init.join(", ")
            )),
        }
    }
}

/// A subject's value space as the coverage analysis sees it: `Bool`'s two
/// values, a tuple's product of its elements' spaces, or open (no finite
/// set of literals covers it).
enum Space {
    Bool,
    Product(Vec<Self>),
    Open,
}

impl Space {
    /// The largest finite product the analysis enumerates.
    const MAX_CELLS: usize = 1024;

    fn of(ty: &Ty) -> Self {
        if matches!(ty.kind(), TyKind::Bool) {
            return Self::Bool;
        }
        tuple_elements(ty).map_or(Self::Open, |items| {
            Self::Product(items.into_iter().map(Self::of).collect())
        })
    }

    /// How many cells the space has, when it is finite and enumerable.
    fn cells(&self) -> Option<usize> {
        match self {
            Self::Bool => Some(2),
            Self::Open => None,
            Self::Product(items) => items.iter().try_fold(1usize, |cells, item| {
                cells
                    .checked_mul(item.cells()?)
                    .filter(|cells| *cells <= Self::MAX_CELLS)
            }),
        }
    }

    /// The cells `pattern` covers: a wildcard or binding every one, a `Bool`
    /// literal its value, a tuple the product of its elements' cells, `|`
    /// the union, anything else none. Empty over an open space.
    fn cover(&self, pattern: &MatchPattern) -> Vec<bool> {
        let Some(cells) = self.cells() else {
            return Vec::new();
        };
        match &pattern.kind {
            MatchPatternKind::Wildcard | MatchPatternKind::Bind { .. } => vec![true; cells],
            MatchPatternKind::As { pattern, .. } => self.cover(pattern),
            MatchPatternKind::Or(alternatives) => {
                let mut union = vec![false; cells];
                for alternative in alternatives {
                    for (cell, covered) in union.iter_mut().zip(self.cover(alternative)) {
                        *cell |= covered;
                    }
                }
                union
            }
            MatchPatternKind::Literal(Expr {
                kind: ExprKind::Bool(value),
                ..
            }) if matches!(self, Self::Bool) => vec![!*value, *value],
            MatchPatternKind::Tuple(items) => match self {
                Self::Product(spaces) if spaces.len() == items.len() => {
                    let elements: Vec<Vec<bool>> = spaces
                        .iter()
                        .zip(items)
                        .map(|(space, item)| space.cover(item))
                        .collect();
                    (0..cells)
                        .map(|cell| {
                            self.decompose(cell)
                                .iter()
                                .zip(&elements)
                                .all(|(index, element)| element[*index])
                        })
                        .collect()
                }
                _ => vec![false; cells],
            },
            _ => vec![false; cells],
        }
    }

    /// A product cell as its elements' cells, first element first.
    fn decompose(&self, mut cell: usize) -> Vec<usize> {
        let Self::Product(spaces) = self else {
            return vec![cell];
        };
        let mut indexes = vec![0; spaces.len()];
        for (index, space) in spaces.iter().enumerate().rev() {
            let radix = space.cells().unwrap_or(1);
            indexes[index] = cell % radix;
            cell /= radix;
        }
        indexes
    }

    /// The value a cell stands for, as the pin spells it (`False`, `True`,
    /// a product as `False, True`).
    fn spell(&self, cell: usize) -> String {
        match self {
            Self::Bool => if cell == 0 { "False" } else { "True" }.to_string(),
            Self::Product(spaces) => self
                .decompose(cell)
                .iter()
                .zip(spaces)
                .map(|(index, space)| space.spell(*index))
                .collect::<Vec<_>>()
                .join(", "),
            Self::Open => String::new(),
        }
    }
}

/// The spelling of a root literal pattern, as a duplicate of an earlier
/// case's is reported.
fn root_literal_spelling(pattern: &MatchPattern) -> Option<String> {
    let MatchPatternKind::Literal(value) = &pattern.kind else {
        return None;
    };
    Some(match &value.kind {
        ExprKind::Int(value) => value.to_string(),
        ExprKind::Float(value) => value.to_string(),
        ExprKind::Str(value) => format!("\"{value}\""),
        ExprKind::Bool(value) => if *value { "True" } else { "False" }.to_string(),
        ExprKind::None => "None".to_string(),
        _ => return None,
    })
}
