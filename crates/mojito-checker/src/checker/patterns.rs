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
//!
//! An enum case `.Case(p)` or `Owner.Case(p)` over a subject that declares
//! the bundled `EnumLike` tests the case's index, looked up by name in the
//! subject's `_enum_case_names`, against `_get_enum_discriminant()`, then
//! compiles `p` over the payload `_unsafe_get_enum_payload[case]()`, read
//! like an element (lazily, once per case, immutably under a temporary
//! subject); a bare `.Case` tests the tag alone. Over any other subject a
//! bare head is an equality test, as upstream's.
//!
//! Each command's shape is recorded at its path for `coverage.rs`, whose
//! verdict concludes the match and seals the cases earlier ones cover.
//!
//! A `comptime match` compiles the same commands in compile-time mode, as
//! upstream emits its cases as one `hlcf.comptime.if`: the subject is a
//! compile-time name, or `comptime $match<id> = subject`, every path read
//! is the path's expression over it (no `$elem<id>`), each test is a
//! compile-time condition, and a bare or `as` binding is `comptime name =
//! path`, a compile-time value as upstream's `PValue` binding is. A case's
//! condition is the conjunction of its tests and its guard, in which a
//! binding reads as the value it denotes; a `|` is the disjunction of its
//! alternatives' conditions, and a name every alternative binds is the
//! value of the first alternative that matches (upstream's `POC::Cond`).

