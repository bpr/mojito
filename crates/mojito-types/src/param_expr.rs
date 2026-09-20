//! Typed, canonical parameter-expression attributes.
//!
//! A [`ParamExpr`] is an immutable, typed node describing a compile-time
//! parameter expression: a constant, a reference to a declared parameter, a
//! primitive operator over such nodes, or one of a few contextual queries.
//! Every node is built by a canonicalizing constructor on a [`ParamContext`],
//! so two expressions the front end can prove equal are the *same* node
//! within one compilation and structurally equal across compilations. Type
//! equality over value arguments (`Buf[n + 1]` against `Buf[1 + n]`) is
//! therefore node identity, decided before any value is supplied.
//!
//! The normal form is deliberately bounded, and no stronger than the pinned
//! Mojo's: typed integer `+`, `-`, and `*` form a sum of products with
//! collected coefficients, and a left shift by a constant is a
//! multiplication. Everything else — unary `-` on a symbol, `//`, `%`, `**`
//! — is an opaque atom of that polynomial. Different nodes mean *not
//! established equal*, never *proved unequal*.
//!
//! Concrete values stay [`CtValue`]s: the interner is for symbolic, type, and
//! constraint boundaries, and the shared concrete folder is [`fold`].
//! `docs/notes/param-expr-attributes.md` records the design and the pin
//! evidence behind each rule.

pub mod fold;

use crate::ct::CtValue;
use crate::types::{PackPredicateRef, TrivialLifecycle, Ty};
use mojito_ast::ast::InfixOp;
use mojito_common::literal::IntLiteral;
use mojito_common::token::SourceSpan;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};

/// The largest polynomial a canonicalizing constructor builds. Exceeding it is
/// a resource diagnostic; no half-normalized node is ever produced.
pub const MAX_MONOMIALS: usize = 4_096;

/// The per-compilation hash-consing context.
///
/// Cloning shares the context. A [`ParamContext::detached`] context
/// canonicalizes without interning, for pure helpers with no compilation at
/// hand; its nodes are structurally equal to interned ones and are re-homed
/// by [`ParamContext::intern`].
#[derive(Debug, Clone, Default)]
pub struct ParamContext {
    shared: Option<Arc<ContextState>>,
}

impl ParamContext {
    /// A fresh interning context, one per compilation.
    pub fn new() -> Self {
        CONTEXTS_CREATED.fetch_add(1, AtomicOrdering::Relaxed);
        Self {
            shared: Some(Arc::new(ContextState::default())),
        }
    }

    /// A context that canonicalizes but does not intern.
    pub const fn detached() -> Self {
        Self { shared: None }
    }

    /// Record a declared parameter so later references can be validated
    /// against its slot and meta-type, and return the reference to it.
    pub fn register(&self, id: ParamId, name: &str, meta: MetaTy) -> ParamExpr {
        if let Some(shared) = &self.shared {
            shared
                .binders
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(
                    id.clone(),
                    BinderInfo {
                        name: name.to_string(),
                        meta: meta.clone(),
                    },
                );
        }
        self.decl_ref(id, name, meta)
    }

    /// The registered declaration of `id`, when this context saw it.
    pub fn binder(&self, id: &ParamId) -> Option<BinderInfo> {
        self.shared.as_ref().and_then(|shared| {
            shared
                .binders
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(id)
                .cloned()
        })
    }

    /// A closed constant. A top-level residual [`CtValue::Expr`] is returned
    /// as its expression; a residual nested inside an aggregate is rejected,
    /// because a constant has no free parameter anywhere below it.
    pub fn constant(&self, value: CtValue) -> Result<ParamExpr, ParamError> {
        if let CtValue::Expr(expr) = value {
            return Ok(self.intern(&expr));
        }
        if crate::types::ct_value_is_symbolic(&value) {
            return Err(ParamError::NotConstant(value.to_string()));
        }
        let meta = MetaTy::of_value(&value);
        Ok(self.make(meta, ParamKind::Constant(value)))
    }

    /// A constant after the declaration's expected-type conversion, so the
    /// literal `3` and `Int(3)` bound to `n: Int` are one node.
    pub fn constant_as(&self, value: CtValue, ty: &Ty) -> Result<ParamExpr, ParamError> {
        let rendered = value.to_string();
        let value = value
            .materialize_as(ty)
            .ok_or_else(|| ParamError::TypeMismatch {
                operation: "parameter value".to_string(),
                expected: ty.to_string(),
                found: rendered,
            })?;
        self.constant(value)
    }

    /// A reference to a declared parameter of an enclosing declaration.
    pub fn decl_ref(&self, id: ParamId, name: &str, meta: MetaTy) -> ParamExpr {
        self.make(
            meta,
            ParamKind::DeclRef(ParamRef {
                id,
                name: Arc::from(name),
            }),
        )
    }

    /// A reference to slot `index` of the signature binder `depth` levels out.
    pub fn index_ref(&self, depth: u32, index: u32, meta: MetaTy) -> ParamExpr {
        self.make(meta, ParamKind::IndexRef { depth, index })
    }

    /// Build `left op right` from a source operator. Subtraction and the
    /// swapped or negated comparisons lower to the primitive set here, so no
    /// `Sub`, `Ne`, `Gt`, or `Ge` node exists.
    pub fn infix(
        &self,
        op: InfixOp,
        left: &ParamExpr,
        right: &ParamExpr,
    ) -> Result<ParamExpr, ParamError> {
        let pair = [left.clone(), right.clone()];
        match op {
            InfixOp::Add => self.op(ParamOp::Add, &pair),
            InfixOp::Sub => {
                let domain = Self::arithmetic_domain(ParamOp::Add, &pair)?;
                match domain {
                    Some(domain) => {
                        let negated =
                            self.op(ParamOp::Mul, &[self.integer(domain, -1), right.clone()])?;
                        self.op(ParamOp::Add, &[left.clone(), negated])
                    }
                    None => self.op(ParamOp::Sub, &pair),
                }
            }
            InfixOp::Mul => self.op(ParamOp::Mul, &pair),
            InfixOp::Div => self.op(ParamOp::Div, &pair),
            InfixOp::FloorDiv => self.op(ParamOp::FloorDiv, &pair),
            InfixOp::Mod => self.op(ParamOp::Mod, &pair),
            InfixOp::Pow => self.op(ParamOp::Pow, &pair),
            InfixOp::Shl => self.op(ParamOp::Shl, &pair),
            InfixOp::Shr => self.op(ParamOp::Shr, &pair),
            InfixOp::BitAnd => self.op(ParamOp::BitAnd, &pair),
            InfixOp::BitOr => self.op(ParamOp::BitOr, &pair),
            InfixOp::BitXor => self.op(ParamOp::BitXor, &pair),
            InfixOp::Eq => self.op(ParamOp::Eq, &pair),
            InfixOp::Ne => self.not(&self.op(ParamOp::Eq, &pair)?),
            InfixOp::Lt => self.op(ParamOp::Lt, &pair),
            InfixOp::Le => self.op(ParamOp::Le, &pair),
            InfixOp::Gt => self.op(ParamOp::Lt, &[right.clone(), left.clone()]),
            InfixOp::Ge => self.op(ParamOp::Le, &[right.clone(), left.clone()]),
            InfixOp::And => self.op(ParamOp::BoolAnd, &pair),
            InfixOp::Or => self.op(ParamOp::BoolOr, &pair),
            InfixOp::MatMul | InfixOp::In | InfixOp::NotIn | InfixOp::Is | InfixOp::IsNot => Err(
                ParamError::Unsupported("unsupported dependent parameter expression".to_string()),
            ),
        }
    }

    /// Unary `-`. A constant folds; a symbolic operand stays an opaque atom,
    /// as the pin keeps it (`assets/type_error/param_expr_opaque_negation.mojo`).
    pub fn neg(&self, operand: &ParamExpr) -> Result<ParamExpr, ParamError> {
        self.op(ParamOp::Neg, std::slice::from_ref(operand))
    }

    /// Boolean negation, which is exclusive-or with `True`.
    pub fn not(&self, operand: &ParamExpr) -> Result<ParamExpr, ParamError> {
        self.op(ParamOp::BoolXor, &[operand.clone(), self.boolean(true)])
    }

    /// The canonicalizing primitive constructor: checks arity and operand
    /// domains, folds what is constant, and returns the normal form, which may
    /// be a constant or one of the operands.
    pub fn op(&self, op: ParamOp, operands: &[ParamExpr]) -> Result<ParamExpr, ParamError> {
        if !op.accepts_arity(operands.len()) {
            return Err(ParamError::Arity {
                operation: op.name().to_string(),
                found: operands.len(),
            });
        }
        let operands = operands
            .iter()
            .map(|operand| self.intern(operand))
            .collect::<Vec<_>>();
        match op {
            ParamOp::Add | ParamOp::Mul => self.build_ring(op, operands),
            ParamOp::Shl => self.build_shift(operands),
            ParamOp::BoolAnd | ParamOp::BoolOr => self.build_connective(op, operands),
            ParamOp::BoolXor => self.build_xor(operands),
            ParamOp::Eq | ParamOp::Lt | ParamOp::Le => self.build_comparison(op, &operands),
            ParamOp::Cond => self.build_cond(operands),
            ParamOp::Neg
            | ParamOp::Sub
            | ParamOp::Div
            | ParamOp::FloorDiv
            | ParamOp::Mod
            | ParamOp::Pow
            | ParamOp::Shr
            | ParamOp::BitAnd
            | ParamOp::BitOr
            | ParamOp::BitXor => self.build_opaque(op, operands),
        }
    }

    /// Whole-value identity of two expressions of one meta-type. Distinct from
    /// numeric `==`: types, generic arguments, and float bits compare here.
    pub fn identical(&self, left: &ParamExpr, right: &ParamExpr) -> ParamExpr {
        let (left, right) = (self.intern(left), self.intern(right));
        if left == right {
            return self.boolean(true);
        }
        if let (Some(a), Some(b)) = (left.as_constant(), right.as_constant()) {
            return self.boolean(identity_eq(a, b));
        }
        let (left, right) = ordered(left, right);
        self.make(MetaTy::bool(), ParamKind::Identical(left, right))
    }

    /// `conforms_to(subject, Trait)` over a Type-meta-type subject. The
    /// checker resolves it; an unresolved query stays a proposition.
    pub fn conforms(&self, subject: &ParamExpr, trait_name: &str) -> Result<ParamExpr, ParamError> {
        Self::require_type_subject("conforms_to", subject)?;
        Ok(self.make(
            MetaTy::bool(),
            ParamKind::Conforms {
                subject: self.intern(subject),
                trait_name: trait_name.to_string(),
            },
        ))
    }

    /// `IsTrivially{Movable,Copyable,Deinitable}[subject]`.
    pub fn trivial(
        &self,
        lifecycle: TrivialLifecycle,
        subject: &ParamExpr,
    ) -> Result<ParamExpr, ParamError> {
        Self::require_type_subject("IsTrivially*", subject)?;
        Ok(self.make(
            MetaTy::bool(),
            ParamKind::Trivial {
                lifecycle,
                subject: self.intern(subject),
            },
        ))
    }

    /// A type-valued expression over an existing [`Ty`]. A closed type folds
    /// to a Type constant; a type with free parameters is a shape.
    pub fn type_shape(&self, ty: Ty) -> ParamExpr {
        if crate::types::is_symbolic(&ty) {
            self.make(MetaTy::Type, ParamKind::TypeShape(Box::new(ty)))
        } else {
            self.make(
                MetaTy::Type,
                ParamKind::Constant(CtValue::Type(Box::new(ty))),
            )
        }
    }

    /// Select one of a finite, already-checked sequence of types by an integer
    /// expression. A constant in-range index folds to the selected type.
    pub fn select(&self, elements: Vec<Ty>, index: &ParamExpr) -> Result<ParamExpr, ParamError> {
        let index = self.intern(index);
        if !index.meta().is_integer() {
            return Err(ParamError::TypeMismatch {
                operation: "type selection".to_string(),
                expected: "an Int dependent type index".to_string(),
                found: index.meta().to_string(),
            });
        }
        if let Some(position) = index.as_constant().and_then(fold::integer_value) {
            if position.is_negative() {
                return Err(ParamError::Arithmetic(format!(
                    "dependent type index {position} is negative"
                )));
            }
            let count = elements.len();
            return position
                .to_i64()
                .and_then(|position| usize::try_from(position).ok())
                .and_then(|position| elements.into_iter().nth(position))
                .map(|ty| self.type_shape(ty))
                .ok_or_else(|| {
                    ParamError::Arithmetic(format!(
                        "dependent type index {position} is out of range for {count} element(s)"
                    ))
                });
        }
        Ok(self.make(MetaTy::Type, ParamKind::Select { elements, index }))
    }

    /// One of today's bound-pack constraint leaves, carried for the checker's
    /// concrete tuple/pack logic to resolve. It adds no symbolic pack support.
    pub fn pack_query(&self, pack: &str, query: PackQuery) -> ParamExpr {
        let meta = match &query {
            PackQuery::Length => MetaTy::int(),
            PackQuery::Conforms(_) | PackQuery::Predicate { .. } | PackQuery::Contains(_) => {
                MetaTy::bool()
            }
        };
        let query = match query {
            PackQuery::Contains(element) => PackQuery::Contains(self.intern(&element)),
            other => other,
        };
        self.make(
            meta,
            ParamKind::PackQuery {
                pack: pack.to_string(),
                query,
            },
        )
    }

    /// A typed hole. Reserved: no source construct produces one, a known
    /// parameter without a binding is its reference, and a hole is a boundary
    /// error at executable facts, MIR, mangling, and the VM.
    pub fn hole(&self, kind: HoleKind, meta: MetaTy) -> ParamExpr {
        let token = NEXT_HOLE.fetch_add(1, AtomicOrdering::Relaxed);
        self.make(meta, ParamKind::Hole { kind, token })
    }

    /// The `Bool` constant.
    pub fn boolean(&self, value: bool) -> ParamExpr {
        self.make(MetaTy::bool(), ParamKind::Constant(CtValue::Bool(value)))
    }

    /// Simultaneous, capture-avoiding replacement, as type identity sees it.
    /// The polynomial part re-enters the canonicalizing constructors, so
    /// `n + 1` at `n = 3` is `4`. An opaque atom is rebuilt and *not* folded,
    /// even once its operands are constants: the pinned Mojo keeps `n // 2`
    /// at `n = 8` as `8 // 2`, which is not the type argument `4`
    /// (`assets/type_error/param_expr_unfolded_atom.mojo`). [`Self::evaluate`]
    /// is the replacement a required value uses.
    pub fn replace(
        &self,
        expr: &ParamExpr,
        bindings: &ParamBindings,
    ) -> Result<ParamExpr, ParamError> {
        if let Some(shared) = &self.shared {
            shared.replacements.fetch_add(1, AtomicOrdering::Relaxed);
        }
        let mut memo = HashMap::new();
        self.replace_at(expr, bindings, 0, &mut memo)
    }

    /// Replacement for a use that needs the value: [`Self::replace`], then
    /// [`Self::fold`], classified. A default, a dependent index, and native
    /// monomorphization evaluate; a type argument and a `where` clause
    /// replace, because the pin proves neither from an unfolded atom.
    pub fn evaluate(
        &self,
        expr: &ParamExpr,
        bindings: &ParamBindings,
    ) -> Result<ParamEval, ParamError> {
        self.replace(expr, bindings)
            .and_then(|replaced| self.fold(&replaced))
            .map(ParamEval::from)
    }

    /// Rebuild `expr` through the folding constructors, so every closed
    /// operator that has a value becomes it. A partial operator whose fold
    /// fails stays a node for [`ParamExpr::require_constant`] to report.
    pub fn fold(&self, expr: &ParamExpr) -> Result<ParamExpr, ParamError> {
        Ok(match expr.kind() {
            ParamKind::Op { op, operands } => {
                let operands: Vec<ParamExpr> = operands
                    .iter()
                    .map(|operand| self.fold(operand))
                    .collect::<Result<_, _>>()?;
                self.op(*op, &operands)?
            }
            ParamKind::Identical(left, right) => {
                self.identical(&self.fold(left)?, &self.fold(right)?)
            }
            ParamKind::Select { elements, index } => {
                self.select(elements.clone(), &self.fold(index)?)?
            }
            _ => expr.clone(),
        })
    }

    /// Re-home `expr` (and its operands) into this context. Importing re-uses
    /// binder identities as they are; it never merges same-spelled binders.
    pub fn intern(&self, expr: &ParamExpr) -> ParamExpr {
        let Some(shared) = &self.shared else {
            return expr.clone();
        };
        // A payload carrying checked decorations (a callable type's inferred
        // transfer effects) is equal to its undecorated twin, so sharing one
        // node would hand one occurrence another's effects.
        if expr.carries_decorations() {
            return expr.clone();
        }
        let existing = shared.lookup_or_insert(expr);
        let counter = if existing.is_some() {
            &shared.hits
        } else {
            &shared.interned
        };
        counter.fetch_add(1, AtomicOrdering::Relaxed);
        existing.unwrap_or_else(|| expr.clone())
    }

    /// Instrumentation for `--timings`: never part of user output.
    pub fn stats(&self) -> ParamStats {
        let load = |counter: &AtomicU64| counter.load(AtomicOrdering::Relaxed);
        self.shared
            .as_ref()
            .map_or_else(ParamStats::default, |shared| ParamStats {
                interned: load(&shared.interned),
                hits: load(&shared.hits),
                constant_folds: load(&shared.constant_folds),
                replacements: load(&shared.replacements),
                contexts: load(&CONTEXTS_CREATED),
            })
    }

    fn make(&self, meta: MetaTy, kind: ParamKind) -> ParamExpr {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        meta.hash(&mut hasher);
        kind.hash(&mut hasher);
        let expr = ParamExpr(Arc::new(Node {
            fingerprint: hasher.finish(),
            meta,
            kind,
        }));
        self.intern(&expr)
    }

    fn integer(&self, domain: IntDomain, value: i64) -> ParamExpr {
        self.integer_literal(domain, &IntLiteral::from(value))
    }

    fn integer_literal(&self, domain: IntDomain, value: &IntLiteral) -> ParamExpr {
        let value = domain.constant(value);
        self.make(MetaTy::of_value(&value), ParamKind::Constant(value))
    }

    fn require_type_subject(operation: &str, subject: &ParamExpr) -> Result<(), ParamError> {
        if matches!(subject.meta(), MetaTy::Type) {
            Ok(())
        } else {
            Err(ParamError::TypeMismatch {
                operation: operation.to_string(),
                expected: "a type".to_string(),
                found: subject.meta().to_string(),
            })
        }
    }