use super::comptime_validation::ComptimeTest;
use super::coverage::{CoverCase, CoverCommand};
use super::desugar::Synth;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::{BindingMode, MatchCase, MatchPattern, MatchPatternKind, SequencePattern};
use mojito_checked::checked::{MatchEmission, MatchPlan, PlannedCommand, PlannedRead};

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
        let paths = PathTable::new(!read_only);
        // The hidden subject lives in a scope of its own around the cases.
        self.push_scope();
        let checked = self
            .anchor_subject(&synth, &name, subject, read_only, ret, in_loop)
            .and_then(|(anchor, ty, temporary)| {
                let subject = MatchSubject::root(&synth, &paths, &name, ty, temporary);
                self.check_match_cases(stmt, &subject, anchor, cases, ret, in_loop)
            });
        self.pop_scope();
        checked
    }

    /// Check `comptime match subject:` with its `cases`: the subject is a
    /// compile-time value, each case compiles in compile-time mode to its
    /// condition and bindings in a scope of its own, and its body is
    /// checked there under what the condition proves, as a `comptime if`
    /// arm is. The plan is recorded at the statement for the conditional
    /// chain the elaborator below MIR decides.
    pub(super) fn check_comptime_match(
        &mut self,
        stmt: &Stmt,
        subject: &Expr,
        cases: &[MatchCase],
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<(), TypeError> {
        if !self.reads_compile_time_alone(subject) {
            return Err(TypeError::Unsupported(
                ComptimeTest::MatchSubject.dynamic_value().to_string(),
            ));
        }
        let synth = Synth::for_statement(stmt, subject.source.clone());
        let paths = PathTable::new(false);
        self.push_scope();
        let checked = self
            .anchor_comptime_subject(&synth, &paths, subject)
            .and_then(|(anchor, root)| {
                self.check_comptime_cases(stmt, &root, anchor, cases, ret, in_loop)
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
        let paths = PathTable::new(!read_only);
        self.push_scope();
        let checked = self
            .anchor_subject(&synth, &name, value, read_only, ret, in_loop)
            .and_then(|(anchor, ty, temporary)| {
                let subject = MatchSubject::root(&synth, &paths, &name, ty, temporary);
                let mut commands = Commands::default();
                self.compile_pattern(pattern, &subject, Join::None, &mut commands)?;
                self.check_block(body, ret, in_loop)?;
                let (program, reads, _) = commands.into_parts();
                self.record_at(pattern.source_span()).match_plan = Some(MatchPlan {
                    anchor: Some(anchor),
                    cases: vec![program],
                    reads: vec![reads],
                    emission: MatchEmission::default(),
                    conditions: Vec::new(),
                    concluded: false,
                    unreachable: Vec::new(),
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

    /// What the check proved of the `match` statement `stmt`: whether its
    /// cases cover every value of its subject, so that nothing falls
    /// through it, and which cases earlier ones already cover.
    pub(super) fn match_reach(&self, stmt: &Stmt) -> MatchReach {
        self.node_facts
            .borrow()
            .get(&stmt.source_span())
            .and_then(|facts| facts.match_plan.as_ref())
            .map(|plan| MatchReach {
                concluded: plan.concluded,
                unreachable: plan.unreachable.clone(),
            })
            .unwrap_or_default()
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

    /// The elements of the `EnumLike` value member `member` of `ty`, read
    /// at its application: `None` when `ty` is no struct or the list does
    /// not fold to known elements.
    pub(super) fn enum_like_list(
        &self,
        ty: &Ty,
        member: &str,
    ) -> Result<Option<Vec<mojito_types::param_expr::ParamExpr>>, TypeError> {
        let TyKind::Struct(name, arguments) = ty.kind() else {
            return Ok(None);
        };
        let Some(info) = self.structs.get(name) else {
            return Ok(None);
        };
        let Some(list) = self
            .struct_value_member(info, arguments, member)
            .transpose()?
        else {
            return Ok(None);
        };
        self.param_context
            .list_elements(&list)
            .map_err(super::param_error)
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
        let mut programs = Vec::new();
        let mut reads = Vec::new();
        let mut covers = Vec::new();
        for case in cases {
            (*self.uninitialized.borrow_mut()).clone_from(&before);
            self.push_scope();
            let checked = self.check_case(case, subject, ret, in_loop);
            self.pop_scope();
            let (program, case_reads, commands) = checked?.into_parts();
            programs.push(program);
            reads.push(case_reads);
            covers.push(CoverCase {
                commands,
                guarded: case.guard.is_some(),
                span: case.pattern.source_span(),
            });
            exits.push(self.uninitialized.borrow().clone());
        }
        let verdict = self.match_coverage(&subject.ty, &covers, &stmt.source_span())?;
        let emission = self.plan_match_emission(&subject.ty, &covers, verdict.concluding)?;
        // Nothing falls through a concluded match, as its emission seals
        // the fall-through, and no case earlier ones cover is reached.
        let reached = exits
            .into_iter()
            .zip(&verdict.unreachable)
            .filter_map(|(exit, unreachable)| (!unreachable).then_some(exit));
        let fall_through = verdict.concluding.is_none().then_some(before);
        *self.uninitialized.borrow_mut() = reached
            .chain(fall_through)
            .flatten()
            .collect::<HashSet<_>>();
        let mut facts = self.record_at(stmt.source_span());
        facts.match_plan = Some(MatchPlan {
            anchor: Some(anchor),
            cases: programs,
            reads,
            emission,
            conditions: Vec::new(),
            concluded: verdict.concluding.is_some(),
            unreachable: verdict.unreachable,
        });
        facts.match_warnings = (!verdict.warnings.is_empty()).then_some(verdict.warnings);
        Ok(())
    }

    /// The root a `comptime match`'s patterns read, and the statement
    /// anchoring it: a type, or a compile-time name (a value parameter, a
    /// `comptime` binding, a `comptime for` variable, `Self.n`), is read
    /// where it is, as upstream's parameter value of a declaration is the
    /// reference itself; any other subject is bound once by `comptime
    /// $match<id> = subject`, so an application is demanded once.
    fn anchor_comptime_subject<'s>(
        &mut self,
        synth: &'s Synth,
        paths: &'s PathTable,
        subject: &Expr,
    ) -> Result<(Option<Stmt>, MatchSubject<'s>), TypeError> {
        let name = format!("$match{}", synth.parent().0);
        let named = self.comptime_type_operand(subject).ok().flatten().is_some()
            || matches!(
                subject.kind,
                ExprKind::Identifier(_) | ExprKind::Member { .. }
            ) && self.parameter_value_ty(subject).is_some();
        if named {
            // The name's declared type: reading it is no runtime
            // materialization.
            let declared = self.parameter_value_ty(subject).cloned();
            let ty = declared
                .map_or_else(|| self.infer(subject), Ok)
                .map_or_else(|_| Ty::error(), dereferenced);
            let mut root = MatchSubject::root(synth, paths, &name, ty, false);
            root.reading = Reading::Comptime(Some(subject.clone()));
            return Ok((None, root));
        }
        let anchor = synth.stmt(StmtKind::Comptime {
            name: name.clone(),
            type_params: Vec::new(),
            ty: None,
            where_clauses: Vec::new(),
            value: subject.clone(),
        });
        self.check_comptime_declaration(&anchor, &name)?;
        let ty = self.lookup(&name).cloned().ok_or_else(|| {
            TypeError::InvariantViolation("the comptime match subject was not declared".into())
        })?;
        let mut root = MatchSubject::root(synth, paths, &name, dereferenced(ty), false);
        root.reading = Reading::Comptime(None);
        Ok((Some(anchor), root))
    }

    /// Check the synthesized local `comptime name = value` `declaration` as
    /// a block's own is checked: the binding is a compile-time one.
    fn check_comptime_declaration(
        &mut self,
        declaration: &Stmt,
        name: &str,
    ) -> Result<(), TypeError> {
        self.check_stmt(declaration, None, false)?;
        if let Some(owner) = self.lookup_owner(name) {
            self.comptime_binding_owners.insert(owner);
        }
        Ok(())
    }

    /// [`Self::check_match_cases`] for a `comptime match`: each case's
    /// condition and bindings, and its body under the condition, then the
    /// coverage verdict, which drops the concluding case's condition, so
    /// that it is the chain's else arm, and seals the cases earlier ones
    /// cover.
    fn check_comptime_cases(
        &mut self,
        stmt: &Stmt,
        subject: &MatchSubject,
        anchor: Option<Stmt>,
        cases: &[MatchCase],
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<(), TypeError> {
        let before = self.uninitialized.borrow().clone();
        let mut exits = Vec::new();
        let mut programs = Vec::new();
        let mut conditions = Vec::new();
        let mut covers = Vec::new();
        for case in cases {
            (*self.uninitialized.borrow_mut()).clone_from(&before);
            self.push_scope();
            let checked = self.check_comptime_case(case, subject, ret, in_loop);
            self.pop_scope();
            let ComptimeCase {
                declarations,
                condition,
                cover,
            } = checked?;
            programs.push(declarations);
            conditions.push(condition);
            covers.push(CoverCase {
                commands: cover,
                guarded: case.guard.is_some(),
                span: case.pattern.source_span(),
            });
            exits.push(self.uninitialized.borrow().clone());
        }
        let verdict = self.match_coverage(&subject.ty, &covers, &stmt.source_span())?;
        // The concluding case's tests pass whenever it is reached, so it is
        // the else arm.
        if let Some(index) = verdict.concluding {
            conditions[index] = None;
        }
        let reached = exits
            .into_iter()
            .zip(&verdict.unreachable)
            .filter_map(|(exit, unreachable)| (!unreachable).then_some(exit));
        let fall_through = verdict.concluding.is_none().then_some(before);
        *self.uninitialized.borrow_mut() = reached
            .chain(fall_through)
            .flatten()
            .collect::<HashSet<_>>();
        let mut facts = self.record_at(stmt.source_span());
        facts.match_plan = Some(MatchPlan {
            anchor,
            reads: vec![Vec::new(); programs.len()],
            cases: programs,
            emission: MatchEmission::default(),
            conditions,
            concluded: verdict.concluding.is_some(),
            unreachable: verdict.unreachable,
        });
        facts.match_warnings = (!verdict.warnings.is_empty()).then_some(verdict.warnings);
        Ok(())
    }

    /// One `comptime match` case in its own scope: its pattern's tests and
    /// bindings, each binding declared as `comptime name = path`, then its
    /// guard over what the bindings denote, the conjunction of the tests
    /// and the guard checked and recorded as a `comptime if` condition,
    /// and its body under what that condition proves. The bindings, the
    /// condition (none when the case always matches), and the shapes
    /// coverage reads.
    fn check_comptime_case(
        &mut self,
        case: &MatchCase,
        subject: &MatchSubject,
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<ComptimeCase, TypeError> {
        let synth = subject.synth;
        let mut commands = Commands::default();
        self.compile_pattern(&case.pattern, subject, Join::None, &mut commands)?;
        let mut declarations = Vec::new();
        for binding in &commands.bindings {
            let declaration = synth.stmt(StmtKind::Comptime {
                name: binding.name.clone(),
                type_params: Vec::new(),
                ty: None,
                where_clauses: Vec::new(),
                value: synth.refresh(&binding.value),
            });
            self.check_comptime_declaration(&declaration, &binding.name)?;
            self.record_at(binding.span.clone()).binding = self.lookup_owner(&binding.name);
            declarations.push(PlannedCommand::Bind {
                declaration: Box::new(declaration),
                reads: Vec::new(),
            });
        }
        let mut conjuncts = std::mem::take(&mut commands.conditions);
        if let Some(guard) = &case.guard {
            let guard = synth.refresh(&substitute_bindings(guard, &commands.bindings));
            self.register_named_bindings(&guard)?;
            self.check_comptime_condition(&guard, ComptimeTest::If)?;
            conjuncts.push(guard);
        }
        let condition = conjoin(synth, conjuncts, InfixOp::And);
        let constraint = condition
            .as_ref()
            .map(|condition| self.check_comptime_condition(condition, ComptimeTest::If))
            .transpose()?
            .flatten();
        let checked = match &condition {
            Some(condition) => {
                // What the condition proves holds in this arm alone.
                self.assume_condition(condition, constraint.as_ref())?;
                let checked = self.check_block(&case.body, ret, in_loop);
                self.pop_assumptions();
                checked
            }
            None => self.check_block(&case.body, ret, in_loop),
        };
        checked?;
        Ok(ComptimeCase {
            declarations,
            condition,
            cover: commands.cover,
        })
    }

    /// One case in its own scope: its pattern's commands, then its body.
    fn check_case(
        &mut self,
        case: &MatchCase,
        subject: &MatchSubject,
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<Commands, TypeError> {
        let mut commands = Commands::default();
        self.compile_pattern(&case.pattern, subject, Join::None, &mut commands)?;
        if let Some(guard) = &case.guard {
            self.register_named_bindings(guard)?;
            self.expect_bool(guard, "case guard")?;
        }
        self.check_block(&case.body, ret, in_loop)?;
        Ok(commands)
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
            // `None` parses as a literal but is no literal pattern, as
            // upstream's `SimpleLiteralNode` builds no command for it.
            MatchPatternKind::Literal {
                value:
                    Expr {
                        kind: ExprKind::None,
                        ..
                    },
                ..
            } => Err(TypeError::MatchPattern(
                "expression is not a valid match pattern; use '==' to compare against its value"
                    .into(),
            )),
            MatchPatternKind::Literal { value, spelling }
            | MatchPatternKind::Equal { value, spelling } => {
                self.equality_test(value, spelling.as_deref(), subject, commands)
            }
            MatchPatternKind::Tuple(items) => {
                self.compile_tuple_pattern(items, subject, join, commands)
            }
            MatchPatternKind::Call { head, args, kwargs } if self.is_enum_like(subject) => {
                self.compile_enum_case(head, args, kwargs, subject, join, commands)
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
            MatchPatternKind::Head(head) if self.is_enum_like(subject) => {
                let case = self.enum_case(head, subject)?.0;
                let (tested, reads) = self.materialize(subject, commands)?;
                self.enum_tag_test(case, &tested, reads, commands)?;
                commands.cover.push(CoverCommand::EnumTag {
                    path: subject.path.clone(),
                    case,
                });
                Ok(())
            }
            MatchPatternKind::Head(head) => self.equality_test(head, None, subject, commands),
            MatchPatternKind::Sequence(items) => {
                self.compile_sequence_pattern(items, subject, join, commands)
            }
        }
    }

    /// `p0 | p1 | ...`: each alternative's commands as a list of its own,
    /// the first that matches selecting. Every alternative binds the same
    /// names, the first's declarations being the bindings the later ones
    /// assign, each with the first's type. An alternative names a path read
    /// the commands before the `|` made, and reads afresh any other.
    fn compile_or_pattern(
        &mut self,
        alternatives: &[MatchPattern],
        subject: &MatchSubject,
        mut join: Join<'_>,
        commands: &mut Commands,
    ) -> Result<(), TypeError> {
        if subject.comptime() {
            return self.compile_comptime_or_pattern(alternatives, subject, join, commands);
        }
        let mut first: Option<Vec<String>> = None;
        let mut programs = Vec::new();
        let mut covers = Vec::new();
        for alternative in alternatives {
            let mut program = Commands {
                cached: commands.cached.clone(),
                reads: std::mem::take(&mut commands.reads),
                ..Commands::default()
            };
            let mut bound = Vec::new();
            let inner = if matches!(join, Join::Assign(_)) || first.is_some() {
                Join::Assign(&mut bound)
            } else {
                Join::Declare(&mut bound)
            };
            let compiled = self.compile_pattern(alternative, subject, inner, &mut program);
            commands.reads = std::mem::take(&mut program.reads);
            compiled?;
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
            programs.push(program.program);
            covers.push(program.cover);
        }
        if let Some(names) = first {
            join.record(names);
        }
        commands
            .program
            .push(PlannedCommand::Alternatives(programs));
        commands.cover.push(CoverCommand::Or(covers));
        Ok(())
    }

    /// [`Self::compile_or_pattern`] in compile-time mode: the condition is
    /// the disjunction of the alternatives' conditions, and each name every
    /// alternative binds is the value of the first alternative that
    /// matches, `v0 if c0 else v1 if c1 else ...`, as upstream merges it
    /// into a `POC::Cond`; every alternative binds the same names at one
    /// type.
    fn compile_comptime_or_pattern(
        &mut self,
        alternatives: &[MatchPattern],
        subject: &MatchSubject,
        mut join: Join<'_>,
        commands: &mut Commands,
    ) -> Result<(), TypeError> {
        let synth = subject.synth;
        let mut compiled: Vec<(Option<Expr>, Vec<ComptimeBinding>)> = Vec::new();
        let mut covers = Vec::new();
        for alternative in alternatives {
            let mut program = Commands::default();
            let mut bound = Vec::new();
            self.compile_pattern(
                alternative,
                subject,
                Join::Declare(&mut bound),
                &mut program,
            )?;
            if let Some((_, first)) = compiled.first() {
                let first: Vec<&str> = first.iter().map(|binding| binding.name.as_str()).collect();
                if let Some(missing) = first
                    .iter()
                    .find(|name| !bound.iter().any(|bound| bound == *name))
                    .map(|name| (*name).to_string())
                    .or_else(|| {
                        bound
                            .iter()
                            .find(|name| !first.contains(&name.as_str()))
                            .cloned()
                    })
                {
                    return Err(TypeError::MatchPattern(format!(
                        "or-pattern alternatives must bind the same names; '{missing}' is \
                         bound in one alternative but not the other"
                    )));
                }
            }
            let condition = conjoin(synth, std::mem::take(&mut program.conditions), InfixOp::And);
            compiled.push((condition, program.bindings));
            covers.push(program.cover);
        }
        let conditions = compiled
            .iter()
            .map(|(condition, _)| condition.as_ref().map(|condition| synth.refresh(condition)))
            .collect::<Option<Vec<_>>>();
        let first = compiled
            .first()
            .map(|(_, first)| first.clone())
            .unwrap_or_default();
        let mut names = Vec::new();
        for binding in &first {
            let declared = dereferenced(self.infer(&binding.value)?);
            let mut values = Vec::new();
            for (condition, bindings) in &compiled {
                let Some(value) = bindings.iter().find(|bound| bound.name == binding.name) else {
                    continue;
                };
                let found = dereferenced(self.infer(&value.value)?);
                if found != declared {
                    return Err(TypeError::MatchPattern(format!(
                        "or-pattern binding '{}' has incompatible types across alternatives: \
                         first alternative has type '{declared}', this alternative has type \
                         '{found}'",
                        binding.name
                    )));
                }
                values.push((condition, &value.value));
            }
            let mut merged = values.pop().map(|(_, value)| synth.refresh(value));
            while let Some((condition, value)) = values.pop() {
                let value = synth.refresh(value);
                merged = Some(match (condition, merged) {
                    (Some(condition), Some(otherwise)) => synth.expr(ExprKind::IfExpr {
                        cond: Box::new(synth.refresh(condition)),
                        then_branch: Box::new(value),
                        else_branch: Box::new(otherwise),
                    }),
                    _ => value,
                });
            }
            if let Some(value) = merged {
                names.push(binding.name.clone());
                commands.bindings.push(ComptimeBinding {
                    name: binding.name.clone(),
                    value,
                    span: binding.span.clone(),
                });
            }
        }
        join.record(names);
        if let Some(condition) =
            conditions.and_then(|conditions| conjoin(synth, conditions, InfixOp::Or))
        {
            commands.conditions.push(condition);
        }
        commands.cover.push(CoverCommand::Or(covers));
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
        if subject.untyped() {
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
        if subject.untyped() {
            return Err(TypeError::MatchPattern(format!(
                "{UNTYPED_ELEMENT} does not conform to 'Sized'"
            )));
        }
        if matches!(
            subject.ty.kind(),
            TyKind::INT | TyKind::UINT | TyKind::FLOAT64 | TyKind::Simd { .. }
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
        if subject.comptime() && star.is_some() {
            return Err(TypeError::Unsupported(
                "a starred sequence pattern in 'comptime match'".into(),
            ));
        }
        let (tested, reads) = self.materialize(subject, commands)?;
        let test = synth.expr(ExprKind::Call {
            name: length_matches,
            param_args: Vec::new(),
            args: vec![
                tested.value(),
                synth.int(count - usize::from(star.is_some())),
                synth.expr(ExprKind::Bool(star.is_none())),
            ],
            kwargs: Vec::new(),
        });
        self.push_test(test, subject, commands, |test| PlannedCommand::ListLength {
            test,
            reads,
        })?;
        commands.cover.push(CoverCommand::ListLength {
            path: subject.path.clone(),
        });
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
            let child = subject.read(
                step,
                Ty::error(),
                ElementRead {
                    by_reference: subject.paths.mutable_reads,
                    moves: root_bindings(pattern) <= 1,
                },
            );
            self.compile_pattern(pattern, &child, join.reborrow(), commands)?;
        }
        Ok(())
    }

    /// The subject a test or binding reads: `subject` itself, or, below a
    /// sequence element or an enum payload, the hidden `$elem<id>` holding
    /// its read under the path's remaining steps, and the chain of the
    /// case's reads the path needs, outermost first. Each read along the
    /// path is declared by this case's (or `|` alternative's) first command
    /// that needs it, as `ref $elem<id> = parent[i]` over a reference result
    /// (to read unless the match binds by reference) and `var $elem<id> =
    /// parent[i]` over an owned one, and named by every later command; the
    /// emission makes it when the first command it emits needs it.
    fn materialize<'s>(
        &mut self,
        subject: &MatchSubject<'s>,
        commands: &mut Commands,
    ) -> Result<(MatchSubject<'s>, Vec<u32>), TypeError> {
        // A compile-time path is its expression, as upstream's parameter
        // path value is.
        if subject.comptime() {
            return Ok((
                MatchSubject {
                    reads: Vec::new(),
                    ..subject.clone()
                },
                Vec::new(),
            ));
        }
        let mut base = (subject.name.clone(), 0);
        let mut last = None;
        let mut chain = Vec::new();
        for &(depth, element) in &subject.reads {
            let key = subject.path[..depth].to_vec();
            let (index, read) = if let Some(cached) = commands.cached.get(&key) {
                cached.clone()
            } else {
                let value = spell(subject.synth, &base.0, None, &subject.path[base.1..depth]);
                let (read, declaration) = self.read_path(subject.synth, value, element)?;
                let index = u32::try_from(commands.reads.len()).map_err(|_| {
                    TypeError::InvariantViolation("too many pattern path reads".into())
                })?;
                commands.reads.push(PlannedRead {
                    path: subject.paths.id(&key),
                    declaration,
                });
                commands.cached.insert(key, (index, read.clone()));
                (index, read)
            };
            chain.push(index);
            base = (read.name.clone(), depth);
            last = Some((read, element));
        }
        let Some((read, element)) = last else {
            return Ok((subject.clone(), chain));
        };
        let path = subject.path[base.1..].to_vec();
        let whole = path.is_empty();
        let subject = MatchSubject {
            synth: subject.synth,
            paths: subject.paths,
            name: read.name,
            ty: if whole { read.ty } else { subject.ty.clone() },
            temporary: false,
            rvalue: subject.rvalue,
            root: false,
            owned: (whole && read.owned).then_some(element.moves),
            reading: Reading::Runtime,
            reads: Vec::new(),
            path,
        };
        Ok((subject, chain))
    }

    /// Declare the hidden `$elem<id>` over the path read `value`: a
    /// reference over a reference result, to read unless `element` binds
    /// by reference, and an owned value otherwise. The read and its
    /// declaration.
    fn read_path(
        &mut self,
        synth: &Synth,
        value: Expr,
        element: ElementRead,
    ) -> Result<(Read, Stmt), TypeError> {
        let value_span = value.source_span();
        let name = format!("$elem{}", value.syntax_id.0);
        self.infer(&value)?;
        let reference = self.infer_reference_value(&value).is_some();
        let declaration = synth.stmt(if reference {
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
            TypeError::InvariantViolation("a pattern path read was not declared".into())
        })?;
        let read = Read {
            name,
            ty: dereferenced(ty),
            owned: !reference,
        };
        Ok((read, declaration))
    }

    /// `Name(field=p, ...)` over a subject of the nominal type `Name`: no
    /// positional argument, then the field patterns. The head names the
    /// type; it runs no constructor. A member head names no type here: the
    /// subject is no enum to select its case.
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
            return Err(TypeError::MatchPattern(match &head.kind {
                ExprKind::Member { object, .. } if is_contextual(object) => {
                    "cannot resolve inferred member without a contextual type".to_string()
                }
                ExprKind::Member { object, field } => {
                    format!(
                        "'{}' value has no attribute '{field}'",
                        head_spelling(object)
                    )
                }
                _ => "a pattern head must name a struct type or an enum case".to_string(),
            }));
        };
        if subject.untyped() {
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

    /// `.Case(p)` or `Owner.Case(p)` over an `EnumLike` subject, checked in
    /// upstream's order: the head's shape and owner, the case, its payload
    /// type, then the one positional payload pattern. The tag test, then
    /// `p` over the payload `subject._unsafe_get_enum_payload[case]()`.
    fn compile_enum_case(
        &mut self,
        head: &Expr,
        args: &[MatchPattern],
        kwargs: &[(String, MatchPattern)],
        subject: &MatchSubject,
        join: Join<'_>,
        commands: &mut Commands,
    ) -> Result<(), TypeError> {
        if !matches!(head.kind, ExprKind::Member { .. }) {
            return Err(TypeError::MatchPattern(
                "enum case pattern must be written as 'Type.Case(...)' or '.Case(...)'".into(),
            ));
        }
        let (case, name) = self.enum_case(head, subject)?;
        let payload = self.enum_case_payload(&subject.ty, case)?;
        if matches!(payload.kind(), TyKind::None) {
            return Err(TypeError::MatchPattern(format!(
                "enum case '{name}' has no associated value"
            )));
        }
        let pattern = match (args, kwargs) {
            ([], []) => {
                return Err(TypeError::MatchPattern(format!(
                    "enum case '{name}' requires a payload pattern inside the parentheses"
                )));
            }
            (_, [_, ..]) => {
                return Err(TypeError::MatchPattern(
                    "enum case patterns do not support keyword arguments".into(),
                ));
            }
            ([pattern], []) => pattern,
            _ => {
                return Err(TypeError::MatchPattern(
                    "enum case patterns currently support at most one payload subpattern".into(),
                ));
            }
        };
        let (tested, reads) = self.materialize(subject, commands)?;
        self.enum_tag_test(case, &tested, reads, commands)?;
        commands.cover.push(CoverCommand::EnumTag {
            path: subject.path.clone(),
            case,
        });
        let child = subject.read(
            Step::EnumPayload(case),
            payload,
            ElementRead {
                by_reference: subject.paths.mutable_reads && !subject.rvalue,
                moves: root_bindings(pattern) <= 1,
            },
        );
        self.compile_pattern(pattern, &child, join, commands)
    }

    /// The case an enum head `.Case` or `Owner.Case` selects over the
    /// `EnumLike` subject, and its name. A qualified head at the root
    /// names the subject's own type, its parameters aside, as upstream
    /// checks it; nested, the owner goes unchecked, as upstream's.
    fn enum_case(&self, head: &Expr, subject: &MatchSubject) -> Result<(usize, String), TypeError> {
        let ExprKind::Member { object, field } = &head.kind else {
            return Err(TypeError::MatchPattern(
                "enum case pattern must be written as 'Type.Case(...)' or '.Case(...)'".into(),
            ));
        };
        if subject.root && !is_contextual(object) {
            self.check_enum_owner(object, &subject.ty)?;
        }
        Ok((self.enum_case_index(&subject.ty, field)?, field.clone()))
    }

    /// Require the qualified owner `owner` of an enum case to name the
    /// subject type's nominal family, parameters ignored.
    fn check_enum_owner(&self, owner: &Expr, subject: &Ty) -> Result<(), TypeError> {
        let family = |ty: &Ty| match ty.kind() {
            TyKind::Struct(name, _) => Some(name.clone()),
            _ => None,
        };
        let owner_family = match &owner.kind {
            ExprKind::Identifier(name) => {
                self.structs.get(name).map(|_| name.clone()).or_else(|| {
                    self.comptime_type_operand(owner)
                        .ok()
                        .flatten()
                        .and_then(|ty| family(&ty))
                })
            }
            _ => self
                .comptime_type_operand(owner)
                .ok()
                .flatten()
                .or_else(|| self.member_type_operand(owner))
                .and_then(|ty| family(&ty)),
        };
        if owner_family.is_some() && owner_family == family(subject) {
            return Ok(());
        }
        Err(TypeError::MatchPattern(format!(
            "cannot match value of type '{subject}' against enum case of type '{}'",
            head_spelling(owner)
        )))
    }

    /// Whether `subject` is a typed value whose type declares the bundled
    /// `EnumLike`, which makes its patterns enum cases.
    fn is_enum_like(&self, subject: &MatchSubject) -> bool {
        !subject.untyped() && self.conforms_to(&subject.ty, &enum_like_trait())
    }

    /// The index of the case `name` in `ty`'s `_enum_case_names`, the first
    /// of a repeated name, as upstream reads it.
    fn enum_case_index(&self, ty: &Ty, name: &str) -> Result<usize, TypeError> {
        let Some(names) = self.enum_like_list(ty, "_enum_case_names")? else {
            return Err(TypeError::MatchPattern(
                "cannot match on a parametric enum type".into(),
            ));
        };
        names
            .iter()
            .position(|case| {
                matches!(case.as_constant(),
                    Some(CtValue::KgenString(case) | CtValue::Str(case)) if case == name)
            })
            .ok_or_else(|| TypeError::MatchPattern(format!("'{name}' is not a case of '{ty}'")))
    }

    /// The payload type of case `index` in `ty`'s `_enum_case_types`, read
    /// lazily: only a payload pattern needs the list to reach its case.
    fn enum_case_payload(&self, ty: &Ty, index: usize) -> Result<Ty, TypeError> {
        self.enum_like_list(ty, "_enum_case_types")?
            .and_then(|types| types.into_iter().nth(index))
            .map(mojito_types::types::DependentType::resolve)
            .map(|payload| subject_binder(payload, ty))
            .ok_or_else(|| {
                TypeError::MatchPattern(
                    "'_enum_case_types' must be a parameter list covering every case".into(),
                )
            })
    }

    /// Push the tag test of `case` over `subject`, whose path needs the
    /// case's `reads`: the discriminant `var $tag<id> =
    /// subject._get_enum_discriminant()`, then `$tag<id> == case`, so that a
    /// cluster reads it once, as upstream's `emitTestableValue`; in
    /// compile-time mode the conjunct `subject._get_enum_discriminant() ==
    /// case`.
    fn enum_tag_test(
        &mut self,
        case: usize,
        subject: &MatchSubject,
        reads: Vec<u32>,
        commands: &mut Commands,
    ) -> Result<(), TypeError> {
        let synth = subject.synth;
        let discriminant = synth.expr(ExprKind::MethodCall {
            object: Box::new(subject.value()),
            method: "_get_enum_discriminant".to_string(),
            args: Vec::new(),
            kwargs: Vec::new(),
        });
        if subject.comptime() {
            let test = synth.expr(ExprKind::Infix(
                InfixOp::Eq,
                Box::new(discriminant),
                Box::new(synth.int(case)),
            ));
            self.check_comptime_condition(&test, ComptimeTest::MatchSubject)?;
            commands.conditions.push(test);
            return Ok(());
        }
        let name = format!("$tag{}", discriminant.syntax_id.0);
        let tag = synth.stmt(StmtKind::VarDecl {
            name: name.clone(),
            ty: None,
            value: discriminant,
        });
        self.check_stmt(&tag, None, false)?;
        let test = synth.expr(ExprKind::Infix(
            InfixOp::Eq,
            Box::new(synth.identifier(&name)),
            Box::new(synth.int(case)),
        ));
        self.push_test(test, subject, commands, |test| PlannedCommand::EnumTag {
            tag: Box::new(tag),
            test,
            case,
            reads,
        })
    }

    /// Push the test `subject == value` through the selected `__eq__`,
    /// the literal operand's `spelling` keying its coverage.
    fn equality_test(
        &mut self,
        value: &Expr,
        spelling: Option<&str>,
        subject: &MatchSubject,
        commands: &mut Commands,
    ) -> Result<(), TypeError> {
        commands.cover.push(CoverCommand::Equal {
            path: subject.path.clone(),
            spelling: spelling.map(str::to_string),
        });
        let (subject, reads) = self.materialize(subject, commands)?;
        let test = subject.synth.expr(ExprKind::Infix(
            InfixOp::Eq,
            Box::new(subject.value()),
            Box::new(value.clone()),
        ));
        self.register_named_bindings(&test)?;
        self.push_test(test, &subject, commands, |test| PlannedCommand::Equal {
            test,
            reads,
        })
    }

    /// Push the test `test` over `subject`: a `Bool` the case's program
    /// branches on, or in compile-time mode a conjunct of the case's
    /// condition, a compile-time `Bool` whose dynamic operand is the pin's
    /// `match subject` rejection.
    fn push_test(
        &self,
        test: Expr,
        subject: &MatchSubject,
        commands: &mut Commands,
        planned: impl FnOnce(Expr) -> PlannedCommand,
    ) -> Result<(), TypeError> {
        if subject.comptime() {
            self.check_comptime_condition(&test, ComptimeTest::MatchSubject)?;
            commands.conditions.push(test);
        } else {
            self.expect_bool(&test, "case pattern")?;
            commands.program.push(planned(test));
        }
        Ok(())
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
        if subject.untyped() {
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
        commands.cover.push(CoverCommand::Bind {
            path: subject.path.clone(),
        });
        if subject.comptime() {
            return comptime_binding(name, binding, pattern_span, subject, join, commands);
        }
        let (subject, reads) = self.materialize(subject, commands)?;
        let subject = &subject;
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
                commands.program.push(PlannedCommand::Bind {
                    declaration: Box::new(assignment),
                    reads,
                });
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
        commands.program.push(PlannedCommand::Bind {
            declaration: Box::new(declaration),
            reads,
        });
        Ok(())
    }
}

/// Whether `pattern` matches every value of its subject's type: a wildcard
/// or a binding, at every position of a tuple or `as`, in some `|`
/// alternative, or at every named field of a struct pattern.
///
/// An enum case (a member head) tests its tag, so it is refutable.
pub fn irrefutable(pattern: &MatchPattern) -> bool {
    match &pattern.kind {
        MatchPatternKind::Wildcard | MatchPatternKind::Bind { .. } => true,
        MatchPatternKind::Tuple(items) => items.iter().all(irrefutable),
        MatchPatternKind::As { pattern, .. } => irrefutable(pattern),
        MatchPatternKind::Or(items) => items.iter().any(irrefutable),
        MatchPatternKind::Call { head, args, kwargs } => {
            matches!(
                head.kind,
                ExprKind::Identifier(_) | ExprKind::TypeApply { .. }
            ) && args.iter().all(irrefutable)
                && kwargs.iter().all(|(_, pattern)| irrefutable(pattern))
        }
        MatchPatternKind::Fields(fields) => fields.iter().all(|(_, pattern)| irrefutable(pattern)),
        MatchPatternKind::Literal { .. }
        | MatchPatternKind::Equal { .. }
        | MatchPatternKind::Sequence(_)
        | MatchPatternKind::Head(_) => false,
    }
}

/// A `match` statement's coverage facts, as the return and initialization
/// analyses read them: whether nothing falls through it, and, per case,
/// whether earlier cases already cover every value it matches (none when
/// the check recorded no plan).
#[derive(Default)]
pub(super) struct MatchReach {
    pub(super) concluded: bool,
    pub(super) unreachable: Vec<bool>,
}

impl MatchReach {
    /// The cases of `cases` a value reaches.
    pub(super) fn reachable<'c>(
        &self,
        cases: &'c [MatchCase],
    ) -> impl Iterator<Item = &'c MatchCase> + use<'c, '_> {
        cases
            .iter()
            .enumerate()
            .filter(|(index, _)| !self.unreachable.get(*index).copied().unwrap_or(false))
            .map(|(_, case)| case)
    }
}

/// One projection of a subject: a tuple element, a struct field, one of
/// upstream's sequence paths — an element by index, an element counted
/// from the end, or the slice between a fixed prefix and suffix — or an
/// enum case's payload.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) enum Step {
    Element(usize),
    Field(String),
    SequenceElement(usize),
    SequenceFromEnd(usize),
    SequenceSlice { start: usize, end_offset: usize },
    EnumPayload(usize),
}

/// The subject a pattern applies to: the hidden `$match<id>` reference the
/// anchor declared (or the hidden `$elem<id>` a path was read into),
/// projected by `path` to the element, field, or payload the pattern's
/// position names, with the node factory that spells it.
#[derive(Clone)]
struct MatchSubject<'a> {
    synth: &'a Synth,
    /// The statement's path ids and read mode, shared by its cases.
    paths: &'a PathTable,
    name: String,
    /// The projection's type, any reference stripped; unknown for an
    /// unread sequence element, as upstream's list element path is
    /// untyped.
    ty: Ty,
    /// Whether the whole subject is a temporary the anchor materializes
    /// rather than a place it borrows; a projection never is.
    temporary: bool,
    /// Whether the whole subject is a temporary, which every projection
    /// inherits: an enum payload read from it is immutable, as a `ref
    /// self` getter's result over an rvalue is.
    rvalue: bool,
    /// Whether this is the whole subject, where a qualified enum case's
    /// owner is checked.
    root: bool,
    /// For an owned path read, an rvalue: whether a `var` binding takes it
    /// by transfer, being the only binding of it.
    owned: Option<bool>,
    /// Whether the subject is a `comptime match`'s, read in compile-time
    /// mode.
    reading: Reading,
    /// The path's reads (sequence elements and enum payloads), each the
    /// length of the path prefix it reads and how it is read.
    reads: Vec<(usize, ElementRead)>,
    path: Vec<Step>,
}

impl<'a> MatchSubject<'a> {
    /// The whole subject, the hidden `name` the anchor declared.
    fn root(synth: &'a Synth, paths: &'a PathTable, name: &str, ty: Ty, temporary: bool) -> Self {
        MatchSubject {
            synth,
            paths,
            name: name.to_string(),
            ty,
            temporary,
            rvalue: temporary,
            root: true,
            owned: None,
            reading: Reading::Runtime,
            reads: Vec::new(),
            path: Vec::new(),
        }
    }

    /// The expression reading this subject: `$match<id>` under the path's
    /// subscripts, field selections, and payload reads.
    fn value(&self) -> Expr {
        let base = match &self.reading {
            Reading::Comptime(base) => base.as_ref(),
            Reading::Runtime => None,
        };
        spell(self.synth, &self.name, base, &self.path)
    }

    /// Whether this is a `comptime match` subject, read in compile-time
    /// mode.
    const fn comptime(&self) -> bool {
        matches!(self.reading, Reading::Comptime(_))
    }

    /// Whether this is an unread sequence element, whose type is unknown,
    /// as upstream's list element path is untyped.
    fn untyped(&self) -> bool {
        matches!(
            self.path.last(),
            Some(Step::SequenceElement(_) | Step::SequenceFromEnd(_) | Step::SequenceSlice { .. })
        )
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
            paths: self.paths,
            name: self.name.clone(),
            ty,
            temporary: false,
            rvalue: self.rvalue,
            root: false,
            owned: None,
            reading: self.reading.clone(),
            reads: self.reads.clone(),
            path,
        }
    }

    /// The unread element, slice, or payload `step` of this subject, of
    /// type `ty`, read as `element` says.
    fn read(&self, step: Step, ty: Ty, element: ElementRead) -> Self {
        let mut child = self.project(step, ty);
        child.reads.push((child.path.len(), element));
        child
    }
}

/// How a subject's patterns read it: at run time, or at compile time in a
/// `comptime match`, through the hidden name or the compile-time name read
/// in its place (`T`, `Self.n`).
#[derive(Clone)]
enum Reading {
    Runtime,
    Comptime(Option<Expr>),
}

/// A case's commands as they are collected: its command list in pattern
/// order, each command's shape at its path aligned with it (as coverage
/// reads upstream's command list), the case's path reads, and the reads
/// visible here by path, as upstream's per-path values — inside a `|`
/// alternative, those made before it too.
/// In compile-time mode the tests are `conditions` and the bindings
/// `bindings` instead.
#[derive(Default)]
struct Commands {
    program: Vec<PlannedCommand>,
    cover: Vec<CoverCommand>,
    reads: Vec<PlannedRead>,
    cached: HashMap<Vec<Step>, (u32, Read)>,
    conditions: Vec<Expr>,
    bindings: Vec<ComptimeBinding>,
}

impl Commands {
    /// The command list the case runs, its path reads, and the shapes
    /// coverage reads.
    fn into_parts(self) -> (Vec<PlannedCommand>, Vec<PlannedRead>, Vec<CoverCommand>) {
        (self.program, self.reads, self.cover)
    }
}

/// A statement's pattern paths from its subject, each numbered once, as
/// upstream uniques its `PatternPath`s per `match`, and how its path reads
/// are made: keeping the subject's capability when some case binds by
/// reference (`mutable_reads`), so that the reads of one path have one type
/// across the cases, which a cluster shares.
struct PathTable {
    ids: RefCell<Vec<Vec<Step>>>,
    mutable_reads: bool,
}

impl PathTable {
    const fn new(mutable_reads: bool) -> Self {
        Self {
            ids: RefCell::new(Vec::new()),
            mutable_reads,
        }
    }

    /// The id of `path`, numbered on its first use.
    fn id(&self, path: &[Step]) -> u32 {
        let mut paths = self.ids.borrow_mut();
        let index = paths
            .iter()
            .position(|known| known == path)
            .unwrap_or_else(|| {
                paths.push(path.to_vec());
                paths.len() - 1
            });
        u32::try_from(index).unwrap_or(u32::MAX)
    }
}

/// One checked `comptime match` case: its binding declarations, its
/// condition (none when it always matches), and the shapes coverage reads.
struct ComptimeCase {
    declarations: Vec<PlannedCommand>,
    condition: Option<Expr>,
    cover: Vec<CoverCommand>,
}

/// A name a `comptime match` pattern binds, the compile-time value it
/// denotes, and the binding pattern's location.
#[derive(Clone)]
struct ComptimeBinding {
    name: String,
    value: Expr,
    span: SourceSpan,
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

/// How an unread sequence element or enum payload is read: by a reference
/// with the subject's capability when its pattern binds by reference, and,
/// when owned, moved into a `var` binding when its pattern binds it once.
#[derive(Clone, Copy)]
struct ElementRead {
    by_reference: bool,
    moves: bool,
}

/// One path read a case made: the hidden name holding it, its type,
/// and whether it is an owned value rather than a reference.
#[derive(Clone)]
struct Read {
    name: String,
    ty: Ty,
    owned: bool,
}

/// What an unread sequence element's type is spelled as in a rejection.
const UNTYPED_ELEMENT: &str = "an untyped sequence element";

/// The expression reading `name`, or the compile-time name `base` in its
/// place, under `steps`, each sequence step spelled as upstream's path
/// values are: an element from the end and a slice's end read the parent's
/// `__len__` afresh, and a payload is the getter
/// `parent._unsafe_get_enum_payload[case]()`.
fn spell(synth: &Synth, name: &str, base: Option<&Expr>, steps: &[Step]) -> Expr {
    // `parent.__len__() - offset`, the parent being `steps[..depth]`, read
    // afresh.
    let length_minus = |depth: usize, offset: usize| {
        let length = synth.expr(ExprKind::MethodCall {
            object: Box::new(spell(synth, name, base, &steps[..depth])),
            method: "__len__".to_string(),
            args: Vec::new(),
            kwargs: Vec::new(),
        });
        synth.expr(ExprKind::Infix(
            InfixOp::Sub,
            Box::new(length),
            Box::new(synth.int(offset)),
        ))
    };
    let mut value = base.map_or_else(|| synth.identifier(name), |base| synth.refresh(base));
    for (at, step) in steps.iter().enumerate() {
        let object = Box::new(value);
        value = match step {
            Step::Element(index) | Step::SequenceElement(index) => synth.expr(ExprKind::Index {
                object,
                index: Box::new(synth.int(*index)),
            }),
            Step::SequenceFromEnd(offset) => synth.expr(ExprKind::Index {
                object,
                index: Box::new(length_minus(at, *offset)),
            }),
            Step::SequenceSlice { start, end_offset } => synth.expr(ExprKind::Slice {
                object,
                lower: Some(Box::new(synth.int(*start))),
                upper: (*end_offset != 0).then(|| Box::new(length_minus(at, *end_offset))),
                step: None,
            }),
            Step::Field(field) => synth.expr(ExprKind::Member {
                object,
                field: field.clone(),
            }),
            Step::EnumPayload(case) => synth.expr(ExprKind::Invoke {
                callee: Box::new(synth.expr(ExprKind::Member {
                    object,
                    field: "_unsafe_get_enum_payload".to_string(),
                })),
                param_args: vec![mojito_ast::ast::ParamArg::Value(synth.int(*case))],
                args: Vec::new(),
                kwargs: Vec::new(),
            }),
        };
    }
    value
}

/// Whether `object` is the leading-dot head's contextual sentinel.
fn is_contextual(object: &Expr) -> bool {
    matches!(&object.kind, ExprKind::Identifier(name) if name == mojito_ast::ast::CONTEXTUAL_SENTINEL)
}

/// A pattern head's owner as a diagnostic spells it: its names joined by
/// dots, an application by its name.
fn head_spelling(owner: &Expr) -> String {
    match &owner.kind {
        ExprKind::Identifier(name) | ExprKind::TypeApply { name, .. } => name.clone(),
        ExprKind::Member { object, field } => format!("{}.{field}", head_spelling(object)),
        _ => "pattern head".to_string(),
    }
}

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
        | MatchPatternKind::Literal { .. }
        | MatchPatternKind::Equal { .. }
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

/// Bind `name` to the compile-time `subject` of a `comptime match`: a bare
/// or `as` binding is the path's value, noted for a `|` alternative's
/// merge; `var` and `ref` bindings are the pin's rejections.
fn comptime_binding(
    name: &str,
    binding: Binding,
    pattern_span: SourceSpan,
    subject: &MatchSubject,
    join: Join<'_>,
    commands: &mut Commands,
) -> Result<(), TypeError> {
    match binding {
        Binding::Var => Err(TypeError::MatchPattern(
            "'var' bindings are not supported in 'comptime match'".into(),
        )),
        Binding::Ref => Err(TypeError::MatchPattern(
            "'ref' bindings are not supported in 'comptime match'".into(),
        )),
        Binding::Bare | Binding::As => {
            if let Join::Declare(bound) | Join::Assign(bound) = join {
                bound.push(name.to_string());
            }
            commands.bindings.push(ComptimeBinding {
                name: name.to_string(),
                value: subject.value(),
                span: pattern_span,
            });
            Ok(())
        }
    }
}

/// `conjuncts` joined left to right by `op` (`and` or `or`); none when
/// there are none.
fn conjoin(synth: &Synth, conjuncts: Vec<Expr>, op: InfixOp) -> Option<Expr> {
    conjuncts
        .into_iter()
        .reduce(|left, right| synth.expr(ExprKind::Infix(op, Box::new(left), Box::new(right))))
}

/// `guard` with each name a case binds read as the value it denotes, as a
/// guard reads upstream's `PValue` binding.
fn substitute_bindings(guard: &Expr, bindings: &[ComptimeBinding]) -> Expr {
    struct Substitute<'b>(&'b [ComptimeBinding]);

    impl mojito_ast::visit::MutVisitor for Substitute<'_> {
        fn visit_expr_mut(&mut self, expr: &mut Expr) {
            if let ExprKind::Identifier(name) = &expr.kind
                && let Some(binding) = self.0.iter().find(|binding| binding.name == *name)
            {
                *expr = binding.value.clone();
            }
        }
    }

    let mut guard = guard.clone();
    mojito_ast::visit::walk_expr_mut(&mut Substitute(bindings), &mut guard);
    guard
}

/// A payload type that names one of the subject's own parameters (`T` of
/// `Result[T, E]`), as the subject's argument for it, which carries the
/// bounds the declaration states. Folding the case types reads the
/// parameter only by its declaration.
fn subject_binder(payload: Ty, subject: &Ty) -> Ty {
    let TyKind::Dependent(mojito_types::types::DependentType::Parameter(expr)) = payload.kind()
    else {
        return payload;
    };
    let (mojito_types::param_expr::ParamKind::DeclRef(reference), TyKind::Struct(_, arguments)) =
        (expr.kind(), subject.kind())
    else {
        return payload;
    };
    arguments
        .iter()
        .find_map(|argument| match argument {
            mojito_types::types::TyArg::Ty(argument)
                if matches!(argument.kind(), TyKind::Param { binder, .. } if binder.id == reference.id) =>
            {
                Some(argument.clone())
            }
            _ => None,
        })
        .unwrap_or(payload)
}