    /// The integer domain `op`'s operands share, after converting literal
    /// constants to a machine operand's type; `None` outside the integers.
    fn arithmetic_domain(
        op: ParamOp,
        operands: &[ParamExpr],
    ) -> Result<Option<IntDomain>, ParamError> {
        let Some(domains) = operands
            .iter()
            .map(|operand| IntDomain::of(operand.meta()))
            .collect::<Option<Vec<_>>>()
        else {
            return Ok(None);
        };
        let machine = domains
            .iter()
            .copied()
            .find(|domain| *domain != IntDomain::Literal);
        let Some(machine) = machine else {
            return Ok(Some(IntDomain::Literal));
        };
        for (operand, domain) in operands.iter().zip(domains) {
            let converts = domain == IntDomain::Literal && operand.as_constant().is_some();
            if domain != machine && !converts {
                return Err(ParamError::TypeMismatch {
                    operation: op.name().to_string(),
                    expected: machine.ty().to_string(),
                    found: domain.ty().to_string(),
                });
            }
        }
        Ok(Some(machine))
    }

    /// `+` and `*` over one integer domain: a sum of products with collected
    /// coefficients. Outside the integers the operator folds constants and is
    /// otherwise opaque, with its operand order kept (no float reassociation).
    fn build_ring(&self, op: ParamOp, operands: Vec<ParamExpr>) -> Result<ParamExpr, ParamError> {
        let Some(domain) = Self::arithmetic_domain(op, &operands)? else {
            return self.build_opaque(op, operands);
        };
        let mut result = if op == ParamOp::Add {
            Polynomial::zero(domain)
        } else {
            Polynomial::constant(domain, &IntLiteral::from(1_i64))
        };
        for operand in &operands {
            let operand = Polynomial::of(domain, operand)?;
            result = if op == ParamOp::Add {
                result.add(&operand)
            } else {
                result.mul(&operand)?
            };
        }
        Ok(self.polynomial_node(&result))
    }

    /// `a << k` with a constant in-range `k` is `a * 2**k`, as the pin folds it.
    fn build_shift(&self, operands: Vec<ParamExpr>) -> Result<ParamExpr, ParamError> {
        let amount = operands[1]
            .as_constant()
            .and_then(fold::integer_value)
            .and_then(|amount| amount.to_i64())
            .filter(|amount| (0..63).contains(amount));
        match (IntDomain::of(operands[0].meta()), amount) {
            (Some(domain), Some(amount)) if operands[0].as_constant().is_none() => {
                let factor = self.integer_literal(
                    domain,
                    &IntLiteral::from(1_i64)
                        .shl(&IntLiteral::from(amount))
                        .expect("a shift below 63 is in range"),
                );
                self.op(ParamOp::Mul, &[operands[0].clone(), factor])
            }
            _ => self.build_opaque(ParamOp::Shl, operands),
        }
    }

    fn polynomial_node(&self, polynomial: &Polynomial) -> ParamExpr {
        let domain = polynomial.domain;
        let mut terms = Vec::new();
        let mut constant = None;
        for (monomial, coefficient) in &polynomial.terms {
            if monomial.is_empty() {
                constant = Some(self.integer_literal(domain, coefficient));
                continue;
            }
            let mut factors = Vec::with_capacity(monomial.len() + 1);
            if *coefficient != IntLiteral::from(1_i64) {
                factors.push(self.integer_literal(domain, coefficient));
            }
            factors.extend(monomial.iter().cloned());
            terms.push(if factors.len() == 1 {
                factors.remove(0)
            } else {
                self.make(
                    MetaTy::value(domain.ty()),
                    ParamKind::Op {
                        op: ParamOp::Mul,
                        operands: factors,
                    },
                )
            });
        }
        terms.extend(constant);
        match terms.len() {
            0 => self.integer(domain, 0),
            1 => terms.remove(0),
            _ => self.make(
                MetaTy::value(domain.ty()),
                ParamKind::Op {
                    op: ParamOp::Add,
                    operands: terms,
                },
            ),
        }
    }

    /// An operator with no algebra: fold it when every operand is constant and
    /// the fold succeeds, and otherwise keep the node. A closed operator whose
    /// fold fails (a division by zero) stays a node too, so an untaken branch
    /// retains it unevaluated and [`ParamExpr::require_constant`] reports it.
    fn build_opaque(&self, op: ParamOp, operands: Vec<ParamExpr>) -> Result<ParamExpr, ParamError> {
        let meta = op.result_meta(&operands)?;
        if let Some(constants) = operands
            .iter()
            .map(ParamExpr::as_constant)
            .collect::<Option<Vec<_>>>()
            && let Ok(value) = fold_primitive(op, &constants)
        {
            self.count_fold();
            return self.constant(value);
        }
        Ok(self.make(meta, ParamKind::Op { op, operands }))
    }

    /// `and`/`or`: flatten, fold constants, drop duplicates, and sort. The
    /// dominating constant wins only once every operand is known to be a
    /// well-typed `Bool`, which the arity and domain checks established.
    fn build_connective(
        &self,
        op: ParamOp,
        operands: Vec<ParamExpr>,
    ) -> Result<ParamExpr, ParamError> {
        let dominating = op == ParamOp::BoolOr;
        let mut flat = Vec::new();
        for operand in operands {
            require_bool(op, &operand)?;
            match operand.kind() {
                ParamKind::Op {
                    op: inner,
                    operands,
                } if *inner == op => flat.extend(operands.iter().cloned()),
                _ => flat.push(operand),
            }
        }
        if flat
            .iter()
            .any(|operand| operand.as_bool() == Some(dominating))
        {
            return Ok(self.boolean(dominating));
        }
        flat.retain(|operand| operand.as_bool().is_none());
        flat.sort();
        flat.dedup();
        Ok(match flat.len() {
            0 => self.boolean(!dominating),
            1 => flat.remove(0),
            _ => self.make(MetaTy::bool(), ParamKind::Op { op, operands: flat }),
        })
    }

    /// Exclusive-or, which also carries negation: constants fold into one
    /// parity bit and a repeated operand cancels.
    fn build_xor(&self, operands: Vec<ParamExpr>) -> Result<ParamExpr, ParamError> {
        let mut parity = false;
        let mut flat: Vec<ParamExpr> = Vec::new();
        let mut pending = operands;
        while let Some(operand) = pending.pop() {
            require_bool(ParamOp::BoolXor, &operand)?;
            if let Some(value) = operand.as_bool() {
                parity ^= value;
            } else if let ParamKind::Op {
                op: ParamOp::BoolXor,
                operands,
            } = operand.kind()
            {
                pending.extend(operands.iter().cloned());
            } else if let Some(position) = flat.iter().position(|existing| *existing == operand) {
                flat.remove(position);
            } else {
                flat.push(operand);
            }
        }
        flat.sort();
        if flat.is_empty() {
            return Ok(self.boolean(parity));
        }
        if parity {
            flat.push(self.boolean(true));
        }
        Ok(match flat.len() {
            1 => flat.remove(0),
            _ => self.make(
                MetaTy::bool(),
                ParamKind::Op {
                    op: ParamOp::BoolXor,
                    operands: flat,
                },
            ),
        })
    }

    /// Numeric comparison. Constants fold; identical operands decide `==` and
    /// `<=` (and refute `<`); an integer `==` whose difference is a nonzero
    /// constant is refuted. Anything else is a residual proposition.
    fn build_comparison(
        &self,
        op: ParamOp,
        operands: &[ParamExpr],
    ) -> Result<ParamExpr, ParamError> {
        let (left, right) = (operands[0].clone(), operands[1].clone());
        if let (Some(a), Some(b)) = (left.as_constant(), right.as_constant()) {
            self.count_fold();
            return fold_primitive(op, &[a, b]).and_then(|value| self.constant(value));
        }
        let domain = Self::arithmetic_domain(op, operands)?;
        if domain.is_none() && left.meta() != right.meta() {
            return Err(ParamError::TypeMismatch {
                operation: op.name().to_string(),
                expected: left.meta().to_string(),
                found: right.meta().to_string(),
            });
        }
        // Coerce a literal constant beside a machine operand, so `n == 4`
        // compares two `Int`s.
        let (left, right) = match domain {
            Some(domain) => (
                self.polynomial_node(&Polynomial::of(domain, &left)?),
                self.polynomial_node(&Polynomial::of(domain, &right)?),
            ),
            None => (left, right),
        };
        if left == right && left.is_total() {
            return Ok(self.boolean(op != ParamOp::Lt));
        }
        if op == ParamOp::Eq
            && let Some(domain) = domain
        {
            let difference =
                Polynomial::of(domain, &left)?.add(&Polynomial::of(domain, &right)?.negated());
            if let Some(constant) = difference.as_constant() {
                return Ok(self.boolean(constant.is_zero()));
            }
        }
        let operands = if op == ParamOp::Eq {
            let (left, right) = ordered(left, right);
            vec![left, right]
        } else {
            vec![left, right]
        };
        Ok(self.make(MetaTy::bool(), ParamKind::Op { op, operands }))
    }

    /// Conditional selection stays lazy: only a constant condition selects.
    fn build_cond(&self, operands: Vec<ParamExpr>) -> Result<ParamExpr, ParamError> {
        require_bool(ParamOp::Cond, &operands[0])?;
        if operands[1].meta() != operands[2].meta() {
            return Err(ParamError::TypeMismatch {
                operation: ParamOp::Cond.name().to_string(),
                expected: operands[1].meta().to_string(),
                found: operands[2].meta().to_string(),
            });
        }
        Ok(match operands[0].as_bool() {
            Some(true) => operands[1].clone(),
            Some(false) => operands[2].clone(),
            None => self.make(
                operands[1].meta().clone(),
                ParamKind::Op {
                    op: ParamOp::Cond,
                    operands,
                },
            ),
        })
    }

    fn count_fold(&self) {
        if let Some(shared) = &self.shared {
            shared.constant_folds.fetch_add(1, AtomicOrdering::Relaxed);
        }
    }

    pub(crate) fn replace_at(
        &self,
        expr: &ParamExpr,
        bindings: &ParamBindings,
        depth: u32,
        memo: &mut HashMap<(usize, u32), ParamExpr>,
    ) -> Result<ParamExpr, ParamError> {
        let key = (Arc::as_ptr(&expr.0) as usize, depth);
        if let Some(done) = memo.get(&key) {
            return Ok(done.clone());
        }
        let replaced = match expr.kind() {
            ParamKind::Constant(_) | ParamKind::Hole { .. } => expr.clone(),
            ParamKind::DeclRef(reference) => match bindings.lookup(reference) {
                Some(value) => self.bound_value(expr, value, depth)?,
                None => expr.clone(),
            },
            ParamKind::IndexRef {
                depth: reference,
                index,
            } => match reference
                .checked_sub(depth)
                .and_then(|outer| bindings.lookup_index(outer, *index))
            {
                Some(value) => self.bound_value(expr, value, depth)?,
                None => expr.clone(),
            },
            ParamKind::Op { op, operands } => {
                let operands: Vec<ParamExpr> = operands
                    .iter()
                    .map(|operand| self.replace_at(operand, bindings, depth, memo))
                    .collect::<Result<_, _>>()?;
                if op.is_atom() {
                    let meta = op.result_meta(&operands)?;
                    self.make(meta, ParamKind::Op { op: *op, operands })
                } else {
                    self.op(*op, &operands)?
                }
            }
            ParamKind::Identical(left, right) => self.identical(
                &self.replace_at(left, bindings, depth, memo)?,
                &self.replace_at(right, bindings, depth, memo)?,
            ),
            ParamKind::Conforms {
                subject,
                trait_name,
            } => self.conforms(
                &self.replace_at(subject, bindings, depth, memo)?,
                trait_name,
            )?,
            ParamKind::Trivial { lifecycle, subject } => self.trivial(
                *lifecycle,
                &self.replace_at(subject, bindings, depth, memo)?,
            )?,
            ParamKind::TypeShape(ty) => {
                self.type_shape(crate::types::replace_parameters(self, ty, bindings, depth)?)
            }
            ParamKind::Select { elements, index } => {
                let index = self.replace_at(index, bindings, depth, memo)?;
                let elements = elements
                    .iter()
                    .map(|ty| crate::types::replace_parameters(self, ty, bindings, depth))
                    .collect::<Result<_, _>>()?;
                self.select(elements, &index)?
            }
            ParamKind::PackQuery { pack, query } => {
                let query = match query {
                    PackQuery::Contains(element) => {
                        PackQuery::Contains(self.replace_at(element, bindings, depth, memo)?)
                    }
                    other => other.clone(),
                };
                self.pack_query(pack, query)
            }
        };
        memo.insert(key, replaced.clone());
        Ok(replaced)
    }

    /// Install a bound value for a reference: its meta-type must be the
    /// reference's after literal conversion, and its free signature indices
    /// shift past the binders the replacement descended through.
    fn bound_value(
        &self,
        reference: &ParamExpr,
        value: &ParamExpr,
        depth: u32,
    ) -> Result<ParamExpr, ParamError> {
        let value = match (reference.meta(), value.as_constant()) {
            (MetaTy::Value(ty), Some(constant)) if value.meta() != reference.meta() => {
                self.constant_as(constant.clone(), ty)?
            }
            _ => value.clone(),
        };
        if value.meta() != reference.meta() && !value.meta().is_unresolved_struct(reference.meta())
        {
            return Err(ParamError::TypeMismatch {
                operation: format!("binding of '{reference}'"),
                expected: reference.meta().to_string(),
                found: value.meta().to_string(),
            });
        }
        if depth == 0 {
            Ok(value)
        } else {
            self.shift(&value, 0, depth)
        }
    }

    /// Raise every signature index free at `cutoff` by `by`.
    fn shift(&self, expr: &ParamExpr, cutoff: u32, by: u32) -> Result<ParamExpr, ParamError> {
        Ok(match expr.kind() {
            ParamKind::IndexRef { depth, index } if *depth >= cutoff => {
                self.index_ref(depth + by, *index, expr.meta().clone())
            }
            ParamKind::Op { op, operands } => {
                let operands: Vec<ParamExpr> = operands
                    .iter()
                    .map(|operand| self.shift(operand, cutoff, by))
                    .collect::<Result<_, _>>()?;
                self.op(*op, &operands)?
            }
            ParamKind::Identical(left, right) => self.identical(
                &self.shift(left, cutoff, by)?,
                &self.shift(right, cutoff, by)?,
            ),
            _ => expr.clone(),
        })
    }
}

/// A cheap handle to an immutable, canonical expression node.
///
/// Equality is pointer identity first and canonical structure otherwise, so it
/// also holds across contexts; hashing is the cached structural fingerprint,
/// never an address. Ordering is the deterministic canonical order operands
/// sort by.
#[derive(Clone)]
pub struct ParamExpr(Arc<Node>);

impl ParamExpr {
    pub fn meta(&self) -> &MetaTy {
        &self.0.meta
    }

    pub fn kind(&self) -> &ParamKind {
        &self.0.kind
    }

    /// The constant this node is, when it is closed and folded.
    pub fn as_constant(&self) -> Option<&CtValue> {
        match self.kind() {
            ParamKind::Constant(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self.as_constant() {
            Some(CtValue::Bool(value)) => Some(*value),
            _ => None,
        }
    }

    /// The declared parameter this node references, when it is exactly one.
    pub fn as_decl_ref(&self) -> Option<&ParamRef> {
        match self.kind() {
            ParamKind::DeclRef(reference) => Some(reference),
            _ => None,
        }
    }

    /// The concrete value of a closed expression, or the structured reason it
    /// has none: a free parameter, a hole, or a partial operator that fails.
    pub fn require_constant(&self) -> Result<CtValue, ParamError> {
        match self.kind() {
            ParamKind::Constant(value) => Ok(value.clone()),
            ParamKind::Op { op, operands } => {
                let constants = operands
                    .iter()
                    .map(Self::require_constant)
                    .collect::<Result<Vec<_>, _>>()?;
                fold_primitive(*op, &constants.iter().collect::<Vec<_>>())
            }
            _ => Err(ParamError::NotConstant(self.to_string())),
        }
    }

    /// The value under a declaration's own name → value map: replacement
    /// through the shared engine, then the constant a required use needs.
    pub fn evaluate_named<S: std::hash::BuildHasher>(
        &self,
        parameters: &HashMap<String, CtValue, S>,
    ) -> Result<CtValue, ParamError> {
        let context = ParamContext::detached();
        let bindings = ParamBindings::from_named_values(&context, parameters);
        context.evaluate(self, &bindings)?.require_constant()
    }

    /// The compatibility transport form: a folded constant is its ordinary
    /// concrete [`CtValue`], and only a residual is [`CtValue::Expr`].
    pub fn into_value(self) -> CtValue {
        match self.as_constant() {
            Some(value) => value.clone(),
            None => CtValue::Expr(self),
        }
    }

    /// Whether no declared parameter, signature index, or hole occurs below.
    pub fn is_closed(&self) -> bool {
        let mut closed = true;
        self.visit(&mut |node| {
            closed &= !matches!(
                node.kind(),
                ParamKind::DeclRef(_)
                    | ParamKind::IndexRef { .. }
                    | ParamKind::Hole { .. }
                    | ParamKind::TypeShape(_)
                    | ParamKind::PackQuery { .. }
            );
        });
        closed
    }

    /// Whether evaluation cannot fail once the parameters are bound: no
    /// partial operator occurs below.
    pub fn is_total(&self) -> bool {
        let mut total = true;
        self.visit(&mut |node| {
            total &= !matches!(node.kind(), ParamKind::Op { op, .. } if op.is_partial());
        });
        total
    }

    /// The declared parameters referenced below, by reference.
    pub fn free_parameters(&self) -> Vec<ParamRef> {
        let mut found = Vec::new();
        self.visit(&mut |node| {
            if let ParamKind::DeclRef(reference) = node.kind()
                && !found.contains(reference)
            {
                found.push(reference.clone());
            }
        });
        found
    }

    /// The diagnostic names of the declared parameters referenced below,
    /// including those inside embedded types.
    pub fn referenced_parameters<S: std::hash::BuildHasher>(
        &self,
        output: &mut HashSet<String, S>,
    ) {
        self.visit(&mut |node| match node.kind() {
            ParamKind::DeclRef(reference) => {
                output.insert(reference.name.to_string());
            }
            ParamKind::PackQuery { pack, .. } => {
                output.insert(pack.clone());
            }
            _ => {}
        });
    }

    /// Pre-order visit of this node and every operand. Embedded [`Ty`]
    /// payloads are the type visitors' to walk.
    pub fn visit(&self, visitor: &mut dyn FnMut(&Self)) {
        visitor(self);
        match self.kind() {
            ParamKind::Op { operands, .. } => {
                for operand in operands {
                    operand.visit(visitor);
                }
            }
            ParamKind::Identical(left, right) => {
                left.visit(visitor);
                right.visit(visitor);
            }
            ParamKind::Conforms { subject, .. } | ParamKind::Trivial { subject, .. } => {
                subject.visit(visitor);
            }
            ParamKind::Select { index, .. } => index.visit(visitor),
            ParamKind::PackQuery {
                query: PackQuery::Contains(element),
                ..
            } => element.visit(visitor),
            ParamKind::Constant(_)
            | ParamKind::DeclRef(_)
            | ParamKind::IndexRef { .. }
            | ParamKind::TypeShape(_)
            | ParamKind::PackQuery { .. }
            | ParamKind::Hole { .. } => {}
        }
    }

    /// The embedded types of this node (not of its operands).
    pub fn embedded_types(&self) -> Vec<&Ty> {
        match self.kind() {
            ParamKind::TypeShape(ty) => vec![ty],
            ParamKind::Select { elements, .. } => elements.iter().collect(),
            ParamKind::Constant(CtValue::Type(ty) | CtValue::Reflected(ty)) => vec![ty],
            _ => Vec::new(),
        }
    }

    fn carries_decorations(&self) -> bool {
        self.embedded_types()
            .into_iter()
            .any(|ty| crate::types::mentions(ty, &carries_transfers))
    }

    fn rank(&self) -> u8 {
        match self.kind() {
            ParamKind::Constant(_) => 0,
            ParamKind::DeclRef(_) => 1,
            ParamKind::IndexRef { .. } => 2,
            ParamKind::Op { .. } => 3,
            ParamKind::Identical(..) => 4,
            ParamKind::Conforms { .. } => 5,
            ParamKind::Trivial { .. } => 6,
            ParamKind::TypeShape(_) => 7,
            ParamKind::Select { .. } => 8,
            ParamKind::PackQuery { .. } => 9,
            ParamKind::Hole { .. } => 10,
        }
    }
}

impl PartialEq for ParamExpr {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
            || (self.0.fingerprint == other.0.fingerprint
                && self.0.meta == other.0.meta
                && self.0.kind == other.0.kind)
    }
}

impl Eq for ParamExpr {}

impl Hash for ParamExpr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.0.fingerprint);
    }
}

impl PartialOrd for ParamExpr {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ParamExpr {
    /// The canonical operand order: kind, then payload. Never an address, an
    /// allocation order, a randomized hash, or a source location.
    fn cmp(&self, other: &Self) -> Ordering {
        if self == other {
            return Ordering::Equal;
        }
        self.rank()
            .cmp(&other.rank())
            .then_with(|| match (self.kind(), other.kind()) {
                (ParamKind::Constant(a), ParamKind::Constant(b)) => constant_order(a, b),
                (ParamKind::DeclRef(a), ParamKind::DeclRef(b)) => a.id.cmp(&b.id),
                (
                    ParamKind::IndexRef { depth, index },
                    ParamKind::IndexRef {
                        depth: other_depth,
                        index: other_index,
                    },
                ) => (depth, index).cmp(&(other_depth, other_index)),
                (
                    ParamKind::Op { op, operands },
                    ParamKind::Op {
                        op: other_op,
                        operands: other_operands,
                    },
                ) => op.cmp(other_op).then_with(|| operands.cmp(other_operands)),
                _ => Ordering::Equal,
            })
            .then_with(|| self.meta().to_string().cmp(&other.meta().to_string()))
            .then_with(|| self.to_string().cmp(&other.to_string()))
            .then_with(|| self.0.fingerprint.cmp(&other.0.fingerprint))
    }
}

impl fmt::Debug for ParamExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ParamExpr({self} : {})", self.meta())
    }
}

impl fmt::Display for ParamExpr {
    /// The Mojo-shaped rendering used in diagnostics: precedence-aware, in
    /// canonical operand order.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_expr(f, self, 0)
    }
}

impl From<ParamExpr> for ParamEval {
    fn from(expr: ParamExpr) -> Self {
        match expr.as_constant() {
            Some(value) => Self::Constant(value.clone()),
            None => Self::Residual(expr),
        }
    }
}

/// The payload of one node. Construction goes through [`ParamContext`]; the
/// variants are public so printers, verifiers, and the later dialect
/// re-homing can read them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ParamKind {
    /// A closed value: no free parameter occurs in it, its type, or below it.
    Constant(CtValue),
    /// A declared parameter of an enclosing declaration.
    DeclRef(ParamRef),
    /// Slot `index` of the signature binder `depth` levels out.
    IndexRef { depth: u32, index: u32 },
    /// A primitive operator in canonical form.
    Op {
        op: ParamOp,
        operands: Vec<ParamExpr>,
    },
    /// Whole-value identity, operands in canonical order.
    Identical(ParamExpr, ParamExpr),
    Conforms {
        subject: ParamExpr,
        trait_name: String,
    },
    Trivial {
        lifecycle: TrivialLifecycle,
        subject: ParamExpr,
    },
    /// A type with free typed references; a closed type is a constant.
    TypeShape(Box<Ty>),
    /// Finite type selection by an integer expression.
    Select { elements: Vec<Ty>, index: ParamExpr },
    /// A bound-pack query the checker's concrete pack logic resolves.
    PackQuery { pack: String, query: PackQuery },
    /// Reserved typed unknown/unbound state; see [`ParamContext::hole`].
    Hole { kind: HoleKind, token: u64 },
}

/// The primitive operators. An enum case does not make a source operation
/// supported: arity and operand domains are checked per opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ParamOp {
    Add,
    Mul,
    /// Unary `-` on a symbolic operand; an opaque atom, as the pin keeps it.
    Neg,
    /// Subtraction outside the integer domains, where no rewrite applies.
    Sub,
    Div,
    FloorDiv,
    Mod,
    /// The deferred power of an unresolved base or exponent. It is never
    /// expanded: the pin does not equate `n ** 2` with `n * n`.
    Pow,
    Shl,
    Shr,
    BitAnd,
    BitOr,
    BitXor,
    Eq,
    Lt,
    Le,
    BoolAnd,
    BoolOr,
    BoolXor,
    Cond,
}

impl ParamOp {
    /// The stable spelling used by canonical text.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Mul => "mul",
            Self::Neg => "neg",
            Self::Sub => "sub",
            Self::Div => "div",
            Self::FloorDiv => "floordiv",
            Self::Mod => "mod",
            Self::Pow => "pow",
            Self::Shl => "shl",
            Self::Shr => "shr",
            Self::BitAnd => "and",
            Self::BitOr => "or",
            Self::BitXor => "xor",
            Self::Eq => "eq",
            Self::Lt => "lt",
            Self::Le => "le",
            Self::BoolAnd => "bool_and",
            Self::BoolOr => "bool_or",
            Self::BoolXor => "bool_xor",
            Self::Cond => "cond",
        }
    }

    /// The inverse of [`Self::name`].
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|op| op.name() == name)
    }

    /// Whether this operator is an opaque atom of the canonical polynomial:
    /// it has no algebra, and type identity never re-folds it.
    pub const fn is_atom(self) -> bool {
        matches!(
            self,
            Self::Neg
                | Self::Sub
                | Self::Div
                | Self::FloorDiv
                | Self::Mod
                | Self::Pow
                | Self::Shl
                | Self::Shr
                | Self::BitAnd
                | Self::BitOr
                | Self::BitXor
        )
    }

    /// Whether evaluation can fail on well-typed operands.
    pub const fn is_partial(self) -> bool {
        matches!(
            self,
            Self::Div | Self::FloorDiv | Self::Mod | Self::Pow | Self::Shl | Self::Shr
        )
    }

    const ALL: [Self; 20] = [
        Self::Add,
        Self::Mul,
        Self::Neg,
        Self::Sub,
        Self::Div,
        Self::FloorDiv,
        Self::Mod,
        Self::Pow,
        Self::Shl,
        Self::Shr,
        Self::BitAnd,
        Self::BitOr,
        Self::BitXor,
        Self::Eq,
        Self::Lt,
        Self::Le,
        Self::BoolAnd,
        Self::BoolOr,
        Self::BoolXor,
        Self::Cond,
    ];

    const fn accepts_arity(self, arity: usize) -> bool {
        match self {
            Self::Neg => arity == 1,
            Self::Cond => arity == 3,
            Self::Add | Self::Mul | Self::BoolAnd | Self::BoolOr | Self::BoolXor => arity >= 2,
            _ => arity == 2,
        }
    }

    const fn infix(self) -> Option<InfixOp> {
        Some(match self {
            Self::Add => InfixOp::Add,
            Self::Mul => InfixOp::Mul,
            Self::Sub => InfixOp::Sub,
            Self::Div => InfixOp::Div,
            Self::FloorDiv => InfixOp::FloorDiv,
            Self::Mod => InfixOp::Mod,
            Self::Pow => InfixOp::Pow,
            Self::Shl => InfixOp::Shl,
            Self::Shr => InfixOp::Shr,
            Self::BitAnd => InfixOp::BitAnd,
            Self::BitOr => InfixOp::BitOr,
            Self::BitXor | Self::BoolXor => InfixOp::BitXor,
            Self::Eq => InfixOp::Eq,
            Self::Lt => InfixOp::Lt,
            Self::Le => InfixOp::Le,
            Self::BoolAnd => InfixOp::And,
            Self::BoolOr => InfixOp::Or,
            Self::Neg | Self::Cond => return None,
        })
    }

    /// The result meta-type of an opaque operator over these operands.
    fn result_meta(self, operands: &[ParamExpr]) -> Result<MetaTy, ParamError> {
        let first = operands[0].meta();
        let mismatch = |found: &MetaTy| ParamError::TypeMismatch {
            operation: self.name().to_string(),
            expected: first.to_string(),
            found: found.to_string(),
        };
        if !matches!(first, MetaTy::Value(ty) if is_scalar_domain(ty)) {
            return Err(ParamError::TypeMismatch {
                operation: self.name().to_string(),
                expected: "a scalar compile-time value".to_string(),
                found: first.to_string(),
            });
        }
        // A literal constant beside a machine operand takes the machine type.
        let machine = operands
            .iter()
            .map(ParamExpr::meta)
            .find(|meta| !meta.is_literal())
            .unwrap_or(first);
        for operand in operands {
            if operand.meta() != machine
                && !(operand.meta().is_literal() && operand.as_constant().is_some())
            {
                return Err(mismatch(operand.meta()));
            }
        }
        Ok(machine.clone())
    }
}

/// The identity of a declared parameter.
///
/// It is the declaration that owns the binder and the slot within it. The
/// spelling is diagnostic metadata on
/// [`ParamRef`], never identity, so two declarations that both spell `n` have
/// different parameters and a `$` clone shares its template's.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ParamId {
    pub owner: Arc<str>,
    pub slot: usize,
}

impl ParamId {
    pub fn new(owner: &str, slot: usize) -> Self {
        Self {
            owner: Arc::from(owner),
            slot,
        }
    }
}

/// A reference to a declared parameter, with its source spelling attached.
#[derive(Debug, Clone)]
pub struct ParamRef {
    pub id: ParamId,
    pub name: Arc<str>,
}

impl PartialEq for ParamRef {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for ParamRef {}

impl Hash for ParamRef {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// The meta-type of an expression: a runtime value domain, a type, a
/// reflection handle, or a compile-time aggregate. `Type` is not a runtime
/// [`Ty`], so it has no place in that lattice.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MetaTy {
    Value(Box<Ty>),
    Type,
    ReflectedType,
    Tuple(Vec<Self>),
    List(Vec<Self>),
    Dict(Vec<(Self, Self)>),
    Set(Vec<Self>),
    /// Reserved for the pack follow-on: an ordered list of one element
    /// meta-type. Nothing constructs it from source yet.
    ParamList(Box<Self>),
}

impl MetaTy {
    /// A runtime value domain.
    pub fn value(ty: Ty) -> Self {
        Self::Value(Box::new(ty))
    }

    pub fn int() -> Self {
        Self::value(Ty::Int)
    }

    pub fn bool() -> Self {
        Self::value(Ty::Bool)
    }

    /// The runtime value domain, when this is one.
    pub fn as_value(&self) -> Option<&Ty> {
        match self {
            Self::Value(ty) => Some(ty),
            _ => None,
        }
    }

    /// The meta-type of a concrete value. A frozen struct is typed by its
    /// nominal name; the checker resolves the instance type at the conversion
    /// boundary where it matters.
    pub fn of_value(value: &CtValue) -> Self {
        let all = |values: &[CtValue]| values.iter().map(Self::of_value).collect();
        match value {
            CtValue::Int(_) => Self::int(),
            CtValue::UInt(_) => Self::value(Ty::UInt),
            CtValue::Float(_) => Self::value(Ty::Float64),
            CtValue::IntLiteral(_) => Self::value(Ty::IntLiteral),
            CtValue::FloatLiteral(_) => Self::value(Ty::FloatLiteral),
            CtValue::Bool(_) => Self::bool(),
            CtValue::Str(_) => Self::value(Ty::StringLiteral),
            CtValue::Tuple(values) => Self::Tuple(all(values)),
            CtValue::List(values) => Self::List(all(values)),
            CtValue::Dict { entries, .. } => Self::Dict(
                entries
                    .iter()
                    .map(|(key, value)| (Self::of_value(key), Self::of_value(value)))
                    .collect(),
            ),
            CtValue::Set { elements, .. } => Self::Set(all(elements)),
            CtValue::Dtype(_) => Self::value(Ty::Dtype),
            CtValue::Simd { dtype, lanes } => Self::value(Ty::Simd {
                dtype: *dtype,
                width: lanes.len() as i64,
            }),
            CtValue::Struct { name, .. } => Self::value(Ty::Struct(name.clone(), Vec::new())),
            CtValue::Type(_) => Self::Type,
            CtValue::Reflected(_) => Self::ReflectedType,
            CtValue::Expr(expr) => expr.meta().clone(),
            // A deferred slot has no value yet, so no domain either.
            CtValue::Deferred(_) => Self::value(Ty::Infer),
        }
    }

    pub fn is_integer(&self) -> bool {
        matches!(self.as_value(), Some(Ty::Int | Ty::UInt | Ty::IntLiteral))
    }

    fn is_literal(&self) -> bool {
        matches!(self.as_value(), Some(Ty::IntLiteral | Ty::FloatLiteral))
    }

    /// A frozen struct constant is typed by bare name, while its parameter
    /// declares the resolved instance type.
    fn is_unresolved_struct(&self, declared: &Self) -> bool {
        matches!(
            (self.as_value(), declared.as_value()),
            (Some(Ty::Struct(found, _)), Some(Ty::Struct(expected, _)))
                if found == expected
        )
    }
}

impl fmt::Display for MetaTy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = |f: &mut fmt::Formatter<'_>, name: &str, metas: &[Self]| {
            write!(f, "{name}[")?;
            for (index, meta) in metas.iter().enumerate() {
                if index > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{meta}")?;
            }
            write!(f, "]")
        };
        match self {
            Self::Value(ty) => write!(f, "{ty}"),
            Self::Type => write!(f, "Type"),
            Self::ReflectedType => write!(f, "ReflectedType"),
            Self::Tuple(metas) => list(f, "ComptimeTuple", metas),
            Self::List(metas) => list(f, "ComptimeList", metas),
            Self::Set(metas) => list(f, "ComptimeSet", metas),
            Self::Dict(entries) => {
                write!(f, "ComptimeDict[")?;
                for (index, (key, value)) in entries.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{key}: {value}")?;
                }
                write!(f, "]")
            }
            Self::ParamList(element) => write!(f, "ParamList[{element}]"),
        }
    }
}

/// The bound-pack leaves of today's `GenericConstraint`, as a transport.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PackQuery {
    /// `TypeList[Ts.values]().length`.
    Length,
    /// `conforms_to(Ts.values, Trait)`.
    Conforms(String),
    /// `TypeList[Ts.values]().any[P]()` / `.all[P]()`.
    Predicate {
        predicate: PackPredicateRef,
        all: bool,
    },
    /// `TypeList[Ts.values]().contains[T]()` over a Type-meta-type element.
    Contains(ParamExpr),
}

/// The two reserved hole states: an explicitly unknown contextual value, and
/// one particular inference hole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HoleKind {
    Unknown,
    Unbound,
}

/// A registered declaration parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinderInfo {
    pub name: String,
    pub meta: MetaTy,
}

/// The replacement environment: values for declared parameters, for
/// signature slots, and the type bindings embedded types substitute.
///
/// A name entry is the parsed/source lookup adapter for callers that hold a
/// declaration's own name map; an identity entry always wins over it.
#[derive(Debug, Clone, Default)]
pub struct ParamBindings {
    by_id: HashMap<ParamId, ParamExpr>,
    by_name: HashMap<String, ParamExpr>,
    frames: Vec<Vec<Option<ParamExpr>>>,
    types: HashMap<String, Ty>,
}

impl ParamBindings {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bindings from a declaration's own name → value map. A value that is
    /// neither a constant nor a residual expression (a deferred slot, or an
    /// aggregate holding one) binds nothing, so its references stay.
    pub fn from_named_values<'a>(
        context: &ParamContext,
        values: impl IntoIterator<Item = (&'a String, &'a CtValue)>,
    ) -> Self {
        let mut bindings = Self::new();
        for (name, value) in values {
            if let Ok(value) = context.constant(value.clone()) {
                bindings.bind_name(name, value);
            }
        }
        bindings
    }

    pub fn bind(&mut self, id: ParamId, value: ParamExpr) {
        self.by_id.insert(id, value);
    }

    pub fn bind_name(&mut self, name: &str, value: ParamExpr) {
        self.by_name
            .insert(name.trim_start_matches('*').to_string(), value);
    }

    pub fn bind_type(&mut self, name: &str, ty: Ty) {
        self.types.insert(name.to_string(), ty);
    }

    /// Bind the slots of the outermost signature binder not yet bound.
    pub fn push_frame(&mut self, slots: Vec<Option<ParamExpr>>) {
        self.frames.push(slots);
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
            && self.by_name.is_empty()
            && self.frames.is_empty()
            && self.types.is_empty()
    }

    /// Drop the name and type entries a nested signature's binders shadow.
    pub fn mask<'a>(&mut self, names: impl IntoIterator<Item = &'a str>) {
        for name in names {
            let name = name.trim_start_matches('*');
            self.by_name.remove(name);
            self.types.remove(name);
        }
    }

    pub const fn types(&self) -> &HashMap<String, Ty> {
        &self.types
    }

    pub fn lookup(&self, reference: &ParamRef) -> Option<&ParamExpr> {
        self.by_id
            .get(&reference.id)
            .or_else(|| self.by_name.get(reference.name.trim_start_matches('*')))
    }

    fn lookup_index(&self, depth: u32, index: u32) -> Option<&ParamExpr> {
        let frame = self.frames.len().checked_sub(1 + depth as usize)?;
        self.frames[frame].get(index as usize)?.as_ref()
    }
}

/// The result of a replacement: a concrete value, or what remains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamEval {
    Constant(CtValue),
    Residual(ParamExpr),
}

impl ParamEval {
    /// The constant a required use needs, or why there is none.
    pub fn require_constant(self) -> Result<CtValue, ParamError> {
        match self {
            Self::Constant(value) => Ok(value),
            Self::Residual(expr) => expr.require_constant(),
        }
    }
}

/// A structured expression error. It is never `false`, `None`, or an
/// arbitrary constant; the owning phase maps it to its source diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamError {
    TypeMismatch {
        operation: String,
        expected: String,
        found: String,
    },
    Arity {
        operation: String,
        found: usize,
    },
    /// An evaluation that has no value: division by zero, an overflowing
    /// power, a negative exponent.
    Arithmetic(String),
    /// An operator the compile-time surface does not define.
    Unsupported(String),
    /// A constant was required and a parameter, hole, or query remains.
    NotConstant(String),
    /// The canonical polynomial would exceed [`MAX_MONOMIALS`].
    Budget {
        limit: usize,
    },
}

impl fmt::Display for ParamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TypeMismatch {
                operation,
                expected,
                found,
            } => write!(
                f,
                "parameter expression '{operation}' expects {expected}, found {found}"
            ),
            Self::Arity { operation, found } => write!(
                f,
                "parameter expression '{operation}' does not take {found} operands"
            ),
            Self::Arithmetic(message) | Self::Unsupported(message) => write!(f, "{message}"),
            Self::NotConstant(what) => {
                write!(f, "'{what}' is not a compile-time constant here")
            }
            Self::Budget { limit } => write!(
                f,
                "parameter expression exceeds the canonicalization budget of {limit} terms"
            ),
        }
    }
}

impl std::error::Error for ParamError {}

/// The three-valued result of a proposition. Lack of proof is not falsehood,
/// and negating it is not a proof either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstraintVerdict {
    Proven,
    Disproven,
    Residual(ParamExpr),
}

impl ConstraintVerdict {
    pub const fn is_proven(&self) -> bool {
        matches!(self, Self::Proven)
    }

    pub const fn is_disproven(&self) -> bool {
        matches!(self, Self::Disproven)
    }
}

impl From<ParamExpr> for ConstraintVerdict {
    fn from(proposition: ParamExpr) -> Self {
        match proposition.as_bool() {
            Some(true) => Self::Proven,
            Some(false) => Self::Disproven,
            None => Self::Residual(proposition),
        }
    }
}

/// A sourced proposition.
///
/// It is the `Bool` expression plus the clause's location and optional
/// message. The location and message are diagnostic occurrence
/// data and take no part in expression identity, so two clauses may share a
/// proposition node and still report separately.
#[derive(Debug, Clone)]
pub struct ParamConstraint {
    pub proposition: ParamExpr,
    pub message: Option<String>,
    pub location: Option<SourceSpan>,
}

impl ParamConstraint {
    /// The verdict under `bindings`, by replacement alone. Contextual queries
    /// that remain are the checker's to resolve.
    pub fn verdict(
        &self,
        context: &ParamContext,
        bindings: &ParamBindings,
    ) -> Result<ConstraintVerdict, ParamError> {
        context
            .replace(&self.proposition, bindings)
            .map(ConstraintVerdict::from)
    }
}

/// Counters behind `--timings`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParamStats {
    pub interned: u64,
    pub hits: u64,
    pub constant_folds: u64,
    pub replacements: u64,
    pub contexts: u64,
}

/// Whether two concrete values are the same parameter identity.
///
/// This is the comparison type arguments use. A dictionary's or set's display
/// spelling is
/// materialization metadata, and a callable type's transfer effects are
/// checked decorations; neither takes part.
pub fn identity_eq(left: &CtValue, right: &CtValue) -> bool {
    let all = |a: &[CtValue], b: &[CtValue]| {
        a.len() == b.len() && a.iter().zip(b).all(|(a, b)| identity_eq(a, b))
    };
    match (left, right) {
        (CtValue::Tuple(a), CtValue::Tuple(b)) | (CtValue::List(a), CtValue::List(b)) => all(a, b),
        (CtValue::Set { elements: a, .. }, CtValue::Set { elements: b, .. }) => all(a, b),
        (CtValue::Dict { entries: a, .. }, CtValue::Dict { entries: b, .. }) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|((ak, av), (bk, bv))| identity_eq(ak, bk) && identity_eq(av, bv))
        }
        (
            CtValue::Struct { name, fields },
            CtValue::Struct {
                name: other,
                fields: other_fields,
            },
        ) => {
            name == other
                && fields.len() == other_fields.len()
                && fields
                    .iter()
                    .zip(other_fields)
                    .all(|((an, av), (bn, bv))| an == bn && identity_eq(av, bv))
        }
        _ => left == right,
    }
}

/// The hash matching [`identity_eq`]: what that comparison ignores, this
/// leaves out.
pub fn identity_hash<H: Hasher>(value: &CtValue, state: &mut H) {
    std::mem::discriminant(value).hash(state);
    match value {
        CtValue::Tuple(values)
        | CtValue::List(values)
        | CtValue::Set {
            elements: values, ..
        } => {
            for value in values {
                identity_hash(value, state);
            }
        }
        CtValue::Dict { entries, .. } => {
            for (key, value) in entries {
                identity_hash(key, state);
                identity_hash(value, state);
            }
        }
        CtValue::Struct { name, fields } => {
            name.hash(state);
            for (field, value) in fields {
                field.hash(state);
                identity_hash(value, state);
            }
        }
        scalar => scalar.hash(state),
    }
}

#[derive(Debug, Default)]
struct ContextState {
    nodes: Mutex<HashMap<u64, Vec<ParamExpr>>>,
    binders: Mutex<HashMap<ParamId, BinderInfo>>,
    interned: AtomicU64,
    hits: AtomicU64,
    constant_folds: AtomicU64,
    replacements: AtomicU64,
}

impl ContextState {
    /// The interned twin of `expr`, or `None` after recording `expr` as the
    /// node of its class. The table is locked for exactly this lookup.
    fn lookup_or_insert(&self, expr: &ParamExpr) -> Option<ParamExpr> {
        let mut table = self
            .nodes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bucket = table.entry(expr.0.fingerprint).or_default();
        let found = bucket.iter().find(|candidate| *candidate == expr).cloned();
        if found.is_none() {
            bucket.push(expr.clone());
        }
        drop(table);
        found
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Node {
    fingerprint: u64,
    meta: MetaTy,
    kind: ParamKind,
}

/// The integer domains the canonical polynomial ranges over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntDomain {
    Int,
    UInt,
    Literal,
}

impl IntDomain {
    fn of(meta: &MetaTy) -> Option<Self> {
        match meta.as_value() {
            Some(Ty::Int) => Some(Self::Int),
            Some(Ty::UInt) => Some(Self::UInt),
            Some(Ty::IntLiteral) => Some(Self::Literal),
            _ => None,
        }
    }

    const fn ty(self) -> Ty {
        match self {
            Self::Int => Ty::Int,
            Self::UInt => Ty::UInt,
            Self::Literal => Ty::IntLiteral,
        }
    }

    /// Reduce a coefficient into the domain: machine domains wrap at 64 bits,
    /// the literal domain is exact.
    fn reduce(self, value: &IntLiteral) -> IntLiteral {
        match self {
            Self::Int => value
                .wrapping_signed(64)
                .map_or_else(|| value.clone(), IntLiteral::from),
            Self::UInt => value
                .wrapping_unsigned(64)
                .map_or_else(|| value.clone(), IntLiteral::from),
            Self::Literal => value.clone(),
        }
    }

    fn constant(self, value: &IntLiteral) -> CtValue {
        match self {
            Self::Int => CtValue::Int(value.wrapping_signed(64).unwrap_or_default()),
            Self::UInt => CtValue::UInt(value.wrapping_unsigned(64).unwrap_or_default()),
            Self::Literal => CtValue::IntLiteral(value.clone()),
        }
    }
}

/// A canonical sum of products: monomial (sorted atoms, with repetition) →
/// coefficient. The empty monomial is the constant term.
struct Polynomial {
    domain: IntDomain,
    terms: BTreeMap<Vec<ParamExpr>, IntLiteral>,
}

impl Polynomial {
    const fn zero(domain: IntDomain) -> Self {
        Self {
            domain,
            terms: BTreeMap::new(),
        }
    }

    fn constant(domain: IntDomain, value: &IntLiteral) -> Self {
        let mut polynomial = Self::zero(domain);
        polynomial.insert(Vec::new(), value);
        polynomial
    }

    /// Decompose a canonical node. Operands are canonical already, so `Add`
    /// and `Mul` decompose structurally and anything else is one atom.
    fn of(domain: IntDomain, expr: &ParamExpr) -> Result<Self, ParamError> {
        if let Some(value) = expr.as_constant().and_then(fold::integer_value) {
            return Ok(Self::constant(domain, &value));
        }
        match expr.kind() {
            ParamKind::Op {
                op: ParamOp::Add,
                operands,
            } if IntDomain::of(expr.meta()).is_some() => {
                let mut sum = Self::zero(domain);
                for operand in operands {
                    sum = sum.add(&Self::of(domain, operand)?);
                }
                Ok(sum)
            }
            ParamKind::Op {
                op: ParamOp::Mul,
                operands,
            } if IntDomain::of(expr.meta()).is_some() => {
                let mut product = Self::constant(domain, &IntLiteral::from(1_i64));
                for operand in operands {
                    product = product.mul(&Self::of(domain, operand)?)?;
                }
                Ok(product)
            }
            _ => {
                let mut atom = Self::zero(domain);
                atom.insert(vec![expr.clone()], &IntLiteral::from(1_i64));
                Ok(atom)
            }
        }
    }

    fn add(&self, other: &Self) -> Self {
        let mut sum = Self {
            domain: self.domain,
            terms: self.terms.clone(),
        };
        for (monomial, coefficient) in &other.terms {
            sum.insert(monomial.clone(), coefficient);
        }
        sum
    }

    fn mul(&self, other: &Self) -> Result<Self, ParamError> {
        let mut product = Self::zero(self.domain);
        for (left, left_coefficient) in &self.terms {
            for (right, right_coefficient) in &other.terms {
                let mut monomial = left.clone();
                monomial.extend(right.iter().cloned());
                monomial.sort();
                product.insert(monomial, &left_coefficient.mul(right_coefficient));
                if product.terms.len() > MAX_MONOMIALS {
                    return Err(ParamError::Budget {
                        limit: MAX_MONOMIALS,
                    });
                }
            }
        }
        Ok(product)
    }

    fn negated(&self) -> Self {
        let mut negated = Self::zero(self.domain);
        for (monomial, coefficient) in &self.terms {
            negated.insert(monomial.clone(), &coefficient.neg());
        }
        negated
    }

    fn as_constant(&self) -> Option<IntLiteral> {
        match self.terms.len() {
            0 => Some(IntLiteral::from(0_i64)),
            1 => self.terms.get(&Vec::new()).cloned(),
            _ => None,
        }
    }

    /// Add `coefficient` to `monomial`. A zero coefficient drops the term,
    /// unless the monomial holds a partial atom whose evaluation error a
    /// cancellation must not erase.
    fn insert(&mut self, monomial: Vec<ParamExpr>, coefficient: &IntLiteral) {
        let total = monomial.iter().all(ParamExpr::is_total);
        let updated = self.domain.reduce(
            &self
                .terms
                .get(&monomial)
                .map_or_else(|| coefficient.clone(), |existing| existing.add(coefficient)),
        );
        if updated.is_zero() && total {
            self.terms.remove(&monomial);
        } else {
            self.terms.insert(monomial, updated);
        }
    }
}

static CONTEXTS_CREATED: AtomicU64 = AtomicU64::new(0);
static NEXT_HOLE: AtomicU64 = AtomicU64::new(0);

/// Fold one primitive over concrete operands through the shared folder.
fn fold_primitive(op: ParamOp, operands: &[&CtValue]) -> Result<CtValue, ParamError> {
    match (op, operands) {
        (ParamOp::Neg, [operand]) => fold::fold_neg(operand),
        (ParamOp::Cond, [condition, then_value, else_value]) => match condition {
            CtValue::Bool(true) => Ok((*then_value).clone()),
            CtValue::Bool(false) => Ok((*else_value).clone()),
            _ => Err(ParamError::Unsupported(
                "a conditional parameter expression needs a Bool condition".to_string(),
            )),
        },
        (_, [first, rest @ ..]) if !rest.is_empty() => {
            let infix = op.infix().ok_or_else(|| ParamError::Arity {
                operation: op.name().to_string(),
                found: operands.len(),
            })?;
            rest.iter().try_fold((*first).clone(), |left, right| {
                fold::fold_infix(infix, &left, right)
            })
        }
        _ => Err(ParamError::Arity {
            operation: op.name().to_string(),
            found: operands.len(),
        }),
    }
}

fn require_bool(op: ParamOp, operand: &ParamExpr) -> Result<(), ParamError> {
    if *operand.meta() == MetaTy::bool() {
        Ok(())
    } else {
        Err(ParamError::TypeMismatch {
            operation: op.name().to_string(),
            expected: "Bool".to_string(),
            found: operand.meta().to_string(),
        })
    }
}

const fn is_scalar_domain(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Int
            | Ty::UInt
            | Ty::IntLiteral
            | Ty::Float64
            | Ty::FloatLiteral
            | Ty::Bool
            | Ty::StringLiteral
            | Ty::Dtype
            | Ty::Simd { .. }
    )
}

const fn carries_transfers(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Func { transfers, .. } | Ty::GenericFunc { transfers, .. } if !transfers.0.is_empty()
    )
}

fn ordered(left: ParamExpr, right: ParamExpr) -> (ParamExpr, ParamExpr) {
    if right < left {
        (right, left)
    } else {
        (left, right)
    }
}

/// Exact numeric constants order mathematically; floats by a specified bit
/// order; everything else by variant and rendering.
fn constant_order(left: &CtValue, right: &CtValue) -> Ordering {
    match (fold::integer_value(left), fold::integer_value(right)) {
        (Some(a), Some(b)) => a.cmp(&b),
        _ => match (left, right) {
            (CtValue::Float(a), CtValue::Float(b)) => a.cmp(b),
            (CtValue::FloatLiteral(a), CtValue::FloatLiteral(b)) => a.numeric_cmp(b),
            (CtValue::Bool(a), CtValue::Bool(b)) => a.cmp(b),
            (CtValue::Str(a), CtValue::Str(b)) => a.cmp(b),
            _ => Ordering::Equal,
        },
    }
}

const fn precedence(op: ParamOp) -> u8 {
    match op {
        ParamOp::Cond => 1,
        ParamOp::BoolOr => 2,
        ParamOp::BoolAnd => 3,
        ParamOp::BoolXor => 4,
        ParamOp::Eq | ParamOp::Lt | ParamOp::Le => 5,
        ParamOp::BitOr => 6,
        ParamOp::BitXor => 7,
        ParamOp::BitAnd => 8,
        ParamOp::Shl | ParamOp::Shr => 9,
        ParamOp::Add | ParamOp::Sub => 10,
        ParamOp::Mul | ParamOp::Div | ParamOp::FloorDiv | ParamOp::Mod => 11,
        ParamOp::Neg => 12,
        ParamOp::Pow => 13,
    }
}

const fn source_operator(op: ParamOp) -> &'static str {
    match op {
        ParamOp::Add => "+",
        ParamOp::Sub => "-",
        ParamOp::Mul => "*",
        ParamOp::Div => "/",
        ParamOp::FloorDiv => "//",
        ParamOp::Mod => "%",
        ParamOp::Pow => "**",
        ParamOp::Shl => "<<",
        ParamOp::Shr => ">>",
        ParamOp::BitAnd => "&",
        ParamOp::BitOr => "|",
        ParamOp::BitXor | ParamOp::BoolXor => "^",
        ParamOp::Eq => "==",
        ParamOp::Lt => "<",
        ParamOp::Le => "<=",
        ParamOp::BoolAnd => "and",
        ParamOp::BoolOr => "or",
        ParamOp::Neg => "-",
        ParamOp::Cond => "if",
    }
}

fn write_expr(f: &mut fmt::Formatter<'_>, expr: &ParamExpr, parent: u8) -> fmt::Result {
    match expr.kind() {
        ParamKind::Constant(CtValue::Bool(value)) => {
            write!(f, "{}", if *value { "True" } else { "False" })
        }
        ParamKind::Constant(value) => write!(f, "{value}"),
        ParamKind::DeclRef(reference) => write!(f, "{}", reference.name),
        ParamKind::IndexRef { depth, index } => write!(f, "${depth}.{index}"),
        ParamKind::Op { op, operands } => {
            let level = precedence(*op);
            if level < parent {
                write!(f, "(")?;
            }
            match (op, operands.as_slice()) {
                (ParamOp::Neg, [operand]) => {
                    write!(f, "-")?;
                    write_expr(f, operand, level)?;
                }
                // `x ^ True` is the canonical negation.
                (ParamOp::BoolXor, [operand, constant]) if constant.as_bool() == Some(true) => {
                    write!(f, "not ")?;
                    write_expr(f, operand, level + 1)?;
                }
                (ParamOp::Cond, [condition, then_value, else_value]) => {
                    write_expr(f, then_value, level + 1)?;
                    write!(f, " if ")?;
                    write_expr(f, condition, level + 1)?;
                    write!(f, " else ")?;
                    write_expr(f, else_value, level)?;
                }
                _ => {
                    for (index, operand) in operands.iter().enumerate() {
                        if index > 0 {
                            write!(f, " {} ", source_operator(*op))?;
                        }
                        write_expr(f, operand, level + u8::from(index > 0))?;
                    }
                }
            }
            if level < parent {
                write!(f, ")")?;
            }
            Ok(())
        }
        ParamKind::Identical(left, right) => write!(f, "{left} == {right}"),
        ParamKind::Conforms {
            subject,
            trait_name,
        } => write!(f, "conforms_to({subject}, {trait_name})"),
        ParamKind::Trivial { lifecycle, subject } => write!(
            f,
            "{}[{subject}]",
            crate::types::trivial_predicate_spelling(*lifecycle)
        ),
        ParamKind::TypeShape(ty) => write!(f, "{ty}"),
        ParamKind::Select { elements, index } => {
            write!(f, "type_sequence[")?;
            for (position, ty) in elements.iter().enumerate() {
                if position > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{ty}")?;
            }
            write!(f, "][{index}]")
        }
        ParamKind::PackQuery { pack, query } => match query {
            PackQuery::Length => write!(f, "TypeList[{pack}.values]().length"),
            PackQuery::Conforms(trait_name) => {
                write!(f, "conforms_to({pack}.values, {trait_name})")
            }
            PackQuery::Predicate { predicate, all } => {
                let reduction = if *all { "all" } else { "any" };
                let predicate = match predicate {
                    PackPredicateRef::Trivial(kind) => {
                        crate::types::trivial_predicate_spelling(*kind)
                    }
                    PackPredicateRef::Alias(name) => name,
                };
                write!(f, "TypeList[{pack}.values]().{reduction}[{predicate}]()")
            }
            PackQuery::Contains(element) => {
                write!(f, "TypeList[{pack}.values]().contains[{element}]()")
            }
        },
        ParamKind::Hole { kind, token } => match kind {
            HoleKind::Unknown => write!(f, "?unknown{token}"),
            HoleKind::Unbound => write!(f, "?unbound{token}"),
        },
    }
}

#[cfg(test)]
mod tests;
