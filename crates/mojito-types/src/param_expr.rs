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
use crate::types::TyKind;
use crate::types::{PackPredicateRef, TrivialLifecycle, Ty};
use mojito_ast::ast::{Dtype, InfixOp, PrefixOp};
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

    /// A constant of the type `ty`: a closed value of a `!kgen.simd`
    /// storage type is an attribute of that storage, which holds its
    /// vector's constants, as a `#kgen.dtype.constant` is an attribute of
    /// `!kgen.dtype` and an `ui8` one of its builtin type; any other closed
    /// value is [`Self::constant`].
    pub fn typed_constant(&self, value: CtValue, ty: &Ty) -> Result<ParamExpr, ParamError> {
        if matches!(
            ty.kind(),
            TyKind::KgenSimd { .. } | TyKind::KgenDtype | TyKind::Ui8
        ) && !crate::types::ct_value_is_symbolic(&value)
        {
            return Ok(self.make(MetaTy::value(ty.clone()), ParamKind::Constant(value)));
        }
        self.constant(value)
    }

    /// A constant after the declaration's expected-type conversion, so the
    /// literal `3` and `Int(3)` bound to `n: Int` are one node.
    pub fn constant_as(&self, value: CtValue, ty: &Ty) -> Result<ParamExpr, ParamError> {
        let rendered = value.to_string();
        // A pack of callables' list holds each bound function's symbol,
        // which a read of an element materializes as that function.
        if let TyKind::ParamList(element) = ty.kind()
            && is_function(element)
            && let CtValue::Tuple(values) = &value
            && values.iter().all(|value| matches!(value, CtValue::Str(_)))
        {
            return Ok(self.make(MetaTy::value(ty.clone()), ParamKind::Constant(value)));
        }
        // A `!kgen.simd` lane attribute (`` `#kgen.simd<1> : !kgen.scalar<ui8>` ``)
        // stays the storage type's constant its alias declares.
        if let (TyKind::KgenSimd { .. }, CtValue::Simd { dtype, lanes }) = (ty.kind(), &value)
            && crate::types::kgen_simd_ty(*dtype, lanes.len() as i64) == *ty
        {
            return self.typed_constant(value, ty);
        }
        let value = value
            .materialize_as(ty)
            .ok_or_else(|| ParamError::TypeMismatch {
                operation: "parameter value".to_string(),
                expected: ty.to_string(),
                found: rendered,
            })?;
        // A tuple at the nominal `Tuple` is a value of that type, not a
        // compile-time tuple of its elements, and a frozen struct instance
        // (named by its monomorphized instance) is a value of the declared
        // closed struct type it materialized at. A trait bound to a
        // trait-valued parameter is a value of the metatype it upcast to.
        // A string at the nominal `String` is a `String`, and a function's
        // symbol at a function type that function. A closed value of an MLIR
        // type is that type's attribute ([`Self::typed_constant`]).
        if matches!(ty.kind(), TyKind::KgenDtype | TyKind::Ui8) {
            return self.typed_constant(value, ty);
        }
        if (crate::types::tuple_elements(ty).is_some()
            || crate::types::is_nominal_string(ty)
            || matches!(ty.kind(), TyKind::Func(_) | TyKind::GenericFunc(_))
            || matches!(value, CtValue::Struct { .. } | CtValue::Trait(_)))
            && !crate::types::ct_value_is_symbolic(&value)
        {
            return Ok(self.make(MetaTy::value(ty.clone()), ParamKind::Constant(value)));
        }
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

    /// A reference to the type pack binder `pack` as a list operand: the
    /// binder's declared name without its variadic star, of type-list meta.
    pub fn type_pack(&self, pack: &ParamRef) -> ParamExpr {
        let name = pack.name.strip_prefix('*').unwrap_or(&pack.name);
        self.decl_ref(pack.id.clone(), name, MetaTy::type_list())
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

    /// `left and right` (`and`) or `left or right` over two `Bool` values,
    /// as upstream folds the operator to `POC::Cond` on `left`: `and`
    /// selects `right` when `left` holds and `left` otherwise, `or` the
    /// reverse. A `Cond` keeps its operand order, so the right operand is
    /// reached only where the left one does not decide.
    pub fn short_circuit(
        &self,
        and: bool,
        left: &ParamExpr,
        right: &ParamExpr,
    ) -> Result<ParamExpr, ParamError> {
        let arms = if and {
            [left.clone(), right.clone(), left.clone()]
        } else {
            [left.clone(), left.clone(), right.clone()]
        };
        self.op(ParamOp::Cond, &arms)
    }

    /// `operand.__mlir_index__()`: an `Int` or its `!kgen.scalar<index>`
    /// storage (or an integer literal, which materializes to an `Int`) as
    /// an `index`; see [`ParamOp::CastToBuiltin`].
    pub fn cast_to_builtin(&self, operand: &ParamExpr) -> Result<ParamExpr, ParamError> {
        self.op(ParamOp::CastToBuiltin, std::slice::from_ref(operand))
    }

    /// `pop.cast_from_builtin[_type=!kgen.scalar<index>](operand)`: an
    /// `index` as the `Int` storage it is; see [`ParamOp::CastFromBuiltin`].
    pub fn cast_from_builtin(&self, operand: &ParamExpr) -> Result<ParamExpr, ParamError> {
        self.op(ParamOp::CastFromBuiltin, std::slice::from_ref(operand))
    }

    /// `Int(mlir_value=operand)` over an `index`: upstream's builtin
    /// `SIMD.__init__(out self: Int, *, mlir_value: index)` folded, the
    /// `Int` created from the storage [`Self::cast_from_builtin`] gives.
    pub fn int_of_index(&self, operand: &ParamExpr) -> Result<ParamExpr, ParamError> {
        self.scalar_from_storage(&self.cast_from_builtin(operand)?)
    }

    /// The `Int` or `UInt` created from `storage`, a value of its
    /// `!kgen.scalar` storage (`struct.create`, `Int(mlir_value=s)`): a
    /// scalar's own `_mlir_value` is that scalar, and a sum of storages the
    /// sum of the scalars created from them.
    pub fn scalar_from_storage(&self, storage: &ParamExpr) -> Result<ParamExpr, ParamError> {
        let scalar = match IntDomain::of(storage.meta()) {
            Some(IntDomain::IntStorage) => IntDomain::Int,
            Some(IntDomain::UIntStorage) => IntDomain::UInt,
            _ => {
                return Err(ParamError::TypeMismatch {
                    operation: "struct value".to_string(),
                    expected: "an `Int`'s or a `UInt`'s storage".to_string(),
                    found: storage.meta().to_string(),
                });
            }
        };
        self.build_create(scalar, storage, None)
    }

    /// ``__mlir_op.`pop.cmp`[pred=p](left, right)`` over two `!kgen.simd`
    /// storages: the lane compare [`ParamOp::SimdCmpEq`] … `SimdCmpLe`,
    /// `gt` and `ge` the `lt` and `le` compares of the swapped operands, as
    /// the pin equates `v.gt(w)` with `w.lt(v)`. The unsigned predicates
    /// only `index.cmp` names are rejected.
    pub fn simd_cmp(
        &self,
        predicate: crate::mlir::CmpPredicate,
        left: &ParamExpr,
        right: &ParamExpr,
    ) -> Result<ParamExpr, ParamError> {
        use crate::mlir::CmpPredicate;
        let (op, swapped) = match predicate {
            CmpPredicate::Eq => (ParamOp::SimdCmpEq, false),
            CmpPredicate::Ne => (ParamOp::SimdCmpNe, false),
            CmpPredicate::Lt => (ParamOp::SimdCmpLt, false),
            CmpPredicate::Le => (ParamOp::SimdCmpLe, false),
            CmpPredicate::Gt => (ParamOp::SimdCmpLt, true),
            CmpPredicate::Ge => (ParamOp::SimdCmpLe, true),
            CmpPredicate::Ult | CmpPredicate::Ule | CmpPredicate::Ugt | CmpPredicate::Uge => {
                return Err(ParamError::Unsupported(
                    "'pop.cmp' takes no unsigned 'index.cmp' predicate".to_string(),
                ));
            }
        };
        let pair = if swapped {
            [right.clone(), left.clone()]
        } else {
            [left.clone(), right.clone()]
        };
        self.op(op, &pair)
    }

    /// ``__mlir_op.`pop.simd.reduce_and`(mask)`` or `reduce_or` over a
    /// `bool` mask's storage, a `Bool` ([`ParamOp::SimdReduceAnd`]).
    pub fn simd_reduce(
        &self,
        reduction: crate::mlir::SimdReduction,
        mask: &ParamExpr,
    ) -> Result<ParamExpr, ParamError> {
        let op = match reduction {
            crate::mlir::SimdReduction::And => ParamOp::SimdReduceAnd,
            crate::mlir::SimdReduction::Or => ParamOp::SimdReduceOr,
        };
        self.op(op, std::slice::from_ref(mask))
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
        if is_pop_literal_operation(op, &operands) {
            return self.build_pop_literal(op, operands);
        }
        match op {
            ParamOp::Add | ParamOp::Mul => self.build_ring(op, operands),
            ParamOp::Shl => self.build_shift(operands),
            ParamOp::BoolAnd | ParamOp::BoolOr => self.build_connective(op, operands),
            ParamOp::BoolXor => self.build_xor(operands),
            ParamOp::Eq | ParamOp::Lt | ParamOp::Le => self.build_comparison(op, &operands),
            ParamOp::Cond => self.build_cond(operands),
            ParamOp::CastToBuiltin | ParamOp::CastFromBuiltin => self.build_cast(op, &operands[0]),
            ParamOp::DataToStr => self.build_data_to_str(operands),
            ParamOp::StringConcat => self.build_string_concat(operands),
            ParamOp::StringSize | ParamOp::StringAddress => self.build_string_read(op, operands),
            ParamOp::SimdSplat | ParamOp::Cast => self.build_lane_op(op, operands),
            ParamOp::TargetGetField | ParamOp::TargetHasFeature => {
                self.build_target_read(op, operands)
            }
            ParamOp::GetEnv => self.build_get_env(operands),
            ParamOp::DtypeToUi8 => self.build_dtype_code(operands),
            ParamOp::StoreToMem => self.build_store_to_mem(operands),
            ParamOp::IntLiteralConvert | ParamOp::FloatLiteralConvert => {
                self.build_literal_convert(op, operands)
            }
            ParamOp::IntToFloatLiteral
            | ParamOp::FloatToIntLiteral
            | ParamOp::FloatLiteralIsNan
            | ParamOp::FloatLiteralIsNegZero
            | ParamOp::FloatLiteralIsNormal => self.build_literal_attribute(op, operands),
            ParamOp::SimdCmpEq | ParamOp::SimdCmpNe | ParamOp::SimdCmpLt | ParamOp::SimdCmpLe => {
                self.build_simd_cmp(op, operands)
            }
            ParamOp::SimdReduceAnd | ParamOp::SimdReduceOr => {
                self.build_simd_reduce(op, &operands[0])
            }
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

    /// Rebuild `op` over `operands` as the canonical constructors kept it:
    /// an unfolded call ([`ParamOp::is_unfolded_call`]) stays a node even
    /// over constants, as [`Self::replace`] keeps it, and a product by a
    /// constant zero keeps a monomial holding a partial atom, as a
    /// cancellation keeps it. Every other operator is [`Self::op`], which
    /// folds a builtin `pop` op over constants as the pin simplifies it
    /// (`-n` at `n = 2` is `-2`, `n / 2` at `n = 7` is `3`). A node the
    /// canonical constructors built is a fixed point, so an importer
    /// rebuilding one from its operands keeps its identity.
    pub fn rebuild(&self, op: ParamOp, operands: &[ParamExpr]) -> Result<ParamExpr, ParamError> {
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
        if is_pop_literal_operation(op, &operands) {
            return self.build_pop_literal(op, operands);
        }
        match op {
            ParamOp::Add | ParamOp::Mul => self.build_ring_with(op, operands, ZeroTerms::Keep),
            _ if op.is_unfolded_call() => {
                let meta = op.result_meta(&operands)?;
                Ok(self.make(meta, ParamKind::Op { op, operands }))
            }
            _ => self.op(op, &operands),
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
        if crate::types::has_free_parameters(&ty) {
            self.make(MetaTy::Type, ParamKind::TypeShape(ty))
        } else {
            self.make(MetaTy::Type, ParamKind::Constant(CtValue::Type(ty)))
        }
    }

    /// Select one of a finite, already-checked sequence of types by an integer
    /// expression. A constant in-range index folds to the selected type, and
    /// any index over a sequence of one repeated type to that type.
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
        // Every element the index may select is one type, as upstream
        // folds a selection over a homogeneous list.
        if let Some((first, rest)) = elements.split_first()
            && rest.iter().all(|element| element == first)
        {
            return Ok(self.type_shape(first.clone()));
        }
        Ok(self.make(MetaTy::Type, ParamKind::Select { elements, index }))
    }

    /// `param_list.get`: element `index` of a parameter list. A constant list
    /// of types is a finite [`Self::select`], so a bound pack and a checked
    /// type sequence share one normal form; a list that is still a parameter
    /// keeps the residual node.
    pub fn list_get(&self, list: &ParamExpr, index: &ParamExpr) -> Result<ParamExpr, ParamError> {
        let list = self.intern(list);
        if let Some(CtValue::Tuple(values)) = list.as_constant()
            && let Some(elements) = values
                .iter()
                .map(|value| match value {
                    CtValue::Type(ty) => Some(ty.clone()),
                    _ => None,
                })
                .collect::<Option<Vec<Ty>>>()
        {
            return self.select(elements, index);
        }
        let index = self.intern(index);
        if let Some(values) = list
            .as_constant()
            .and_then(CtValue::comptime_iteration_elements)
            && let Some(position) = index.as_constant().and_then(fold::integer_value)
        {
            let count = values.len();
            return position
                .to_i64()
                .and_then(|position| usize::try_from(position).ok())
                .and_then(|position| values.get(position))
                .ok_or_else(|| {
                    ParamError::Arithmetic(format!(
                        "parameter list index {position} is out of range for {count} element(s)"
                    ))
                })
                .and_then(|value| match list.meta() {
                    MetaTy::ParamList(element)
                        if let Some(ty) = element.as_value().filter(|ty| is_function(ty)) =>
                    {
                        self.constant_as(value.clone(), ty)
                    }
                    _ => self.constant(value.clone()),
                });
        }
        match list.kind() {
            ParamKind::ListTabulate { count, element } => {
                if let (Some(position), Some(count)) = (
                    index.as_constant().and_then(fold::integer_value),
                    count.as_constant().and_then(fold::integer_value),
                ) && (position.is_negative() || position >= count)
                {
                    return Err(ParamError::Arithmetic(format!(
                        "parameter list index {position} is out of range for {count} element(s)"
                    )));
                }
                return self.tabulate_element(element, &index);
            }
            // A constant index is an element of the list it falls in, while
            // every list before it has a known length.
            ParamKind::ListConcat { lists } => {
                let constant =
                    |expr: &ParamExpr| expr.as_constant().and_then(fold::integer_value)?.to_i64();
                let mut offset = constant(&index);
                for part in lists {
                    let (Some(position), Some(length)) =
                        (offset, constant(&self.list_length(part)?))
                    else {
                        break;
                    };
                    if position < length {
                        return self.list_get(part, &self.constant(CtValue::Int(position))?);
                    }
                    offset = Some(position - length);
                }
            }
            _ => {}
        }
        // A constant list of one kind of value read at an index still a
        // parameter (`[10, 20, 30][n]`) keeps the residual node, which folds
        // once the index is bound. Differently valued literals of one family
        // merge at its default type, as upstream's `coerceTypesToEachOther`
        // does.
        let element = match list.meta() {
            MetaTy::ParamList(element) => Some(element.clone()),
            MetaTy::Tuple(members) | MetaTy::List(members) => {
                members.split_first().and_then(|(first, rest)| {
                    let first_ty = first.as_value()?;
                    if rest.iter().all(|member| member == first) {
                        return Some(Box::new(first.clone()));
                    }
                    let family = |ty: &Ty| (ty.is_int_literal(), ty.is_float_literal());
                    let merges = first_ty.is_numeric_literal()
                        && rest.iter().all(|member| {
                            member
                                .as_value()
                                .is_some_and(|ty| family(ty) == family(first_ty))
                        });
                    merges.then(|| Box::new(MetaTy::value(crate::types::default_literal(first_ty))))
                })
            }
            _ => None,
        };
        let Some(element) = element else {
            return Err(ParamError::TypeMismatch {
                operation: "parameter list element".to_string(),
                expected: "a parameter list".to_string(),
                found: list.meta().to_string(),
            });
        };
        if !index.meta().is_integer() {
            return Err(ParamError::TypeMismatch {
                operation: "parameter list element".to_string(),
                expected: "an Int parameter list index".to_string(),
                found: index.meta().to_string(),
            });
        }
        if let Some(position) = index.as_constant().and_then(fold::integer_value)
            && position.is_negative()
        {
            return Err(ParamError::Arithmetic(format!(
                "parameter list index {position} is negative"
            )));
        }
        Ok(self.make(*element, ParamKind::ListGet { list, index }))
    }

    /// The bound index of a [`ParamKind::ListTabulate`] element.
    pub fn tabulate_index(&self) -> ParamExpr {
        self.index_ref(0, 0, MetaTy::int())
    }

    /// `param_list.tabulate`: the list of `count` elements, element `i`
    /// being `element` with [`Self::tabulate_index`] bound to `i`. A constant
    /// count whose elements are all constants folds to the constant list.
    pub fn list_tabulate(
        &self,
        count: &ParamExpr,
        element: &ParamExpr,
    ) -> Result<ParamExpr, ParamError> {
        let count = self.intern(count);
        if !count.meta().is_integer() {
            return Err(ParamError::TypeMismatch {
                operation: "parameter list tabulation".to_string(),
                expected: "an Int element count".to_string(),
                found: count.meta().to_string(),
            });
        }
        let element = self.intern(element);
        let list = self.make(
            MetaTy::ParamList(Box::new(element.meta().clone())),
            ParamKind::ListTabulate { count, element },
        );
        let constants = self.list_elements(&list)?.and_then(|elements| {
            elements
                .iter()
                .map(|element| element.as_constant().cloned())
                .collect::<Option<Vec<_>>>()
        });
        match constants {
            Some(values) => self.constant(CtValue::Tuple(values)),
            None => Ok(list),
        }
    }

    /// `param_list.concat`: the elements of `lists` in order. Nested
    /// concatenations flatten, empty constants drop, and adjacent constants
    /// merge, so one list or none is that list or the empty constant.
    pub fn list_concat(&self, lists: &[ParamExpr]) -> Result<ParamExpr, ParamError> {
        let mut flat: Vec<ParamExpr> = Vec::new();
        let mut pending = lists
            .iter()
            .rev()
            .map(|list| self.intern(list))
            .collect::<Vec<_>>();
        while let Some(list) = pending.pop() {
            match list.kind() {
                ParamKind::ListConcat { lists } => pending.extend(lists.iter().rev().cloned()),
                ParamKind::Constant(CtValue::Tuple(values)) => {
                    if values.is_empty() {
                        continue;
                    }
                    if let Some(ParamKind::Constant(CtValue::Tuple(previous))) =
                        flat.last().map(ParamExpr::kind)
                    {
                        let merged = previous.iter().chain(values).cloned().collect();
                        flat.pop();
                        flat.push(self.constant(CtValue::Tuple(merged))?);
                    } else {
                        flat.push(list);
                    }
                }
                ParamKind::DeclRef(_) | ParamKind::ListTabulate { .. } => flat.push(list),
                _ => {
                    return Err(ParamError::TypeMismatch {
                        operation: "parameter list concatenation".to_string(),
                        expected: "a parameter list".to_string(),
                        found: list.meta().to_string(),
                    });
                }
            }
        }
        if flat.len() < 2 {
            return flat
                .pop()
                .map_or_else(|| self.constant(CtValue::Tuple(Vec::new())), Ok);
        }
        let element = flat
            .iter()
            .find_map(|list| match list.meta() {
                MetaTy::ParamList(element) => Some((**element).clone()),
                MetaTy::Tuple(members) => members.first().cloned(),
                _ => None,
            })
            .unwrap_or(MetaTy::Type);
        Ok(self.make(
            MetaTy::ParamList(Box::new(element)),
            ParamKind::ListConcat { lists: flat },
        ))
    }

    /// The list `list` reversed: upstream's `TypeList.reverse`, a tabulation
    /// of `list`'s length whose element `i` is `list[length - 1 - i]`.
    pub fn list_reverse(&self, list: &ParamExpr) -> Result<ParamExpr, ParamError> {
        let length = self.list_length(list)?;
        let minus_one = self.constant(CtValue::Int(-1))?;
        let index = self.op(
            ParamOp::Add,
            &[
                self.shift(&length, 0, 1)?,
                minus_one.clone(),
                self.op(ParamOp::Mul, &[self.tabulate_index(), minus_one])?,
            ],
        )?;
        let element = self.list_get(&self.shift(list, 0, 1)?, &index)?;
        self.list_tabulate(&length, &element)
    }

    /// The length of a parameter list, as an `Int` expression: a constant
    /// list's count, a pack's length query, a tabulation's count, the sum of
    /// a concatenation's, or the builtin application
    /// ([`LIST_LENGTH_FUNCTION`]) to a list that is itself an application.
    pub fn list_length(&self, list: &ParamExpr) -> Result<ParamExpr, ParamError> {
        let elements = list
            .as_constant()
            .and_then(CtValue::comptime_iteration_elements);
        match list.kind() {
            ParamKind::Constant(_) if let Some(values) = elements => {
                let length = i64::try_from(values.len()).map_err(|_| {
                    ParamError::Arithmetic("parameter list is too long to count".to_string())
                })?;
                self.constant(CtValue::Int(length))
            }
            ParamKind::DeclRef(_) if matches!(list.meta(), MetaTy::ParamList(_)) => {
                Ok(self.pack_query(list, PackQuery::Length))
            }
            ParamKind::ListTabulate { count, .. } => Ok(count.clone()),
            ParamKind::ListConcat { lists } => {
                let lengths: Vec<ParamExpr> = lists
                    .iter()
                    .map(|list| self.list_length(list))
                    .collect::<Result<_, _>>()?;
                self.op(ParamOp::Add, &lengths)
            }
            // A list the elaborator evaluates (a local display binding's
            // sequence, a raw-list member read through a bound) has the
            // length its value will have.
            ParamKind::Apply { .. } | ParamKind::TypeMember { .. }
                if matches!(list.meta(), MetaTy::ParamList(_)) =>
            {
                Ok(self.apply(
                    LIST_LENGTH_FUNCTION,
                    std::slice::from_ref(list),
                    MetaTy::int(),
                ))
            }
            _ => Err(ParamError::TypeMismatch {
                operation: "parameter list length".to_string(),
                expected: "a parameter list".to_string(),
                found: list.meta().to_string(),
            }),
        }
    }

    /// The elements of a parameter list whose length is known, in order: a
    /// constant list's values (a frozen collection's elements), a
    /// tabulation of a constant count, or a
    /// concatenation of such lists. `None` while a length is still open.
    pub fn list_elements(&self, list: &ParamExpr) -> Result<Option<Vec<ParamExpr>>, ParamError> {
        match list.kind() {
            ParamKind::Constant(value)
                if let Some(values) = value.comptime_iteration_elements() =>
            {
                values
                    .into_iter()
                    .map(|value| self.constant(value))
                    .collect::<Result<_, _>>()
                    .map(Some)
            }
            ParamKind::ListTabulate { count, element } => {
                let Some(count) = count.as_constant().and_then(fold::integer_value) else {
                    return Ok(None);
                };
                let count = count.to_i64().filter(|count| *count >= 0).ok_or_else(|| {
                    ParamError::Arithmetic(format!("parameter list length {count} is negative"))
                })?;
                (0..count)
                    .map(|index| {
                        self.tabulate_element(element, &self.constant(CtValue::Int(index))?)
                    })
                    .collect::<Result<_, _>>()
                    .map(Some)
            }
            ParamKind::ListConcat { lists } => {
                let mut elements = Vec::new();
                for list in lists {
                    let Some(part) = self.list_elements(list)? else {
                        return Ok(None);
                    };
                    elements.extend(part);
                }
                Ok(Some(elements))
            }
            _ => Ok(None),
        }
    }

    /// The parameter list of `elements`: the constant list when every type
    /// is closed, and otherwise a tabulation selecting among them.
    pub fn type_list(&self, elements: Vec<Ty>) -> Result<ParamExpr, ParamError> {
        let values = elements.iter().cloned().map(CtValue::Type).collect();
        if let Ok(constant) = self.constant(CtValue::Tuple(values)) {
            return Ok(constant);
        }
        let count = i64::try_from(elements.len()).map_err(|_| {
            ParamError::Arithmetic("parameter list is too long to count".to_string())
        })?;
        let element = self.select(elements, &self.tabulate_index())?;
        self.list_tabulate(&self.constant(CtValue::Int(count))?, &element)
    }

    /// A field of a struct-typed parameter value (`Self.e.rows`): upstream's
    /// struct extract on a parameter attribute. A constant base folds to its
    /// field; `meta` is the field's declared type. A literal's `value` is
    /// its type's `value` parameter (`"ab".value` is `StringLiteral["ab"]`'s
    /// `!kgen.string`), and a symbolic literal's (`s.value` over
    /// `s: StringLiteral[_]`) the field node each instance closes.
    pub fn field(
        &self,
        base: &ParamExpr,
        name: &str,
        meta: MetaTy,
    ) -> Result<ParamExpr, ParamError> {
        if name == crate::ct::LITERAL_VALUE_MEMBER
            && let Some(slot) = base
                .meta()
                .as_value()
                .and_then(|ty| ty.kind().literal_value_slot())
        {
            return self.constant(slot.clone());
        }
        if let Some(value) = base.as_constant() {
            // A constant `SIMD`'s storage, an `Int`'s or a `UInt`'s among
            // them, is that constant at the storage type (upstream's
            // `StructExtractOp::fold`).
            if name == crate::ct::MLIR_VALUE_FIELD
                && matches!(
                    value,
                    CtValue::Simd { .. } | CtValue::Int(_) | CtValue::UInt(_)
                )
                && let Some(storage) = meta.as_value()
            {
                return self.typed_constant(value.clone(), storage);
            }
            let CtValue::Struct {
                name: owner,
                fields,
            } = value
            else {
                return Err(ParamError::TypeMismatch {
                    operation: format!("field `{name}`"),
                    expected: "a struct value".to_string(),
                    found: value.to_string(),
                });
            };
            let field = fields
                .iter()
                .find(|(field, _)| field == name)
                .ok_or_else(|| ParamError::TypeMismatch {
                    operation: format!("field `{name}`"),
                    expected: format!("a field of `{owner}`"),
                    found: name.to_string(),
                })?;
            return self.constant(field.1.clone());
        }
        if name == crate::ct::MLIR_VALUE_FIELD
            && let Some(domain @ (IntDomain::Int | IntDomain::UInt)) = IntDomain::of(base.meta())
        {
            return self.build_extract(domain, base);
        }
        if let ParamKind::StructValue { fields, spelling } = base.kind() {
            return fields
                .iter()
                .find(|(field, _)| field == name)
                .map(|(_, value)| value.clone())
                .ok_or_else(|| ParamError::TypeMismatch {
                    operation: format!("field `{name}`"),
                    expected: format!("a field of `{}`", spelling.0),
                    found: name.to_string(),
                });
        }
        let base = self.intern(base);
        Ok(self.make(
            meta,
            ParamKind::Field {
                base,
                name: name.to_string(),
            },
        ))
    }

    /// A query over a pack operand: the pack binder's reference in a
    /// template, or the list a substitution bound it to. The elaborator
    /// decides a closed one; nothing here folds it.
    pub fn pack_query(&self, pack: &ParamExpr, query: PackQuery) -> ParamExpr {
        let meta = match &query {
            PackQuery::Length | PackQuery::IndexOf(_) => MetaTy::int(),
            PackQuery::Conforms(_) | PackQuery::Predicate { .. } | PackQuery::Contains(_) => {
                MetaTy::bool()
            }
        };
        let query = match query {
            PackQuery::Contains(element) => PackQuery::Contains(self.intern(&element)),
            PackQuery::IndexOf(element) => PackQuery::IndexOf(self.intern(&element)),
            other => other,
        };
        self.make(
            meta,
            ParamKind::PackQuery {
                pack: self.intern(pack),
                query,
            },
        )
    }

    /// Rebuild a pack query over `rewrite`'s image of its pack and element.
    fn rewrite_pack_query(
        &self,
        pack: &ParamExpr,
        query: &PackQuery,
        rewrite: &mut dyn FnMut(&ParamExpr) -> Result<ParamExpr, ParamError>,
    ) -> Result<ParamExpr, ParamError> {
        let query = match query {
            PackQuery::Contains(element) => PackQuery::Contains(rewrite(element)?),
            PackQuery::IndexOf(element) => PackQuery::IndexOf(rewrite(element)?),
            other => other.clone(),
        };
        Ok(self.pack_query(&rewrite(pack)?, query))
    }

    /// A reflection query on a subject type. The subject is any type
    /// expression; the checker never builds a node over a closed struct,
    /// which it reflects from its own table.
    pub fn reflect_query(&self, subject: &ParamExpr, query: ReflectQuery) -> ParamExpr {
        let subject = self.intern(subject);
        self.make(query.meta(), ParamKind::Reflect { subject, query })
    }

    /// The compile-time value member `name` of a subject type (`T.K` for a
    /// type parameter `T` whose bound requires `comptime K: Int`), applied
    /// to `args` when the member is parameterized (`T.get[0]`), typed by
    /// `meta`, the member's declared value type. It folds nothing: the
    /// checker answers a closed subject from its struct table, and the
    /// elaborator answers one per instance.
    pub fn type_member(
        &self,
        subject: &ParamExpr,
        name: &str,
        args: &[ParamExpr],
        meta: MetaTy,
    ) -> ParamExpr {
        let subject = self.intern(subject);
        let args = args.iter().map(|arg| self.intern(arg)).collect();
        self.make(
            meta,
            ParamKind::TypeMember {
                subject,
                name: name.to_string(),
                args,
            },
        )
    }

    /// `generator` with `args` bound to its parameters in declaration
    /// order, `None` leaving a slot unbound: upstream's
    /// `#kgen.bind_params`. Binding every slot of a constant generator is
    /// its body at the arguments, and binding some is the generator over
    /// the rest; binding every slot of a type's member read unapplied
    /// (`T.get`) is the member's application (`T.get[4]`); a partial
    /// binding of a partial binding binds the first's open slots. Any other
    /// generator — a parameter still unbound — stays a node, typed as the
    /// application's result or the generator over the open slots.
    pub fn bind_params(
        &self,
        generator: &ParamExpr,
        args: &[Option<ParamExpr>],
    ) -> Result<ParamExpr, ParamError> {
        let generator = self.intern(generator);
        let Some(TyKind::Generator(ty)) = generator.meta().as_value().map(Ty::kind) else {
            return Err(ParamError::TypeMismatch {
                operation: "parameter binding".to_string(),
                expected: "a generator".to_string(),
                found: generator.meta().to_string(),
            });
        };
        if args.len() != ty.params.len() {
            return Err(ParamError::TypeMismatch {
                operation: "parameter binding".to_string(),
                expected: format!("{} argument(s)", ty.params.len()),
                found: args.len().to_string(),
            });
        }
        let args: Vec<Option<ParamExpr>> = args
            .iter()
            .map(|arg| arg.as_ref().map(|arg| self.intern(arg)))
            .collect();
        let complete = args.iter().all(Option::is_some);
        let bindings = generator_bindings(&ty.params, &args);
        match generator.kind() {
            ParamKind::Constant(CtValue::Generator(value)) => {
                if complete {
                    return self.fold(&self.replace(&value.body, &bindings)?);
                }
                // A partial binding over a parameter still unbound stays a
                // node, so a later replacement reaches that argument.
                if args.iter().flatten().all(ParamExpr::is_closed) {
                    let body = self.fold(&self.replace(&value.body, &bindings)?)?;
                    let params = bound_generator_params(&ty.unbound_params(&args), &bindings);
                    return self.constant(CtValue::Generator(Box::new(
                        crate::ct::GeneratorValue { params, body },
                    )));
                }
            }
            ParamKind::TypeMember {
                subject,
                name,
                args: applied,
            } if applied.is_empty() && complete => {
                let applied: Vec<ParamExpr> = args.iter().flatten().cloned().collect();
                let result = crate::types::substitute(&ty.result, bindings.types());
                return Ok(self.type_member(subject, name, &applied, MetaTy::value(result)));
            }
            ParamKind::BindParams {
                generator: inner,
                args: inner_args,
            } => {
                // A bound slot may read an open one (`D[_]` over `D[a:
                // Int, b: Int = a]` binds `b` to `a`), so the slots this
                // binding fills are substituted into the bound ones.
                let mut open = args.into_iter();
                let filled: Vec<Option<ParamExpr>> = inner_args
                    .iter()
                    .map(|slot| slot.is_none().then(|| open.next().flatten()).flatten())
                    .collect();
                let fresh = match inner.meta().as_value().map(Ty::kind) {
                    Some(TyKind::Generator(inner_ty)) => {
                        generator_bindings(&inner_ty.params, &filled)
                    }
                    _ => ParamBindings::new(),
                };
                let merged = inner_args
                    .iter()
                    .zip(filled)
                    .map(|(slot, filled)| match slot {
                        Some(bound) => self.replace(bound, &fresh).map(Some),
                        None => Ok(filled),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                return self.bind_params(inner, &merged);
            }
            _ => {}
        }
        let meta = if complete {
            MetaTy::value(crate::types::substitute(&ty.result, bindings.types()))
        } else {
            MetaTy::value(Ty::generator(
                bound_generator_params(&ty.unbound_params(&args), &bindings),
                ty.result.clone(),
            ))
        };
        Ok(self.make(meta, ParamKind::BindParams { generator, args }))
    }

    /// A compile-time application, typed by the callee's declared result.
    /// The arguments keep their order; nothing folds, since only the
    /// elaborator evaluates an application.
    pub fn apply(&self, function: &str, args: &[ParamExpr], meta: MetaTy) -> ParamExpr {
        let args = args.iter().map(|arg| self.intern(arg)).collect();
        self.make(
            meta,
            ParamKind::Apply {
                function: function.to_string(),
                args,
            },
        )
    }

    /// The struct value `spelling`, an application of an
    /// `@always_inline("builtin")` `__init__`, folds to: `fields` in
    /// declaration order, typed as the construction is. A field read through
    /// it is the field. A construction from a value's own fields, in order,
    /// is that value (`SIMDLength(mlir_value=m._mlir_value)` is `m`), as the
    /// `struct.create` of a struct's own extracts canonicalizes to it, and a
    /// `SIMD` created from a constant storage is that constant, as
    /// upstream's `StructCreateOp::fold` folds it.
    pub fn struct_value(&self, fields: &[(String, ParamExpr)], spelling: &ParamExpr) -> ParamExpr {
        if let [(field, storage)] = fields
            && field == crate::ct::MLIR_VALUE_FIELD
            && let Some(scalar @ (IntDomain::Int | IntDomain::UInt)) =
                IntDomain::of(spelling.meta())
            && IntDomain::of(storage.meta()) == scalar.storage()
            && let Ok(created) = self.build_create(scalar, storage, Some(spelling))
        {
            return created;
        }
        match (own_fields_base(fields), fields) {
            (Some(base), _) if base.meta() == spelling.meta() => base.clone(),
            (_, [(field, storage)])
                if field == crate::ct::MLIR_VALUE_FIELD
                    && spelling
                        .meta()
                        .as_value()
                        .is_some_and(|ty| ty.kind().is_simd())
                    && let Some(Ok(vector)) = storage
                        .as_constant()
                        .map(|value| self.constant(value.clone())) =>
            {
                vector
            }
            _ => self.construction(fields, spelling),
        }
    }

    /// The struct value of `fields` spelled `spelling`, kept a construction
    /// even over a value's own fields: a builtin body's struct-typed slot,
    /// whose field reads resolve before its struct's field types do.
    pub fn construction(&self, fields: &[(String, ParamExpr)], spelling: &ParamExpr) -> ParamExpr {
        let fields = fields
            .iter()
            .map(|(name, value)| (name.clone(), self.intern(value)))
            .collect();
        self.make(
            spelling.meta().clone(),
            ParamKind::StructValue {
                fields,
                spelling: Spelling(self.intern(spelling)),
            },
        )
    }

    /// KGEN's `#kgen.param.expr<current_target>`: the application of
    /// [`CURRENT_TARGET_FUNCTION`] to nothing, a `!kgen.target` the
    /// elaborator answers with its native target, so one checked program
    /// serves any target.
    pub fn current_target(&self) -> ParamExpr {
        self.apply(
            CURRENT_TARGET_FUNCTION,
            &[],
            MetaTy::value(TyKind::KgenTarget.into()),
        )
    }

    /// KGEN's `#kgen.param.expr<get_sizeof|get_alignof, ty, target>`: the
    /// application of `query`'s function to the `!kgen.type` `ty` and the
    /// `!kgen.target` `target`, an `index` only the elaborator answers,
    /// from its layout of `ty` under `target`.
    pub fn layout_query(
        &self,
        query: LayoutQuery,
        ty: &ParamExpr,
        target: &ParamExpr,
    ) -> ParamExpr {
        self.apply(
            query.function(),
            &[ty.clone(), target.clone()],
            MetaTy::value(TyKind::Index.into()),
        )
    }

    /// The reference a compile-time call passes at a `ref` parameter:
    /// `operand` stored in compile-time memory ([`ParamOp::StoreToMem`]),
    /// the frozen reference once it is constant.
    pub fn store_to_mem(&self, operand: &ParamExpr) -> Result<ParamExpr, ParamError> {
        self.op(ParamOp::StoreToMem, std::slice::from_ref(operand))
    }

    /// The value a [`Self::store_to_mem`] reference holds, and any other
    /// expression itself.
    pub fn referent(&self, expr: &ParamExpr) -> Result<ParamExpr, ParamError> {
        match expr.kind() {
            ParamKind::Op {
                op: ParamOp::StoreToMem,
                operands,
            } => Ok(operands[0].clone()),
            ParamKind::Constant(CtValue::StoreToMem(referent)) => {
                self.constant((**referent).clone())
            }
            _ => Ok(expr.clone()),
        }
    }

    /// The `Bool` predicate `dtype.<method>()` (one of
    /// [`mojito_ast::ast::DTYPE_PREDICATES`]), as upstream's
    /// `@always_inline("builtin")` body folds into the parameter
    /// expression: a `_match` tests a mask bit of the dtype's code
    /// ([`ParamOp::DtypeToUi8`]), beside comparisons with `DType.int` and
    /// `DType.uint`. A comparison with a dtype Mojito does not represent
    /// (`bfloat16`, the eight-bit and narrower floats) is `False`. `None`
    /// for any other method.
    pub fn dtype_predicate(
        &self,
        method: &str,
        dtype: &ParamExpr,
    ) -> Option<Result<ParamExpr, ParamError>> {
        let code = |mask: u8| {
            self.constant(CtValue::Simd {
                dtype: Dtype::UInt8,
                lanes: vec![crate::ct::CtLane::Int(i128::from(mask))],
            })
        };
        let field = self.field(
            dtype,
            crate::ct::MLIR_VALUE_FIELD,
            MetaTy::value(Ty::kgen_dtype()),
        );
        let matches = |mask: u8| {
            let own = self.op(ParamOp::DtypeToUi8, &[field.clone()?])?;
            let masked = self.op(ParamOp::BitAnd, &[own, code(mask)?])?;
            self.infix(InfixOp::Ne, &masked, &code(0)?)
        };
        let is = |other: Dtype| {
            self.op(
                ParamOp::Eq,
                &[
                    dtype.clone(),
                    self.constant(crate::types::dtype_value(other))?,
                ],
            )
        };
        let or = |left, right| self.op(ParamOp::BoolOr, &[left, right]);
        let and = |left, right| self.op(ParamOp::BoolAnd, &[left, right]);
        let floating = || matches(mojito_ast::ast::DTYPE_FLOAT_MASK);
        let integral = || {
            or(
                or(is(Dtype::Int)?, is(Dtype::UInt)?)?,
                matches(mojito_ast::ast::DTYPE_INTEGER_MASK)?,
            )
        };
        let signed = || matches(mojito_ast::ast::DTYPE_SIGNED_MASK);
        Some(match method {
            "is_floating_point" => floating(),
            "is_integral" => integral(),
            "is_signed" => (|| or(floating()?, and(integral()?, signed()?)?))(),
            "is_unsigned" => (|| {
                or(
                    is(Dtype::UInt)?,
                    and(
                        matches(mojito_ast::ast::DTYPE_INTEGER_MASK)?,
                        self.not(&signed()?)?,
                    )?,
                )
            })(),
            "is_numeric" => (|| or(integral()?, floating()?))(),
            "is_float8" => Ok(self.boolean(false)),
            "is_half_float" => is(Dtype::Float16),
            _ => return None,
        })
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
    /// The polynomial part and a builtin `pop` atom re-enter the
    /// canonicalizing constructors, so `n + 1` at `n = 3` is `4` and `n / 2`
    /// at `n = 7` is `3`. An unfolded call ([`ParamOp::is_unfolded_call`])
    /// is rebuilt and *not* folded, even once its operands are constants:
    /// the pinned Mojo keeps `n // 2` at `n = 8` as `8 // 2`, which is not
    /// the type argument `4`
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
    /// [`Self::fold`] and [`Self::answer_builtin_applications`], classified. A default, a dependent index, and native
    /// monomorphization evaluate; a type argument and a `where` clause
    /// replace, because the pin proves neither from an unfolded atom.
    pub fn evaluate(
        &self,
        expr: &ParamExpr,
        bindings: &ParamBindings,
    ) -> Result<ParamEval, ParamError> {
        self.replace(expr, bindings)
            .and_then(|replaced| self.fold(&replaced))
            .and_then(|folded| self.answer_builtin_applications(&folded))
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
            ParamKind::ListGet { list, index } => {
                self.list_get(&self.fold(list)?, &self.fold(index)?)?
            }
            ParamKind::Field { base, name } => {
                self.field(&self.fold(base)?, name, expr.meta().clone())?
            }
            ParamKind::ListTabulate { count, element } => {
                self.list_tabulate(&self.fold(count)?, &self.fold(element)?)?
            }
            ParamKind::ListConcat { lists } => {
                let lists: Vec<ParamExpr> = lists
                    .iter()
                    .map(|list| self.fold(list))
                    .collect::<Result<_, _>>()?;
                self.list_concat(&lists)?
            }
            ParamKind::Reflect { subject, query } => {
                self.reflect_query(&self.fold(subject)?, query.clone())
            }
            ParamKind::TypeMember {
                subject,
                name,
                args,
            } => {
                let args: Vec<ParamExpr> = args
                    .iter()
                    .map(|arg| self.fold(arg))
                    .collect::<Result<_, _>>()?;
                self.type_member(&self.fold(subject)?, name, &args, expr.meta().clone())
            }
            ParamKind::Apply { function, args } => {
                let args: Vec<ParamExpr> = args
                    .iter()
                    .map(|arg| self.fold(arg))
                    .collect::<Result<_, _>>()?;
                self.apply(function, &args, expr.meta().clone())
            }
            ParamKind::BindParams { generator, args } => {
                let args: Vec<Option<ParamExpr>> = args
                    .iter()
                    .map(|arg| arg.as_ref().map(|arg| self.fold(arg)).transpose())
                    .collect::<Result<_, _>>()?;
                self.bind_params(&self.fold(generator)?, &args)?
            }
            ParamKind::StructValue { fields, spelling } => self.struct_value(
                &rebuilt_fields(fields, |value| self.fold(value))?,
                &self.fold(&spelling.0)?,
            ),
            ParamKind::PackQuery { pack, query } => {
                self.rewrite_pack_query(pack, query, &mut |operand| self.fold(operand))?
            }
            _ => expr.clone(),
        })
    }

    /// Rebuild `expr` with every reflection query `oracle` answers replaced
    /// by its answer, through the folding constructors, so what sits above
    /// an answered query closes too (`reflect[T].field_count() == 2` to a
    /// `Bool`, `reflect[T].field_names()[1]` to a string). A query the oracle
    /// leaves (`Ok(None)`) is rebuilt as it is.
    pub fn answer_reflections(
        &self,
        expr: &ParamExpr,
        oracle: &mut ReflectOracle<'_>,
    ) -> Result<ParamExpr, ParamError> {
        self.answer_queries(expr, &mut |node| match node.kind() {
            ParamKind::Reflect { subject, query } => self.constant_answer(oracle(subject, query)?),
            _ => Ok(None),
        })
    }

    /// Rebuild `expr` with every `conforms_to` query `oracle` answers
    /// replaced by its `Bool`, through the folding constructors, as
    /// upstream's elaborator folds `#kgen.type_conforms_to_trait` over a
    /// concrete type, so what sits above an answered query closes too
    /// (`not conforms_to(T, Copyable)`). A query the oracle leaves (`None`)
    /// is rebuilt as it is.
    pub fn answer_conformances(
        &self,
        expr: &ParamExpr,
        oracle: &mut ConformsOracle<'_>,
    ) -> Result<ParamExpr, ParamError> {
        self.answer_queries(expr, &mut |node| self.conformance_answer(node, oracle))
    }

    /// Rebuild `ty` with every `conforms_to` query `oracle` answers in the
    /// parameter expressions it embeds replaced by its `Bool`
    /// (`Box[conforms_to(T, Copyable)]`), as [`Self::answer_conformances`]
    /// rebuilds one expression.
    pub fn answer_type_conformances(
        &self,
        ty: &Ty,
        oracle: &mut ConformsOracle<'_>,
    ) -> Result<Ty, ParamError> {
        crate::types::rewrite_ty(
            ty,
            &mut Answering {
                context: self,
                answer: &mut |node| self.conformance_answer(node, oracle),
            },
        )
    }

    /// Rebuild `expr` with every type member read `oracle` answers replaced
    /// by its answer at the read's declared type (a literal `7` read as the
    /// `Int` a requirement declares), through the folding constructors, so
    /// what sits above an answered read closes too (`T.K + 3` to `10`). A
    /// read the oracle leaves (`Ok(None)`) is rebuilt as it is.
    pub fn answer_type_members(
        &self,
        expr: &ParamExpr,
        oracle: &mut TypeMemberOracle<'_>,
    ) -> Result<ParamExpr, ParamError> {
        self.answer_queries(expr, &mut |node| match node.kind() {
            ParamKind::TypeMember {
                subject,
                name,
                args,
            } => self.constant_answer(oracle(subject, name, args)?.map(|value| {
                node.meta()
                    .as_value()
                    .and_then(|ty| value.clone().materialize_as(ty))
                    .unwrap_or(value)
            })),
            _ => Ok(None),
        })
    }

    /// Rebuild `expr` with every type member read `oracle` answers replaced
    /// by the member's expression at the instance, as upstream's
    /// substitution reads the conformance's witness attribute: a member
    /// whose value is a compile-time application (`comptime V = P(2)`)
    /// becomes that application, for the elaborator to run. A read the
    /// oracle leaves (`Ok(None)`) is rebuilt as it is.
    pub fn expand_type_members(
        &self,
        expr: &ParamExpr,
        oracle: &mut TypeMemberBodyOracle<'_>,
    ) -> Result<ParamExpr, ParamError> {
        self.answer_queries(expr, &mut |node| match node.kind() {
            ParamKind::TypeMember {
                subject,
                name,
                args,
            } => oracle(subject, name, args),
            _ => Ok(None),
        })
    }

    /// Rebuild `expr` with every builtin application whose arguments are
    /// constant replaced by its answer ([`builtin_application_value`]),
    /// through the folding constructors. A required value answers one;
    /// [`Self::replace`] does not, since an application stays symbolic in a
    /// type, as at the pin.
    pub fn answer_builtin_applications(&self, expr: &ParamExpr) -> Result<ParamExpr, ParamError> {
        self.answer_queries(expr, &mut |node| match node.kind() {
            ParamKind::Apply { function, args } => {
                self.constant_answer(builtin_application_value(function, args).transpose()?)
            }
            _ => Ok(None),
        })
    }

    /// Rebuild `expr` with every unevaluated application `oracle` answers
    /// replaced by its answer, through the folding constructors, so what
    /// sits above an answered application closes too. The oracle takes the
    /// applied function and its arguments; one it leaves (`Ok(None)`) is
    /// rebuilt as it is.
    pub fn answer_applications(
        &self,
        expr: &ParamExpr,
        oracle: &mut ApplicationOracle<'_>,
    ) -> Result<ParamExpr, ParamError> {
        self.answer_queries(expr, &mut |node| match node.kind() {
            ParamKind::Apply { function, args } => self.constant_answer(oracle(function, args)?),
            _ => Ok(None),
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

    /// The context's node equal to `ty`, which becomes it when the context
    /// holds none yet, as MLIR's context uniques a type; `ty` itself when
    /// the context is detached.
    ///
    /// A type carrying a decoration its equality leaves out
    /// ([`Ty::carries_decorations`]) is not shared, since sharing its node
    /// would hand one occurrence another's decoration.
    pub fn intern_ty(&self, ty: &Ty) -> Ty {
        let Some(shared) = &self.shared else {
            return ty.clone();
        };
        if ty.carries_decorations() {
            return ty.clone();
        }
        let existing = shared.lookup_or_insert_ty(ty);
        let counter = if existing.is_some() {
            &shared.type_hits
        } else {
            &shared.types_interned
        };
        counter.fetch_add(1, AtomicOrdering::Relaxed);
        existing.unwrap_or_else(|| ty.clone())
    }

    /// Instrumentation for `--timings`: never part of user output.
    pub fn stats(&self) -> ParamStats {
        let load = |counter: &AtomicU64| counter.load(AtomicOrdering::Relaxed);
        self.shared
            .as_ref()
            .map_or_else(ParamStats::default, |shared| ParamStats {
                interned: load(&shared.interned),
                hits: load(&shared.hits),
                types_interned: load(&shared.types_interned),
                type_hits: load(&shared.type_hits),
                constant_folds: load(&shared.constant_folds),
                replacements: load(&shared.replacements),
                contexts: load(&CONTEXTS_CREATED),
            })
    }

    /// Rebuild `expr` bottom-up, replacing each reflection query or
    /// application `answer` answers (after its operands are rebuilt) by its
    /// answer; everything above re-enters the folding constructors. A
    /// display application's answer is the frozen collection itself, which
    /// list reads see through [`CtValue::comptime_iteration_elements`].
    fn answer_queries(
        &self,
        expr: &ParamExpr,
        answer: &mut dyn FnMut(&ParamExpr) -> Result<Option<ParamExpr>, ParamError>,
    ) -> Result<ParamExpr, ParamError> {
        let rebuilt = match expr.kind() {
            ParamKind::Reflect { subject, query } => {
                self.reflect_query(&self.answer_queries(subject, answer)?, query.clone())
            }
            ParamKind::TypeMember {
                subject,
                name,
                args,
            } => {
                let args: Vec<ParamExpr> = args
                    .iter()
                    .map(|arg| self.answer_queries(arg, answer))
                    .collect::<Result<_, _>>()?;
                self.type_member(
                    &self.answer_queries(subject, answer)?,
                    name,
                    &args,
                    expr.meta().clone(),
                )
            }
            ParamKind::Op { op, operands } => {
                let operands: Vec<ParamExpr> = operands
                    .iter()
                    .map(|operand| self.answer_queries(operand, answer))
                    .collect::<Result<_, _>>()?;
                return self.op(*op, &operands);
            }
            ParamKind::Identical(left, right) => {
                return Ok(self.identical(
                    &self.answer_queries(left, answer)?,
                    &self.answer_queries(right, answer)?,
                ));
            }
            ParamKind::Conforms {
                subject,
                trait_name,
            } => self.conforms(&self.answer_queries(subject, answer)?, trait_name)?,
            ParamKind::Select { elements, index } => {
                return self.select(elements.clone(), &self.answer_queries(index, answer)?);
            }
            ParamKind::ListGet { list, index } => {
                return self.list_get(
                    &self.answer_queries(list, answer)?,
                    &self.answer_queries(index, answer)?,
                );
            }
            ParamKind::Field { base, name } => {
                return self.field(
                    &self.answer_queries(base, answer)?,
                    name,
                    expr.meta().clone(),
                );
            }
            ParamKind::ListTabulate { count, element } => {
                return self.list_tabulate(
                    &self.answer_queries(count, answer)?,
                    &self.answer_queries(element, answer)?,
                );
            }
            ParamKind::ListConcat { lists } => {
                let lists: Vec<ParamExpr> = lists
                    .iter()
                    .map(|list| self.answer_queries(list, answer))
                    .collect::<Result<_, _>>()?;
                return self.list_concat(&lists);
            }
            ParamKind::Apply { function, args } => {
                let args: Vec<ParamExpr> = args
                    .iter()
                    .map(|arg| self.answer_queries(arg, answer))
                    .collect::<Result<_, _>>()?;
                self.apply(function, &args, expr.meta().clone())
            }
            ParamKind::BindParams { generator, args } => {
                let args: Vec<Option<ParamExpr>> = args
                    .iter()
                    .map(|arg| {
                        arg.as_ref()
                            .map(|arg| self.answer_queries(arg, answer))
                            .transpose()
                    })
                    .collect::<Result<_, _>>()?;
                return self.bind_params(&self.answer_queries(generator, answer)?, &args);
            }
            // An answered construction is the struct value it computes.
            ParamKind::StructValue { fields, spelling } => {
                let spelling = self.answer_queries(&spelling.0, answer)?;
                if spelling.as_constant().is_some() {
                    return Ok(spelling);
                }
                return Ok(self.struct_value(
                    &rebuilt_fields(fields, |value| self.answer_queries(value, answer))?,
                    &spelling,
                ));
            }
            // A parametric type is canonicalized with the parameter
            // expressions it embeds.
            ParamKind::TypeShape(ty) => {
                let mut answering = Answering {
                    context: self,
                    answer,
                };
                return Ok(self.type_shape(crate::types::rewrite_ty(ty, &mut answering)?));
            }
            _ => return Ok(expr.clone()),
        };
        Ok(answer(&rebuilt)?.unwrap_or(rebuilt))
    }

    /// The `Bool` `oracle` answers for the `conforms_to` query `node`, as
    /// the node [`Self::answer_queries`] puts in its place; `None` for any
    /// other node.
    fn conformance_answer(
        &self,
        node: &ParamExpr,
        oracle: &mut ConformsOracle<'_>,
    ) -> Result<Option<ParamExpr>, ParamError> {
        match node.kind() {
            ParamKind::Conforms {
                subject,
                trait_name,
            } => self.constant_answer(oracle(subject, trait_name).map(CtValue::Bool)),
            _ => Ok(None),
        }
    }

    /// An oracle's constant answer as the node [`Self::answer_queries`]
    /// puts in place of the query.
    fn constant_answer(&self, value: Option<CtValue>) -> Result<Option<ParamExpr>, ParamError> {
        value.map(|value| self.constant(value)).transpose()
    }

    fn make(&self, meta: MetaTy, kind: ParamKind) -> ParamExpr {
        self.intern(&ParamExpr(Arc::new(Node::new(meta, kind))))
    }

    fn integer(&self, domain: IntDomain, value: i64) -> ParamExpr {
        self.integer_literal(domain, &IntLiteral::from(value))
    }

    fn integer_literal(&self, domain: IntDomain, value: &IntLiteral) -> ParamExpr {
        let value = domain.constant(value);
        let meta = match domain {
            IntDomain::IntStorage | IntDomain::UIntStorage => domain.meta(),
            _ => MetaTy::of_value(&value),
        };
        self.make(meta, ParamKind::Constant(value))
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
                    expected: machine.name(),
                    found: domain.name(),
                });
            }
        }
        Ok(Some(machine))
    }

    /// `+` and `*` over one integer domain: a sum of products with collected
    /// coefficients. Outside the integers the operator folds constants and is
    /// otherwise opaque, with its operand order kept (no float reassociation).
    fn build_ring(&self, op: ParamOp, operands: Vec<ParamExpr>) -> Result<ParamExpr, ParamError> {
        self.build_ring_with(op, operands, ZeroTerms::Drop)
    }

    fn build_ring_with(
        &self,
        op: ParamOp,
        operands: Vec<ParamExpr>,
        zeros: ZeroTerms,
    ) -> Result<ParamExpr, ParamError> {
        let Some(domain) = Self::arithmetic_domain(op, &operands)? else {
            return self.build_opaque(op, operands);
        };
        let result = if op == ParamOp::Add {
            let mut sum = Polynomial::zero(domain);
            for operand in &operands {
                sum = sum.add(&Polynomial::of_with(domain, operand, zeros)?);
            }
            sum
        } else {
            Polynomial::product(domain, &operands, zeros)?
        };
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

    /// A cast between `Int`'s `!kgen.scalar<index>` storage and `index`
    /// over the canonical polynomial: each coefficient is kept and each
    /// atom maps to its cast, a cast of the inverse cast being its operand,
    /// so `(k + j).__mlir_index__()` and `k.__mlir_index__() +
    /// j.__mlir_index__()` are one node and a round trip is the identity.
    /// An `Int` operand casts its storage, as `Int.__mlir_index__()` reads
    /// it, and a symbolic `IntLiteral` one casts from the literal domain,
    /// as `IntLiteral.__mlir_index__()` does at the pin.
    fn build_cast(&self, op: ParamOp, operand: &ParamExpr) -> Result<ParamExpr, ParamError> {
        let (from, to, inverse) = if op == ParamOp::CastToBuiltin {
            let from = match IntDomain::of(operand.meta()) {
                Some(IntDomain::Literal) if operand.as_constant().is_none() => IntDomain::Literal,
                _ => IntDomain::IntStorage,
            };
            (from, IntDomain::Index, ParamOp::CastFromBuiltin)
        } else {
            (
                IntDomain::Index,
                IntDomain::IntStorage,
                ParamOp::CastToBuiltin,
            )
        };
        let operand = match (operand.as_constant(), IntDomain::of(operand.meta())) {
            (Some(CtValue::IntLiteral(value)), _) if from == IntDomain::IntStorage => {
                self.integer_literal(from, value)
            }
            (_, Some(IntDomain::Int)) if from == IntDomain::IntStorage => {
                self.field(operand, crate::ct::MLIR_VALUE_FIELD, from.meta())?
            }
            _ => operand.clone(),
        };
        if IntDomain::of(operand.meta()) != Some(from) {
            return Err(ParamError::TypeMismatch {
                operation: op.name().to_string(),
                expected: from.name(),
                found: operand.meta().to_string(),
            });
        }
        self.map_polynomial(from, to, &operand, |atom| match atom.kind() {
            ParamKind::Op { op, operands }
                if *op == inverse && IntDomain::of(operands[0].meta()) == Some(to) =>
            {
                operands[0].clone()
            }
            _ => self.make(
                to.meta(),
                ParamKind::Op {
                    op,
                    operands: vec![atom.clone()],
                },
            ),
        })
    }

    /// `operand`, of the domain `from`, mapped term by term into `to`: each
    /// coefficient kept, each atom replaced by `image(atom)`. A ring
    /// isomorphism such as a cast or a scalar's `struct.extract` and
    /// `struct.create` distributes over the canonical polynomial, so its
    /// image is canonical too.
    fn map_polynomial(
        &self,
        from: IntDomain,
        to: IntDomain,
        operand: &ParamExpr,
        image: impl Fn(&ParamExpr) -> ParamExpr,
    ) -> Result<ParamExpr, ParamError> {
        let mut mapped = Polynomial::zero(to);
        for (monomial, coefficient) in &Polynomial::of(from, operand)?.terms {
            let mut term = Polynomial::constant(to, coefficient);
            for atom in monomial {
                term = term.mul(&Polynomial::of(to, &image(atom))?)?;
            }
            mapped = mapped.add(&term);
        }
        if mapped.as_constant().is_some() {
            self.count_fold();
        }
        Ok(self.polynomial_node(&mapped))
    }

    /// A scalar's `_mlir_value` (`struct.extract`) over the canonical
    /// polynomial of `base`, of the scalar domain `domain`: each atom's
    /// storage is its field, a scalar created from a storage being that
    /// storage, so `(n + 1)._mlir_value` is `n._mlir_value + 1`.
    fn build_extract(&self, domain: IntDomain, base: &ParamExpr) -> Result<ParamExpr, ParamError> {
        let storage = domain.storage().ok_or_else(|| ParamError::TypeMismatch {
            operation: format!("field `{}`", crate::ct::MLIR_VALUE_FIELD),
            expected: "an `Int` or a `UInt`".to_string(),
            found: base.meta().to_string(),
        })?;
        self.map_polynomial(domain, storage, base, |atom| {
            match scalar_storage_field(atom)
                .filter(|field| IntDomain::of(field.meta()) == Some(storage))
            {
                Some(field) => field.clone(),
                None => self.make(
                    storage.meta(),
                    ParamKind::Field {
                        base: atom.clone(),
                        name: crate::ct::MLIR_VALUE_FIELD.to_string(),
                    },
                ),
            }
        })
    }

    /// The scalar of the domain `scalar` created from `storage`
    /// (`struct.create`) over its canonical polynomial: each atom that is a
    /// scalar's field is that scalar, and any other the struct value of it,
    /// so `Int(mlir_value=n._mlir_value + 1)` is `n + 1`. A created atom is
    /// spelled `spelling` when it is the whole storage, and otherwise the
    /// application of [`SCALAR_FROM_STORAGE_FUNCTION`] to it.
    fn build_create(
        &self,
        scalar: IntDomain,
        storage: &ParamExpr,
        spelling: Option<&ParamExpr>,
    ) -> Result<ParamExpr, ParamError> {
        let domain = scalar.storage().ok_or_else(|| ParamError::TypeMismatch {
            operation: "struct value".to_string(),
            expected: "an `Int` or a `UInt`".to_string(),
            found: scalar.name(),
        })?;
        let meta = scalar.meta();
        let whole = Polynomial::of(domain, storage)?
            .as_atom()
            .is_some_and(|atom| atom == storage);
        self.map_polynomial(domain, scalar, storage, |atom| match atom.kind() {
            ParamKind::Field { base, name }
                if name == crate::ct::MLIR_VALUE_FIELD
                    && IntDomain::of(base.meta()) == Some(scalar) =>
            {
                base.clone()
            }
            _ => {
                let spelling = match spelling {
                    Some(spelling) if whole => self.intern(spelling),
                    _ => self.apply(
                        SCALAR_FROM_STORAGE_FUNCTION,
                        std::slice::from_ref(atom),
                        meta.clone(),
                    ),
                };
                self.make(
                    meta.clone(),
                    ParamKind::StructValue {
                        fields: vec![(crate::ct::MLIR_VALUE_FIELD.to_string(), atom.clone())],
                        spelling: Spelling(spelling),
                    },
                )
            }
        })
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
                    self.node_meta(domain, ParamOp::Mul, &factors),
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
                self.node_meta(domain, ParamOp::Add, &terms),
                ParamKind::Op {
                    op: ParamOp::Add,
                    operands: terms,
                },
            ),
        }
    }

    /// The meta-type of a polynomial node `op` over `operands` in
    /// `domain`: a machine domain's one type, and in the literal domain
    /// [`Self::literal_node_meta`].
    fn node_meta(&self, domain: IntDomain, op: ParamOp, operands: &[ParamExpr]) -> MetaTy {
        match domain {
            IntDomain::Literal => self.literal_node_meta(op, operands),
            _ => domain.meta(),
        }
    }

    /// The meta-type of a literal-domain node `op` over `operands`: the
    /// `IntLiteral` whose slot is the node's `!pop.int_literal` image, the
    /// operands' slots joined left to right by `#pop<int_literal_bin<…>>`,
    /// as upstream types `x * 2` over `x: IntLiteral[_]`
    /// `IntLiteral[#pop.int_literal_bin<mul x.value, 2>]`.
    fn literal_node_meta(&self, op: ParamOp, operands: &[ParamExpr]) -> MetaTy {
        let slot = |operand: &ParamExpr| match operand.as_constant() {
            Some(CtValue::IntLiteral(value)) => {
                self.constant(CtValue::PopIntLiteral(value.clone())).ok()
            }
            _ => operand
                .meta()
                .as_value()
                .and_then(|ty| ty.kind().int_literal_slot().cloned())
                .and_then(|slot| self.constant(slot).ok()),
        };
        let image = operands
            .iter()
            .map(slot)
            .collect::<Option<Vec<_>>>()
            .and_then(|slots| {
                let mut slots = slots.into_iter();
                let first = slots.next()?;
                slots.try_fold(first, |joined, slot| {
                    self.build_pop_literal(op, vec![joined, slot]).ok()
                })
            });
        image.map_or_else(MetaTy::int, |image| {
            MetaTy::value(Ty::int_literal_at(image.into_value()))
        })
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

    /// KGEN's `data_to_str` over a string view and a list of views: the
    /// `!kgen.string` of their bytes once both are constant, and otherwise
    /// the node, as KGEN keeps an unresolved `#kgen.param.expr`.
    fn build_data_to_str(&self, operands: Vec<ParamExpr>) -> Result<ParamExpr, ParamError> {
        if let [Some(string), Some(rest)] = [operands[0].as_constant(), operands[1].as_constant()] {
            self.count_fold();
            return self.constant(fold_primitive(ParamOp::DataToStr, &[string, rest])?);
        }
        Ok(self.make(
            MetaTy::value(TyKind::KgenString.into()),
            ParamKind::Op {
                op: ParamOp::DataToStr,
                operands,
            },
        ))
    }

    /// A `#pop` attribute over one `!pop.int_literal` or `!pop.float_literal`
    /// operand ([`ParamOp::IntToFloatLiteral`] through
    /// [`ParamOp::FloatLiteralIsNormal`]): its fold once the operand is
    /// constant, and otherwise the node, whose type the attribute spells.
    fn build_literal_attribute(
        &self,
        op: ParamOp,
        operands: Vec<ParamExpr>,
    ) -> Result<ParamExpr, ParamError> {
        let (operand, result) = match op {
            ParamOp::IntToFloatLiteral => (
                TyKind::PopIntLiteral,
                MetaTy::value(TyKind::PopFloatLiteral.into()),
            ),
            ParamOp::FloatToIntLiteral => (
                TyKind::PopFloatLiteral,
                MetaTy::value(TyKind::PopIntLiteral.into()),
            ),
            _ => (TyKind::PopFloatLiteral, MetaTy::bool()),
        };
        if *operands[0].meta() != MetaTy::value(operand.clone().into()) {
            return Err(ParamError::TypeMismatch {
                operation: op.name().to_string(),
                expected: operand.to_string(),
                found: operands[0].meta().to_string(),
            });
        }
        if let Some(value) = operands[0].as_constant() {
            self.count_fold();
            return self.constant(fold_primitive(op, &[value])?);
        }
        Ok(self.make(result, ParamKind::Op { op, operands }))
    }

    /// `#pop.string_concat` over two `!kgen.string`s: the string of both
    /// texts once both are constant, and otherwise the node.
    fn build_string_concat(&self, operands: Vec<ParamExpr>) -> Result<ParamExpr, ParamError> {
        if let [Some(left), Some(right)] = [operands[0].as_constant(), operands[1].as_constant()] {
            self.count_fold();
            return self.constant(fold_primitive(ParamOp::StringConcat, &[left, right])?);
        }
        Ok(self.make(
            MetaTy::value(TyKind::KgenString.into()),
            ParamKind::Op {
                op: ParamOp::StringConcat,
                operands,
            },
        ))
    }

    /// KGEN's `target_get_field` or `target_has_feature` over a target and
    /// a `!kgen.string` name: the field's value or the feature's presence
    /// once both are constant, and otherwise the node. A field is a
    /// `!kgen.string` or an `index` by its name, which must be constant.
    fn build_target_read(
        &self,
        op: ParamOp,
        operands: Vec<ParamExpr>,
    ) -> Result<ParamExpr, ParamError> {
        if let [Some(target), Some(name)] = [operands[0].as_constant(), operands[1].as_constant()] {
            self.count_fold();
            return self.constant(fold_primitive(op, &[target, name])?);
        }
        let result = match (op, operands[1].as_constant()) {
            (ParamOp::TargetHasFeature, _) => MetaTy::bool(),
            (_, Some(CtValue::KgenString(name))) if crate::ct::KgenTarget::is_index_field(name) => {
                MetaTy::value(Ty::index())
            }
            (_, Some(CtValue::KgenString(_))) => MetaTy::value(TyKind::KgenString.into()),
            _ => {
                return Err(ParamError::TypeMismatch {
                    operation: op.name().to_string(),
                    expected: "a constant '!kgen.string' field name".to_string(),
                    found: operands[1].meta().to_string(),
                });
            }
        };
        Ok(self.make(result, ParamKind::Op { op, operands }))
    }

    /// `pop.dtype.to_ui8` over a `!kgen.dtype`: its code once the dtype is
    /// constant, and otherwise the node, a `UInt8`.
    fn build_dtype_code(&self, operands: Vec<ParamExpr>) -> Result<ParamExpr, ParamError> {
        if let Some(dtype) = operands[0].as_constant() {
            self.count_fold();
            return self.constant(fold_primitive(ParamOp::DtypeToUi8, &[dtype])?);
        }
        if *operands[0].meta() != MetaTy::value(Ty::kgen_dtype()) {
            return Err(ParamError::TypeMismatch {
                operation: ParamOp::DtypeToUi8.name().to_string(),
                expected: crate::types::KGEN_DTYPE_TYPE_SPELLING.to_string(),
                found: operands[0].meta().to_string(),
            });
        }
        Ok(self.make(
            MetaTy::value(crate::types::canonical_simd_ty(Dtype::UInt8, 1)),
            ParamKind::Op {
                op: ParamOp::DtypeToUi8,
                operands,
            },
        ))
    }

    /// `store_to_mem` over a value: the frozen reference once the value is
    /// constant, and otherwise the node, an immutable reference into
    /// compile-time memory, whose origin is the static one.
    fn build_store_to_mem(&self, operands: Vec<ParamExpr>) -> Result<ParamExpr, ParamError> {
        if let Some(referent) = operands[0].as_constant() {
            self.count_fold();
            return self.constant(fold_primitive(ParamOp::StoreToMem, &[referent])?);
        }
        let referent =
            operands[0]
                .meta()
                .as_value()
                .cloned()
                .ok_or_else(|| ParamError::TypeMismatch {
                    operation: ParamOp::StoreToMem.name().to_string(),
                    expected: "a compile-time value".to_string(),
                    found: operands[0].meta().to_string(),
                })?;
        let reference = TyKind::Ref(crate::origin::RefTy {
            referent,
            origin: crate::origin::Origin::Static,
            mutability: crate::origin::Mutability::Immutable,
        });
        Ok(self.make(
            MetaTy::value(reference.into()),
            ParamKind::Op {
                op: ParamOp::StoreToMem,
                operands,
            },
        ))
    }

    /// `pop.string.size` or `pop.string.address` over a `!kgen.string`:
    /// its byte length or the address of its bytes once the string is
    /// constant, and otherwise the node.
    fn build_string_read(
        &self,
        op: ParamOp,
        operands: Vec<ParamExpr>,
    ) -> Result<ParamExpr, ParamError> {
        if let Some(string) = operands[0].as_constant() {
            self.count_fold();
            return self.constant(fold_primitive(op, &[string])?);
        }
        let result = if op == ParamOp::StringSize {
            TyKind::Index.into()
        } else {
            crate::types::static_string_address_ty()
        };
        Ok(self.make(MetaTy::value(result), ParamKind::Op { op, operands }))
    }

    /// `pop.simd.splat` or `pop.cast` at the type its first operand names:
    /// the vector or scalar of the converted lane once the type and the
    /// value are closed, and otherwise the node, a value of that type.
    fn build_lane_op(
        &self,
        op: ParamOp,
        operands: Vec<ParamExpr>,
    ) -> Result<ParamExpr, ParamError> {
        let ty = match operands[0].kind() {
            ParamKind::TypeShape(ty) | ParamKind::Constant(CtValue::Type(ty)) => ty.clone(),
            _ => {
                return Err(ParamError::TypeMismatch {
                    operation: op.name().to_string(),
                    expected: "a type".to_string(),
                    found: operands[0].meta().to_string(),
                });
            }
        };
        if let [Some(result), Some(value)] = [operands[0].as_constant(), operands[1].as_constant()]
        {
            self.count_fold();
            return self.typed_constant(fold_primitive(op, &[result, value])?, &ty);
        }
        Ok(self.make(MetaTy::value(ty), ParamKind::Op { op, operands }))
    }

    /// A `#pop<int_literal_bin>`, `#pop<float_literal_bin>`, or `…_cmp`
    /// attribute: an arithmetic operator or a comparison over two
    /// `!pop.int_literal` or two `!pop.float_literal` operands, folded
    /// exactly once both are constant, and otherwise the node, of the
    /// operands' type or a `Bool`. A closed operator whose fold fails (a
    /// zero divisor) stays a node, as [`Self::build_opaque`] keeps one.
    /// The operators are upstream's: `int_literal_bin` takes `add sub mul
    /// floordiv mod pow lshift rshift and xor or`, and `float_literal_bin`
    /// `add sub mul truediv floordiv`; a fold leaving the operands' kind
    /// (an integer power with a negative exponent) is no fold.
    fn build_pop_literal(
        &self,
        op: ParamOp,
        operands: Vec<ParamExpr>,
    ) -> Result<ParamExpr, ParamError> {
        let literal = operands[0].meta().clone();
        if let Some(other) = operands.iter().find(|operand| *operand.meta() != literal) {
            return Err(ParamError::TypeMismatch {
                operation: op.name().to_string(),
                expected: literal.to_string(),
                found: other.meta().to_string(),
            });
        }
        let float = literal == MetaTy::value(TyKind::PopFloatLiteral.into());
        let accepted = match op {
            ParamOp::Add
            | ParamOp::Sub
            | ParamOp::Mul
            | ParamOp::FloorDiv
            | ParamOp::Eq
            | ParamOp::Lt
            | ParamOp::Le => true,
            ParamOp::Div => float,
            ParamOp::Mod
            | ParamOp::Pow
            | ParamOp::Shl
            | ParamOp::Shr
            | ParamOp::BitAnd
            | ParamOp::BitXor
            | ParamOp::BitOr => !float,
            _ => false,
        };
        if !accepted || operands.len() != 2 {
            return Err(ParamError::Unsupported(format!(
                "'{}' is not an operator of '{literal}'",
                op.name()
            )));
        }
        let result = if matches!(op, ParamOp::Eq | ParamOp::Lt | ParamOp::Le) {
            MetaTy::bool()
        } else {
            literal
        };
        if let [Some(left), Some(right)] = [operands[0].as_constant(), operands[1].as_constant()]
            && let Ok(value) = fold_primitive(op, &[left, right])
            && MetaTy::of_value(&value) == result
        {
            self.count_fold();
            return self.constant(value);
        }
        Ok(self.make(result, ParamKind::Op { op, operands }))
    }

    /// `#pop.int_literal_convert` or `#pop<float_literal_convert>` of a
    /// literal at the `!kgen.simd` storage type its first operand names:
    /// the storage of the vector holding the literal in every lane once
    /// both are constant, and otherwise the node, a value of that type.
    fn build_literal_convert(
        &self,
        op: ParamOp,
        operands: Vec<ParamExpr>,
    ) -> Result<ParamExpr, ParamError> {
        let literal = if op == ParamOp::IntLiteralConvert {
            TyKind::PopIntLiteral
        } else {
            TyKind::PopFloatLiteral
        };
        if *operands[1].meta() != MetaTy::value(literal.clone().into()) {
            return Err(ParamError::TypeMismatch {
                operation: op.name().to_string(),
                expected: literal.to_string(),
                found: operands[1].meta().to_string(),
            });
        }
        self.build_lane_op(op, operands)
    }

    /// KGEN's `get_env` at a result type shape over a `!kgen.string` name:
    /// the define's presence or value once the name is constant, and
    /// otherwise the node, of the result type.
    fn build_get_env(&self, operands: Vec<ParamExpr>) -> Result<ParamExpr, ParamError> {
        if *operands[1].meta() != MetaTy::value(TyKind::KgenString.into()) {
            return Err(ParamError::TypeMismatch {
                operation: ParamOp::GetEnv.name().to_string(),
                expected: "a '!kgen.string' define name".to_string(),
                found: operands[1].meta().to_string(),
            });
        }
        self.build_lane_op(ParamOp::GetEnv, operands)
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

    /// Numeric comparison. Constants fold; identical operands prove `==`,
    /// and an integer `==` whose difference is a constant is decided by it.
    /// Anything else, `n < n` and `n <= n` included, is a residual
    /// proposition, as the pin's canonicalizer keeps it.
    fn build_comparison(
        &self,
        op: ParamOp,
        operands: &[ParamExpr],
    ) -> Result<ParamExpr, ParamError> {
        let (left, right) = (operands[0].clone(), operands[1].clone());
        if op != ParamOp::Eq && [&left, &right].iter().any(|o| *o.meta() == MetaTy::Type) {
            return Err(ParamError::Unsupported(
                "only == and != are defined for compile-time types".to_string(),
            ));
        }
        if let (Some(a), Some(b)) = (left.as_constant(), right.as_constant()) {
            self.count_fold();
            return fold_primitive(op, &[a, b]).and_then(|value| self.constant(value));
        }
        // `x < 10` over a non-`Int` scalar is `SIMD.__lt__(self, rhs: Self)`,
        // whose implicit `SIMD.__init__(IntLiteral)` (or `FloatLiteral`)
        // materializes the literal at the other operand's type.
        let (left, right) = (
            self.literal_at(left, operands[1].meta()),
            self.literal_at(right, operands[0].meta()),
        );
        let operands = &[left.clone(), right.clone()];
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
        if op == ParamOp::Eq && left == right && left.is_total() {
            return Ok(self.boolean(true));
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

    /// `operand`, a literal constant, materialized at the `SIMD` type
    /// `other` is a value of, as the pin's implicit
    /// `SIMD.__init__(IntLiteral)` and `SIMD.__init__(FloatLiteral)` fold
    /// it, splatting across a vector's lanes; any other operand, or one
    /// beside an integer-domain `other` (which [`Self::arithmetic_domain`]
    /// converts), unchanged. At a `SIMD` whose dtype or width is still
    /// symbolic, the materialized literal is that `__init__`'s struct value:
    /// its `_mlir_value` the `#pop.int_literal_convert` (or
    /// `#pop<float_literal_convert>`) of the literal at the vector's
    /// `!kgen.simd` storage, which folds to the vector once the type closes.
    fn literal_at(&self, operand: ParamExpr, other: &MetaTy) -> ParamExpr {
        let Some(ty) = other.as_value().filter(|_| IntDomain::of(other).is_none()) else {
            return operand;
        };
        let Some(value) = operand
            .as_constant()
            .filter(|value| matches!(value, CtValue::IntLiteral(_) | CtValue::FloatLiteral(_)))
        else {
            return operand;
        };
        let materialized = match ty.kind().known_simd() {
            Some((dtype, width)) => match width {
                1 => value.clone().materialize_as(ty),
                _ => usize::try_from(width)
                    .ok()
                    .and_then(|width| fold::splat_lanes(value, dtype, width))
                    .map(|lanes| CtValue::Simd { dtype, lanes }),
            }
            .map(|value| self.make(other.clone(), ParamKind::Constant(value))),
            None => self.symbolic_literal_at(&operand, value, ty),
        };
        materialized.unwrap_or(operand)
    }

    /// The struct value upstream's `SIMD.__init__(IntLiteral)` or
    /// `SIMD.__init__(FloatLiteral)` builds from the literal `operand`
    /// (whose constant is `value`) at the open `SIMD` type `ty`.
    fn symbolic_literal_at(
        &self,
        operand: &ParamExpr,
        value: &CtValue,
        ty: &Ty,
    ) -> Option<ParamExpr> {
        let storage = crate::types::simd_storage(ty)?;
        let (op, init) = match value {
            CtValue::IntLiteral(_) => (
                ParamOp::IntLiteralConvert,
                crate::types::SIMD_FROM_INT_LITERAL,
            ),
            _ => (
                ParamOp::FloatLiteralConvert,
                crate::types::SIMD_FROM_FLOAT_LITERAL,
            ),
        };
        let literal = self.constant(value.literal_value()?).ok()?;
        let converted = self
            .op(op, &[self.type_shape(storage), literal.clone()])
            .ok()?;
        let spelling = self.apply(
            init,
            &[self.type_shape(ty.clone()), literal, operand.clone()],
            MetaTy::value(ty.clone()),
        );
        Some(self.struct_value(
            &[(crate::ct::MLIR_VALUE_FIELD.to_string(), converted)],
            &spelling,
        ))
    }

    /// A lane compare over two `!kgen.simd` storages: the mask once both
    /// are closed, and otherwise the node, its `eq` and `ne` operands
    /// ordered.
    fn build_simd_cmp(
        &self,
        op: ParamOp,
        operands: Vec<ParamExpr>,
    ) -> Result<ParamExpr, ParamError> {
        let mask = lane_mask_ty(op, &operands)?;
        if let [Some(left), Some(right)] = [operands[0].as_constant(), operands[1].as_constant()]
            && let Ok(value) = fold_primitive(op, &[left, right])
        {
            self.count_fold();
            return self.typed_constant(value, &mask);
        }
        let operands = match op {
            ParamOp::SimdCmpEq | ParamOp::SimdCmpNe => {
                let [left, right]: [ParamExpr; 2] =
                    operands
                        .try_into()
                        .map_err(|operands: Vec<_>| ParamError::Arity {
                            operation: op.name().to_string(),
                            found: operands.len(),
                        })?;
                let (left, right) = ordered(left, right);
                vec![left, right]
            }
            _ => operands,
        };
        Ok(self.make(MetaTy::value(mask), ParamKind::Op { op, operands }))
    }

    /// A reduction of a `bool` mask, as the pin's simplifier folds the
    /// `SIMD` comparison dunders' bodies: a closed mask reduces; any lane
    /// count's `reduce_and(x.eq(y))` is `x == y`, and its
    /// `reduce_or(x.lt(y).ne(0))` (`__lt__`'s `self.lt(rhs).__bool__()`)
    /// is `x < y`, `le` likewise. Over one lane both reductions read
    /// alike, so either over `x.eq(y)` is `x == y`, over a truth-tested
    /// `x.lt(y)` is `x < y`, over any other `x.ne(y)` is `not x == y`, and
    /// any other is `reduce_and`. A vector's other reductions stay nodes.
    fn build_simd_reduce(&self, op: ParamOp, mask: &ParamExpr) -> Result<ParamExpr, ParamError> {
        let width = match mask
            .meta()
            .as_value()
            .and_then(|ty| crate::types::kgen_simd_slots(ty.kind()))
        {
            Some((dtype, width)) if dtype.known().is_none_or(|dtype| dtype == Dtype::Bool) => width,
            _ => {
                return Err(ParamError::TypeMismatch {
                    operation: op.name().to_string(),
                    expected: "a 'bool' '!kgen.simd' mask".to_string(),
                    found: mask.meta().to_string(),
                });
            }
        };
        if let Some(value) = mask.as_constant()
            && let Ok(value) = fold_primitive(op, &[value])
        {
            self.count_fold();
            return self.constant(value);
        }
        let one_lane = matches!(width, crate::types::SimdWidth::Known(1));
        if let Some(compared) = self.reduced_comparison(op, mask, one_lane)? {
            return Ok(compared);
        }
        let op = if one_lane { ParamOp::SimdReduceAnd } else { op };
        Ok(self.make(
            MetaTy::bool(),
            ParamKind::Op {
                op,
                operands: vec![mask.clone()],
            },
        ))
    }

    /// The source comparison the reduction `op` of `mask` is
    /// ([`Self::build_simd_reduce`]), or `None` when it is none or a
    /// compared storage has no value to compare.
    fn reduced_comparison(
        &self,
        op: ParamOp,
        mask: &ParamExpr,
        one_lane: bool,
    ) -> Result<Option<ParamExpr>, ParamError> {
        let ParamKind::Op {
            op: compare,
            operands,
        } = mask.kind()
        else {
            return Ok(None);
        };
        let [left, right] = operands.as_slice() else {
            return Ok(None);
        };
        let any = op == ParamOp::SimdReduceOr || one_lane;
        let every = op == ParamOp::SimdReduceAnd || one_lane;
        let (source, negated, left, right) = match (compare, truth_tested_order(left, right)) {
            (ParamOp::SimdCmpEq, _) if every => (ParamOp::Eq, false, left, right),
            (ParamOp::SimdCmpNe, Some((ordering, left, right))) if any => {
                (ordering, false, left, right)
            }
            (ParamOp::SimdCmpNe, _) if one_lane => (ParamOp::Eq, true, left, right),
            _ => return Ok(None),
        };
        let (Some(left), Some(right)) = (self.storage_owner(left), self.storage_owner(right))
        else {
            return Ok(None);
        };
        let compared = self.op(source, &[left, right])?;
        Ok(Some(if negated {
            self.not(&compared)?
        } else {
            compared
        }))
    }

    /// The value whose `_mlir_value` `storage` is: the base of a field
    /// read, the `SIMD` (or `Int`) a closed storage holds, or the `Int` or
    /// `UInt` created from an integer storage ([`Self::build_create`]);
    /// `None` for any other storage.
    fn storage_owner(&self, storage: &ParamExpr) -> Option<ParamExpr> {
        match storage.kind() {
            ParamKind::Field { base, name } if name == crate::ct::MLIR_VALUE_FIELD => {
                Some(base.clone())
            }
            ParamKind::Constant(value) => self.constant(value.clone()).ok(),
            _ => match IntDomain::of(storage.meta())? {
                IntDomain::IntStorage => self.build_create(IntDomain::Int, storage, None).ok(),
                IntDomain::UIntStorage => self.build_create(IntDomain::UInt, storage, None).ok(),
                _ => None,
            },
        }
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

    /// `element` of a tabulation with its index bound to `index`.
    fn tabulate_element(
        &self,
        element: &ParamExpr,
        index: &ParamExpr,
    ) -> Result<ParamExpr, ParamError> {
        let mut bindings = ParamBindings::new();
        bindings.push_frame(vec![Some(index.clone())]);
        let mut memo = HashMap::new();
        self.replace_at(element, &bindings, 0, &mut memo)
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
                Some(value) => self.bound_value(expr, value, bindings, depth)?,
                None => expr.clone(),
            },
            ParamKind::IndexRef {
                depth: reference,
                index,
            } => match reference
                .checked_sub(depth)
                .and_then(|outer| bindings.lookup_index(outer, *index))
            {
                Some(value) => self.bound_value(expr, value, bindings, depth)?,
                None => expr.clone(),
            },
            ParamKind::Op { op, operands } => {
                let operands: Vec<ParamExpr> = operands
                    .iter()
                    .map(|operand| self.replace_at(operand, bindings, depth, memo))
                    .collect::<Result<_, _>>()?;
                if op.is_atom() {
                    self.rebuild(*op, &operands)?
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
            ParamKind::ListGet { list, index } => self.list_get(
                &self.replace_at(list, bindings, depth, memo)?,
                &self.replace_at(index, bindings, depth, memo)?,
            )?,
            // The field's type names the binders its base's does
            // (`v._mlir_value`'s `!kgen.simd<length, dtype>` at `v: Int32`
            // is `!kgen.scalar<si32>`).
            ParamKind::Field { base, name } => self.field(
                &self.replace_at(base, bindings, depth, memo)?,
                name,
                self.replace_meta(expr.meta(), bindings, depth)?,
            )?,
            ParamKind::Reflect { subject, query } => self.reflect_query(
                &self.replace_at(subject, bindings, depth, memo)?,
                query.clone(),
            ),
            ParamKind::TypeMember {
                subject,
                name,
                args,
            } => {
                let args: Vec<ParamExpr> = args
                    .iter()
                    .map(|arg| self.replace_at(arg, bindings, depth, memo))
                    .collect::<Result<_, _>>()?;
                self.type_member(
                    &self.replace_at(subject, bindings, depth, memo)?,
                    name,
                    &args,
                    expr.meta().clone(),
                )
            }
            ParamKind::ListTabulate { count, element } => self.list_tabulate(
                &self.replace_at(count, bindings, depth, memo)?,
                &self.replace_at(element, bindings, depth + 1, memo)?,
            )?,
            ParamKind::ListConcat { lists } => {
                let lists: Vec<ParamExpr> = lists
                    .iter()
                    .map(|list| self.replace_at(list, bindings, depth, memo))
                    .collect::<Result<_, _>>()?;
                self.list_concat(&lists)?
            }
            // A bound pack's length is its list's; any other query is over
            // the bound list.
            ParamKind::PackQuery {
                pack,
                query: PackQuery::Length,
            } => self.list_length(&self.replace_at(pack, bindings, depth, memo)?)?,
            ParamKind::PackQuery { pack, query } => {
                self.rewrite_pack_query(pack, query, &mut |operand| {
                    self.replace_at(operand, bindings, depth, memo)
                })?
            }
            ParamKind::Apply { function, args } => {
                let args: Vec<ParamExpr> = args
                    .iter()
                    .map(|arg| self.replace_at(arg, bindings, depth, memo))
                    .collect::<Result<_, _>>()?;
                // The callee's declared result names the binders its
                // arguments do (`Self(7)`'s `S[n]` at `n = 2` is `S[2]`).
                let meta = self.replace_meta(expr.meta(), bindings, depth)?;
                self.apply(function, &args, meta)
            }
            ParamKind::BindParams { generator, args } => {
                let args: Vec<Option<ParamExpr>> = args
                    .iter()
                    .map(|arg| {
                        arg.as_ref()
                            .map(|arg| self.replace_at(arg, bindings, depth, memo))
                            .transpose()
                    })
                    .collect::<Result<_, _>>()?;
                self.bind_params(&self.replace_at(generator, bindings, depth, memo)?, &args)?
            }
            ParamKind::StructValue { fields, spelling } => self.struct_value(
                &rebuilt_fields(fields, |value| {
                    self.replace_at(value, bindings, depth, memo)
                })?,
                &self.replace_at(&spelling.0, bindings, depth, memo)?,
            ),
        };
        memo.insert(key, replaced.clone());
        Ok(replaced)
    }

    /// `meta` with its value domains' parameters replaced, as
    /// [`crate::types::replace_parameters`] replaces a type's.
    fn replace_meta(
        &self,
        meta: &MetaTy,
        bindings: &ParamBindings,
        depth: u32,
    ) -> Result<MetaTy, ParamError> {
        let each = |metas: &[MetaTy]| -> Result<Vec<MetaTy>, ParamError> {
            metas
                .iter()
                .map(|meta| self.replace_meta(meta, bindings, depth))
                .collect()
        };
        Ok(match meta {
            MetaTy::Value(ty) => {
                MetaTy::value(crate::types::replace_parameters(self, ty, bindings, depth)?)
            }
            MetaTy::Type => MetaTy::Type,
            MetaTy::Tuple(elements) => MetaTy::Tuple(each(elements)?),
            MetaTy::List(elements) => MetaTy::List(each(elements)?),
            MetaTy::Set(elements) => MetaTy::Set(each(elements)?),
            MetaTy::Dict(entries) => MetaTy::Dict(
                entries
                    .iter()
                    .map(|(key, value)| {
                        Ok((
                            self.replace_meta(key, bindings, depth)?,
                            self.replace_meta(value, bindings, depth)?,
                        ))
                    })
                    .collect::<Result<_, ParamError>>()?,
            ),
            MetaTy::ParamList(element) => {
                MetaTy::ParamList(Box::new(self.replace_meta(element, bindings, depth)?))
            }
        })
    }

    /// Install a bound value for a reference: its meta-type must be the
    /// reference's after literal conversion, and its free signature indices
    /// shift past the binders the replacement descended through.
    fn bound_value(
        &self,
        reference: &ParamExpr,
        value: &ParamExpr,
        bindings: &ParamBindings,
        depth: u32,
    ) -> Result<ParamExpr, ParamError> {
        // A reference typed by an earlier value binder (`c: Scalar[dtype]`)
        // is typed at that binder's value.
        let declared = self.replace_meta(
            &reference.meta().substitute(bindings.types()),
            bindings,
            depth,
        )?;
        // A reference typed by a type binder no binding here solves
        // (`v: T`) takes the value as typed: its declaration's application
        // solved the binder from it.
        if declared.has_free_parameters() {
            return if depth == 0 {
                Ok(value.clone())
            } else {
                self.shift(value, 0, depth)
            };
        }
        let value = match (&declared, value.as_constant()) {
            // A compile-time string bound to a `StringLiteral[v]` parameter
            // is its literal, whose type carries the text the binding of
            // `v` keys.
            (MetaTy::Value(ty), Some(CtValue::Str(_)))
                if ty.is_string_literal()
                    && value.meta().as_value().is_some_and(Ty::is_string_literal) =>
            {
                return if depth == 0 {
                    Ok(value.clone())
                } else {
                    self.shift(value, 0, depth)
                };
            }
            // A numeric literal bound to an `IntLiteral[v]` or
            // `FloatLiteral[v]` parameter is likewise its literal, whose
            // type carries the value the binding of `v` keys.
            (MetaTy::Value(ty), Some(constant))
                if matches!(constant, CtValue::IntLiteral(_)) && ty.is_int_literal()
                    || matches!(constant, CtValue::FloatLiteral(_)) && ty.is_float_literal() =>
            {
                return if depth == 0 {
                    Ok(value.clone())
                } else {
                    self.shift(value, 0, depth)
                };
            }
            (MetaTy::Value(ty), Some(constant)) if *value.meta() != declared => {
                self.constant_as(constant.clone(), ty)?
            }
            (MetaTy::ParamList(element), Some(constant @ CtValue::Tuple(_)))
                if *value.meta() != declared
                    && element.as_value().is_some_and(is_function)
                    && let Some(ty) = declared.value_ty() =>
            {
                self.constant_as(constant.clone(), &ty)?
            }
            _ => value.clone(),
        };
        if *value.meta() != declared
            && !value.meta().is_generator_of(&declared)
            && !value.meta().is_trait_metatype_of(&declared)
            && !value.meta().is_unresolved_struct(&declared)
            && !value.meta().is_list_of(&declared)
        {
            return Err(ParamError::TypeMismatch {
                operation: format!("binding of '{reference}'"),
                expected: declared.to_string(),
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
            ParamKind::ListGet { list, index } => self.list_get(
                &self.shift(list, cutoff, by)?,
                &self.shift(index, cutoff, by)?,
            )?,
            ParamKind::Field { base, name } => {
                self.field(&self.shift(base, cutoff, by)?, name, expr.meta().clone())?
            }
            ParamKind::ListTabulate { count, element } => self.list_tabulate(
                &self.shift(count, cutoff, by)?,
                &self.shift(element, cutoff + 1, by)?,
            )?,
            ParamKind::ListConcat { lists } => {
                let lists: Vec<ParamExpr> = lists
                    .iter()
                    .map(|list| self.shift(list, cutoff, by))
                    .collect::<Result<_, _>>()?;
                self.list_concat(&lists)?
            }
            ParamKind::Reflect { subject, query } => {
                self.reflect_query(&self.shift(subject, cutoff, by)?, query.clone())
            }
            ParamKind::TypeMember {
                subject,
                name,
                args,
            } => {
                let args: Vec<ParamExpr> = args
                    .iter()
                    .map(|arg| self.shift(arg, cutoff, by))
                    .collect::<Result<_, _>>()?;
                self.type_member(
                    &self.shift(subject, cutoff, by)?,
                    name,
                    &args,
                    expr.meta().clone(),
                )
            }
            ParamKind::Apply { function, args } => {
                let args: Vec<ParamExpr> = args
                    .iter()
                    .map(|arg| self.shift(arg, cutoff, by))
                    .collect::<Result<_, _>>()?;
                self.apply(function, &args, expr.meta().clone())
            }
            ParamKind::BindParams { generator, args } => {
                let args: Vec<Option<ParamExpr>> = args
                    .iter()
                    .map(|arg| {
                        arg.as_ref()
                            .map(|arg| self.shift(arg, cutoff, by))
                            .transpose()
                    })
                    .collect::<Result<_, _>>()?;
                self.bind_params(&self.shift(generator, cutoff, by)?, &args)?
            }
            ParamKind::StructValue { fields, spelling } => self.struct_value(
                &rebuilt_fields(fields, |value| self.shift(value, cutoff, by))?,
                &self.shift(&spelling.0, cutoff, by)?,
            ),
            ParamKind::PackQuery { pack, query } => {
                self.rewrite_pack_query(pack, query, &mut |operand| {
                    self.shift(operand, cutoff, by)
                })?
            }
            _ => expr.clone(),
        })
    }
}

/// The callable symbol of KGEN's `get_sizeof` ([`ParamContext::layout_query`]).
pub const GET_SIZEOF_FUNCTION: &str = "kgen.get_sizeof";

/// The callable symbol of KGEN's `get_alignof` ([`ParamContext::layout_query`]).
pub const GET_ALIGNOF_FUNCTION: &str = "kgen.get_alignof";

/// The layout KGEN's `get_sizeof` or `get_alignof` reads of a type
/// ([`ParamContext::layout_query`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutQuery {
    Size,
    Align,
}

impl LayoutQuery {
    /// The query a layout application's callable symbol names.
    pub fn of_function(function: &str) -> Option<Self> {
        match function {
            GET_SIZEOF_FUNCTION => Some(Self::Size),
            GET_ALIGNOF_FUNCTION => Some(Self::Align),
            _ => None,
        }
    }

    /// The callable symbol of the query's application.
    pub const fn function(self) -> &'static str {
        match self {
            Self::Size => GET_SIZEOF_FUNCTION,
            Self::Align => GET_ALIGNOF_FUNCTION,
        }
    }
}

/// The callable symbol of the compilation target the elaborator answers
/// ([`ParamContext::current_target`]).
pub const CURRENT_TARGET_FUNCTION: &str = "kgen.current_target";

/// The callable symbol spelling a scalar created from a storage atom.
///
/// It is a [`ParamKind::StructValue`] spelling (`struct.create` of an `Int`
/// over its `!kgen.scalar`) that names no declaration: the value is its
/// field, and a closed one folds to the scalar's constant.
pub const SCALAR_FROM_STORAGE_FUNCTION: &str = "$scalar_from_storage";

/// The callable symbol of the length of a parameter list the elaborator
/// evaluates ([`ParamContext::list_length`]).
pub const LIST_LENGTH_FUNCTION: &str = "param_list.length";

/// The answer to an application of a builtin compile-time function.
///
/// `None` for an application this policy does not own or whose arguments are
/// not yet constant.
pub fn builtin_application_value(
    function: &str,
    args: &[ParamExpr],
) -> Option<Result<CtValue, ParamError>> {
    if function != LIST_LENGTH_FUNCTION {
        return None;
    }
    let [list] = args else {
        return None;
    };
    let values = list.as_constant()?.comptime_iteration_elements()?;
    Some(
        i64::try_from(values.len())
            .map(CtValue::Int)
            .map_err(|_| ParamError::Arithmetic("parameter list is too long to count".to_string())),
    )
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

    /// The hole state this node is, when it is one.
    pub fn as_hole(&self) -> Option<HoleKind> {
        match self.kind() {
            ParamKind::Hole { kind, .. } => Some(*kind),
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

    /// The frozen struct a folded construction of a non-generic struct
    /// builds once every field is constant (`SIMDLength(1)`), as a value a
    /// compile-time application passes; `None` for any other expression.
    pub fn frozen_struct(&self) -> Option<CtValue> {
        let ParamKind::StructValue { fields, .. } = self.kind() else {
            return None;
        };
        let TyKind::Struct(name, arguments) = self.meta().as_value()?.kind() else {
            return None;
        };
        if !arguments.is_empty() {
            return None;
        }
        Some(CtValue::Struct {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|(field, value)| Some((field.clone(), value.require_constant().ok()?)))
                .collect::<Option<_>>()?,
        })
    }

    /// The integer a closed `Int`-valued expression denotes.
    pub fn as_i64(&self) -> Option<i64> {
        self.require_constant()
            .ok()
            .as_ref()
            .and_then(fold::integer_value)
            .and_then(|value| value.to_i64())
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
        self.visit(&mut |node| {
            if let ParamKind::DeclRef(reference) = node.kind() {
                output.insert(reference.name.to_string());
            }
        });
    }

    /// Pre-order visit of this node and every operand. Embedded [`Ty`]
    /// payloads are the type visitors' to walk.
    pub fn visit(&self, visitor: &mut dyn FnMut(&Self)) {
        visitor(self);
        for operand in self.operands() {
            operand.visit(visitor);
        }
    }

    /// The expressions directly below this node, in order. A tabulation's
    /// element is the last of its two, under the tabulation's own binder.
    pub fn operands(&self) -> Vec<&Self> {
        match self.kind() {
            ParamKind::Op { operands, .. } => operands.iter().collect(),
            ParamKind::Identical(left, right) => vec![left, right],
            ParamKind::Conforms { subject, .. }
            | ParamKind::Trivial { subject, .. }
            | ParamKind::Reflect { subject, .. }
            | ParamKind::Field { base: subject, .. } => vec![subject],
            ParamKind::TypeMember { subject, args, .. } => {
                std::iter::once(subject).chain(args).collect()
            }
            ParamKind::Select { index, .. } => vec![index],
            ParamKind::ListGet { list, index } => vec![list, index],
            ParamKind::ListTabulate { count, element } => vec![count, element],
            ParamKind::ListConcat { lists } => lists.iter().collect(),
            ParamKind::PackQuery {
                pack,
                query: PackQuery::Contains(element) | PackQuery::IndexOf(element),
            } => vec![pack, element],
            ParamKind::PackQuery { pack, .. } => vec![pack],
            ParamKind::Apply { args, .. } => args.iter().collect(),
            ParamKind::BindParams { generator, args } => std::iter::once(generator)
                .chain(args.iter().flatten())
                .collect(),
            ParamKind::StructValue { fields, spelling } => fields
                .iter()
                .map(|(_, value)| value)
                .chain(std::iter::once(&spelling.0))
                .collect(),
            ParamKind::Constant(_)
            | ParamKind::DeclRef(_)
            | ParamKind::IndexRef { .. }
            | ParamKind::TypeShape(_)
            | ParamKind::Hole { .. } => Vec::new(),
        }
    }

    /// The embedded types of this node (not of its operands).
    pub fn embedded_types(&self) -> Vec<&Ty> {
        match self.kind() {
            ParamKind::TypeShape(ty) => vec![ty],
            ParamKind::Select { elements, .. } => elements.iter().collect(),
            ParamKind::Constant(CtValue::Type(ty)) => vec![ty],
            _ => Vec::new(),
        }
    }

    pub(crate) fn carries_decorations(&self) -> bool {
        matches!(self.kind(), ParamKind::StructValue { .. })
            || self
                .embedded_types()
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
            ParamKind::ListGet { .. } => 9,
            ParamKind::Reflect { .. } => 10,
            ParamKind::PackQuery { .. } => 11,
            ParamKind::Apply { .. } => 12,
            ParamKind::Hole { .. } => 13,
            ParamKind::Field { .. } => 14,
            ParamKind::ListTabulate { .. } => 15,
            ParamKind::ListConcat { .. } => 16,
            ParamKind::TypeMember { .. } => 17,
            ParamKind::BindParams { .. } => 18,
            ParamKind::StructValue { .. } => 19,
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

/// A node is written as its meta-type and payload; a read one is built
/// afresh, with the fingerprint its structure hashes to, so it equals and
/// hashes like the node it was written from in any context.
impl serde::Serialize for ParamExpr {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (self.meta(), self.kind()).serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for ParamExpr {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (meta, kind) = <(MetaTy, ParamKind)>::deserialize(deserializer)?;
        Ok(Self(Arc::new(Node::new(meta, kind))))
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
                (
                    ParamKind::Apply { function, args, .. },
                    ParamKind::Apply {
                        function: other_function,
                        args: other_args,
                        ..
                    },
                ) => function
                    .cmp(other_function)
                    .then_with(|| args.cmp(other_args)),
                (
                    ParamKind::StructValue { fields, .. },
                    ParamKind::StructValue {
                        fields: other_fields,
                        ..
                    },
                ) => fields.cmp(other_fields),
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
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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
    TypeShape(Ty),
    /// Finite type selection by an integer expression.
    Select { elements: Vec<Ty>, index: ParamExpr },
    /// `param_list.get`: one element of a parameter list that is still a
    /// parameter. A constant list is a [`Self::Select`] instead.
    ListGet { list: ParamExpr, index: ParamExpr },
    /// A field of a struct-typed parameter value that is still a parameter
    /// (`Self.e.rows`); a constant base is folded instead.
    Field { base: ParamExpr, name: String },
    /// A query on the reflection of a type that is still a parameter
    /// (`reflect[T].field_count()`). The checker answers a closed subject
    /// from its struct table before building a node; a node whose subject
    /// closes under replacement stays residual for the checker to fold.
    Reflect {
        subject: ParamExpr,
        query: ReflectQuery,
    },
    /// The compile-time value member `name` of a type that is still a
    /// parameter (`T.K`, `Self.T.K`), typed by the member's declared value
    /// type. The checker answers a closed subject from its struct table;
    /// the elaborator answers one per instance from the struct's members.
    /// `args` are a parameterized member's bound arguments in its
    /// declaration order (`T.get[0]`), empty for an unparameterized one.
    TypeMember {
        subject: ParamExpr,
        name: String,
        args: Vec<ParamExpr>,
    },
    /// A query over a pack, which is an operand as upstream's
    /// `pop.variadic.size` and `TypeList` queries take it: the pack
    /// binder's reference in a template, or the list a substitution bound
    /// it to. Only the elaborator decides one.
    PackQuery { pack: ParamExpr, query: PackQuery },
    /// A compile-time application of the callable symbol `function` to
    /// `args`, in argument order. It is never folded: two applications are
    /// one node by structure, as the pin keeps `f(Int(7))` symbolic through
    /// the check, so a type over one never matches the folded value. A
    /// call of an `@always_inline("builtin")` callable whose body folds is
    /// never one: the check builds it as that body's expression. KGEN's
    /// layout reads (`get_sizeof`, `get_alignof`) and `current_target` are
    /// applications of their own symbols. Only the elaborator below MIR
    /// evaluates one, on first demand.
    Apply {
        #[serde(serialize_with = "crate::symbol_ref::serialize")]
        function: String,
        args: Vec<ParamExpr>,
    },
    /// `param_list.tabulate`: the list of `count` elements whose element `i`
    /// is `element` at `i`. `element` sits under one binder of one slot, its
    /// index, which it names as slot `0` of depth `0`; it names no signature
    /// slot outside that binder.
    ListTabulate {
        count: ParamExpr,
        element: ParamExpr,
    },
    /// `param_list.concat`: the lists' elements in order. It holds at least
    /// two lists, none of them a concatenation or an empty constant.
    ListConcat { lists: Vec<ParamExpr> },
    /// `#kgen.bind_params`: `generator` with `args` bound to its parameters
    /// in declaration order, `None` (upstream's `?`) leaving a slot unbound.
    /// A constant generator, or a type's member read unapplied, folds when
    /// built ([`ParamContext::bind_params`]); this node is a generator that
    /// is still a parameter (`G[3]` in `apply[G: …]`).
    BindParams {
        generator: ParamExpr,
        args: Vec<Option<ParamExpr>>,
    },
    /// The struct value a call of an `@always_inline("builtin")` `__init__`
    /// folds to, upstream's folded construction: its fields, in declaration
    /// order, are its identity, so `L(n, 0)` over a body storing `v + w` and
    /// `L(n)` over one storing `v` are one value. `spelling` is the
    /// construction it folded from, an application of the `__init__`, which
    /// a diagnostic shows and the elaborator runs; it takes no part in
    /// equality, hashing, or order. A field read through it is the field
    /// ([`ParamContext::field`]).
    StructValue {
        fields: Vec<(String, ParamExpr)>,
        spelling: Spelling,
    },
    /// Reserved typed unknown/unbound state; see [`ParamContext::hole`].
    /// Its token is one process's, so no encoding carries one. It stays the
    /// last variant: a decoder numbers the variants an encoding skips out.
    #[serde(skip)]
    Hole { kind: HoleKind, token: u64 },
}

/// The construction a [`ParamKind::StructValue`] folded from: shown and
/// elaborated, but every spelling of one struct value is equal and hashes
/// alike, so identity is the fields'.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Spelling(pub ParamExpr);

impl PartialEq for Spelling {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Spelling {}

impl Hash for Spelling {
    fn hash<H: Hasher>(&self, _: &mut H) {}
}

/// The primitive operators. An enum case does not make a source operation
/// supported: arity and operand domains are checked per opcode.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
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
    /// `pop.cast_to_builtin`, `Int.__mlir_index__()`: an `Int` as the
    /// `index` it stores. A ring isomorphism on the canonical polynomial, as
    /// the pin folds it: the cast of a sum or product is the sum or product
    /// of the casts, so only an atom is ever cast.
    CastToBuiltin,
    /// `pop.cast_from_builtin`, `Int(mlir_value=i)`: an `index` as the `Int`
    /// whose storage it is; [`Self::CastToBuiltin`]'s inverse.
    CastFromBuiltin,
    /// KGEN's `data_to_str`: the `!kgen.string` whose text is the bytes a
    /// compile-time string view addresses, then those of each view in a
    /// list, concatenated (upstream's `_get_kgen_string`).
    DataToStr,
    /// `#pop.string_concat`: the `!kgen.string` of two `!kgen.string`s'
    /// texts, the left's first (upstream's `StringLiteral.__add__`).
    StringConcat,
    /// `pop.string.size`: the byte length of a `!kgen.string`, as an
    /// `index` (upstream's `StringLiteral.byte_length`).
    StringSize,
    /// `pop.string.address`: the address of a `!kgen.string`'s
    /// nul-terminated bytes, which live for the whole program (upstream's
    /// `StringLiteral.ptr`).
    StringAddress,
    /// `pop.simd.splat`: a one-lane value across every lane of the
    /// `!kgen.simd` storage type its first operand names, a type shape
    /// (upstream's `_type=`); the node is a value of that storage.
    SimdSplat,
    /// `pop.cast`: a one-lane value converted to the one-lane `!kgen.simd`
    /// storage type its first operand names, a type shape
    /// ([`crate::ct::CtLane::cast`]).
    Cast,
    /// KGEN's `target_get_field`: the field of a `!kgen.target` its second
    /// operand names, a `!kgen.string` ([`crate::ct::KgenTarget::field`]).
    TargetGetField,
    /// KGEN's `target_has_feature`: whether a `!kgen.target`'s feature list
    /// enables the feature its second operand names, a `!kgen.string`
    /// ([`crate::ct::KgenTarget::has_feature`]).
    TargetHasFeature,
    /// KGEN's `get_env`: the compile-time define its second operand names,
    /// a `!kgen.string`, read at the result type its first operand names,
    /// a type shape: whether it is defined (an `i1`), or its value as a
    /// `!kgen.string` or an `index`, which fails over an undefined name
    /// ([`fold::compile_time_define`]).
    GetEnv,
    /// `pop.dtype.to_ui8`: a `DType`'s code, a `UInt8` (upstream's
    /// `DType._as_ui8`), whose mask bits `DType`'s builtin predicates test
    /// ([`ParamContext::dtype_predicate`]).
    DtypeToUi8,
    /// KGEN's `store_to_mem`: a reference to its operand stored in
    /// compile-time memory, which a compile-time call passes at a `ref`
    /// parameter, as upstream's `apply` does (`apply(f, store_to_mem(v))`).
    /// Over a constant it is the frozen reference
    /// ([`CtValue::StoreToMem`]).
    StoreToMem,
    /// `#pop.int_literal_convert<v> : T`: a `!pop.int_literal`, the second
    /// operand, converted into every lane of the `!kgen.simd` storage type
    /// the first operand names, a type shape as [`ParamOp::SimdSplat`]'s is
    /// (upstream's `SIMD.__init__(IntLiteral)`, and
    /// `IntLiteral.__mlir_index__` at `!kgen.scalar<index>`). A lane wraps
    /// to its dtype ([`crate::ct::CtLane::of_int_literal`]); a `bool` lane
    /// is whether the literal is nonzero.
    IntLiteralConvert,
    /// `#pop<float_literal_convert<v>> : T`: a `!pop.float_literal`, the
    /// second operand, rounded once into every lane of the float
    /// `!kgen.simd` storage type the first operand names (upstream's
    /// `SIMD.__init__(FloatLiteral)`).
    FloatLiteralConvert,
    /// `#pop<int_to_float_literal>`: a `!pop.int_literal` as the
    /// `!pop.float_literal` of the same value (upstream's implicit
    /// `FloatLiteral` initializer from an `IntLiteral`).
    IntToFloatLiteral,
    /// `#pop<float_to_int_literal>`: a `!pop.float_literal` truncated toward
    /// zero to a `!pop.int_literal` (upstream's
    /// `FloatLiteral.__int_literal__`).
    FloatToIntLiteral,
    /// `#pop<float_literal_isa<nan>>`: whether a `!pop.float_literal` is
    /// not a number, an `i1`.
    FloatLiteralIsNan,
    /// `#pop<float_literal_isa<neg_zero>>`: whether a `!pop.float_literal`
    /// is negative zero, an `i1`.
    FloatLiteralIsNegZero,
    /// `#pop<float_literal_isa<normal>>`: whether a `!pop.float_literal` is
    /// no special value (neither not a number, an infinity, nor negative
    /// zero), an `i1`.
    FloatLiteralIsNormal,
    /// `pop.cmp` at `#kgen.cmp_pred<eq>`: the `bool` `!kgen.simd` mask of
    /// two storages of one `!kgen.simd` type, compared lane by lane
    /// (upstream's `SIMD.eq` body). Its operands are ordered, as equality
    /// commutes.
    SimdCmpEq,
    /// `pop.cmp` at `#kgen.cmp_pred<ne>`, [`Self::SimdCmpEq`]'s
    /// complement, which a NaN lane satisfies; operands ordered.
    SimdCmpNe,
    /// `pop.cmp` at `#kgen.cmp_pred<lt>`, the lanes' own ordering; `gt`
    /// is this predicate over the swapped operands.
    SimdCmpLt,
    /// `pop.cmp` at `#kgen.cmp_pred<le>`; `ge` is this predicate over the
    /// swapped operands.
    SimdCmpLe,
    /// `pop.simd.reduce_and` over a `bool` mask: whether every lane is set,
    /// a `Bool` as `Bool(mlir_value=…)` reads its `!kgen.scalar<bool>`
    /// result (R1955).
    SimdReduceAnd,
    /// `pop.simd.reduce_or` over a `bool` mask: whether some lane is set.
    /// Over one lane it is [`Self::SimdReduceAnd`], its canonical spelling.
    SimdReduceOr,
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
            Self::CastToBuiltin => "cast_to_builtin",
            Self::CastFromBuiltin => "cast_from_builtin",
            Self::DataToStr => "data_to_str",
            Self::StringConcat => "string_concat",
            Self::StringSize => "string_size",
            Self::StringAddress => "string_address",
            Self::SimdSplat => "simd_splat",
            Self::Cast => "cast",
            Self::TargetGetField => "target_get_field",
            Self::TargetHasFeature => "target_has_feature",
            Self::GetEnv => "get_env",
            Self::DtypeToUi8 => "dtype_to_ui8",
            Self::StoreToMem => "store_to_mem",
            Self::IntLiteralConvert => "int_literal_convert",
            Self::FloatLiteralConvert => "float_literal_convert",
            Self::IntToFloatLiteral => "int_to_float_literal",
            Self::FloatToIntLiteral => "float_to_int_literal",
            Self::FloatLiteralIsNan => "float_literal_isa_nan",
            Self::FloatLiteralIsNegZero => "float_literal_isa_neg_zero",
            Self::FloatLiteralIsNormal => "float_literal_isa_normal",
            Self::SimdCmpEq => "simd_cmp_eq",
            Self::SimdCmpNe => "simd_cmp_ne",
            Self::SimdCmpLt => "simd_cmp_lt",
            Self::SimdCmpLe => "simd_cmp_le",
            Self::SimdReduceAnd => "simd_reduce_and",
            Self::SimdReduceOr => "simd_reduce_or",
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

    /// Whether the pin keeps this operator's application unfolded over
    /// constants: its `SIMD` dunder (`__floordiv__`, `__mod__`, `__pow__`)
    /// is not `@always_inline("builtin")`, so a type argument or a `where`
    /// clause holds the call. Every other atom is a builtin `pop` op, which
    /// the pin simplifies.
    pub const fn is_unfolded_call(self) -> bool {
        matches!(self, Self::FloorDiv | Self::Mod | Self::Pow)
    }

    /// Whether evaluation can fail on well-typed operands.
    pub const fn is_partial(self) -> bool {
        matches!(
            self,
            Self::Div | Self::FloorDiv | Self::Mod | Self::Pow | Self::Shl | Self::Shr
        )
    }

    const ALL: [Self; 46] = [
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
        Self::CastToBuiltin,
        Self::CastFromBuiltin,
        Self::DataToStr,
        Self::StringConcat,
        Self::StringSize,
        Self::StringAddress,
        Self::SimdSplat,
        Self::Cast,
        Self::TargetGetField,
        Self::TargetHasFeature,
        Self::GetEnv,
        Self::DtypeToUi8,
        Self::StoreToMem,
        Self::IntLiteralConvert,
        Self::FloatLiteralConvert,
        Self::IntToFloatLiteral,
        Self::FloatToIntLiteral,
        Self::FloatLiteralIsNan,
        Self::FloatLiteralIsNegZero,
        Self::FloatLiteralIsNormal,
        Self::SimdCmpEq,
        Self::SimdCmpNe,
        Self::SimdCmpLt,
        Self::SimdCmpLe,
        Self::SimdReduceAnd,
        Self::SimdReduceOr,
    ];

    const fn accepts_arity(self, arity: usize) -> bool {
        match self {
            Self::Neg
            | Self::CastToBuiltin
            | Self::CastFromBuiltin
            | Self::StringSize
            | Self::StringAddress
            | Self::DtypeToUi8
            | Self::StoreToMem
            | Self::IntToFloatLiteral
            | Self::FloatToIntLiteral
            | Self::FloatLiteralIsNan
            | Self::FloatLiteralIsNegZero
            | Self::FloatLiteralIsNormal
            | Self::SimdReduceAnd
            | Self::SimdReduceOr => arity == 1,
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
            Self::Neg
            | Self::Cond
            | Self::CastToBuiltin
            | Self::CastFromBuiltin
            | Self::DataToStr
            | Self::StringConcat
            | Self::StringSize
            | Self::StringAddress
            | Self::SimdSplat
            | Self::Cast
            | Self::TargetGetField
            | Self::TargetHasFeature
            | Self::GetEnv
            | Self::DtypeToUi8
            | Self::StoreToMem
            | Self::IntLiteralConvert
            | Self::FloatLiteralConvert
            | Self::IntToFloatLiteral
            | Self::FloatToIntLiteral
            | Self::FloatLiteralIsNan
            | Self::FloatLiteralIsNegZero
            | Self::FloatLiteralIsNormal
            | Self::SimdCmpEq
            | Self::SimdCmpNe
            | Self::SimdCmpLt
            | Self::SimdCmpLe
            | Self::SimdReduceAnd
            | Self::SimdReduceOr => return None,
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
/// different parameters and a `$` clone shares its template's. An overloaded
/// module-level `def` is owned by the signature-qualified symbol it lowers to
/// (`tally$ov$…`), so two overloads of one name never share a binder.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ParamId {
    pub owner: Arc<str>,
    pub slot: usize,
}

impl serde::Serialize for ParamId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&*self.owner, self.slot).serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for ParamId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (owner, slot) = <(String, usize)>::deserialize(deserializer)?;
        Ok(Self::new(&owner, slot))
    }
}

impl ParamId {
    pub fn new(owner: &str, slot: usize) -> Self {
        Self {
            owner: Arc::from(owner),
            slot,
        }
    }

    /// Whether this is a `comptime for` variable's binder, whose owner
    /// [`COMPTIME_FOR_OWNER`] starts.
    pub fn is_comptime_for_binder(&self) -> bool {
        self.owner.starts_with(COMPTIME_FOR_OWNER)
    }
}

/// The prefix of a `comptime for` binder's owner, which the loop's iterable
/// completes.
pub const COMPTIME_FOR_OWNER: &str = "$comptime_for@";

/// The owner spelling every [`ParamRef::unbound`] reference starts with.
pub const UNBOUND_BINDER_PREFIX: &str = "$unbound:";

/// A reference to a declared parameter, with its source spelling attached.
#[derive(Debug, Clone)]
pub struct ParamRef {
    pub id: ParamId,
    pub name: Arc<str>,
}

impl ParamRef {
    /// A reference no declaration in scope binds, identified by its spelling
    /// alone. It equals only another unbound reference of the same spelling
    /// and is bound by no environment.
    pub fn unbound(name: &str) -> Self {
        Self {
            id: ParamId::new(&format!("{UNBOUND_BINDER_PREFIX}{name}"), 0),
            name: Arc::from(name),
        }
    }

    pub fn is_unbound(&self) -> bool {
        self.id.owner.starts_with(UNBOUND_BINDER_PREFIX)
    }
}

impl serde::Serialize for ParamRef {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (&self.id, &*self.name).serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for ParamRef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (id, name) = <(ParamId, String)>::deserialize(deserializer)?;
        Ok(Self {
            id,
            name: Arc::from(name),
        })
    }
}

impl PartialEq for ParamRef {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for ParamRef {}

impl fmt::Display for ParamRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

impl Hash for ParamRef {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// The meta-type of an expression: a runtime value domain, a type, or a
/// compile-time aggregate. `Type` is not a runtime
/// [`Ty`], so it has no place in that lattice.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum MetaTy {
    Value(Ty),
    Type,
    Tuple(Vec<Self>),
    List(Vec<Self>),
    Dict(Vec<(Self, Self)>),
    Set(Vec<Self>),
    /// An ordered list of one element meta-type: a variadic pack's binder.
    ParamList(Box<Self>),
}

impl MetaTy {
    /// The meta-type of a value of type `ty`. A raw parameter list
    /// (`KGENParamList[T]`) is the same ordered list a `*values: T` binder
    /// declares, so it has one encoding; a value of a trait type
    /// (`KGENParamList[AnyType]`'s element, or of a trait-valued binder's
    /// `Tr`) is a type.
    pub fn value(ty: Ty) -> Self {
        match ty.kind() {
            TyKind::ParamList(element) => Self::ParamList(Box::new(Self::value(element.clone()))),
            TyKind::Trait(_) => Self::Type,
            TyKind::Dependent(dependent)
                if dependent
                    .expr()
                    .meta()
                    .as_value()
                    .is_some_and(|meta| matches!(meta.kind(), TyKind::AnyTrait(_))) =>
            {
                Self::Type
            }
            _ => Self::Value(ty),
        }
    }

    /// The type a meta-type's values have, when it names one: a runtime
    /// value domain, or the raw parameter list (`KGENParamList[T]`) of one.
    pub fn value_ty(&self) -> Option<Ty> {
        match self {
            Self::Value(ty) => Some(ty.clone()),
            Self::ParamList(element) => element.value_ty().map(|ty| TyKind::ParamList(ty).into()),
            _ => None,
        }
    }

    pub fn int() -> Self {
        Self::value(Ty::int())
    }

    /// Whether this trait metatype converts to `declared`, another one: a
    /// trait-valued binder forwards to one whose bound its own refines
    /// (`g[Tr=Tr2]` over `Tr2: type_of(Copyable)`), which the checker
    /// proved.
    pub fn is_trait_metatype_of(&self, declared: &Self) -> bool {
        [self, declared].iter().all(|meta| {
            meta.as_value()
                .is_some_and(|ty| matches!(ty.kind(), TyKind::AnyTrait(_)))
        })
    }

    pub fn bool() -> Self {
        Self::value(Ty::bool())
    }

    /// Whether this generator type converts to `declared`, another one
    /// ([`crate::types::generator_converts`]), as a generator argument
    /// binds a generator-typed parameter.
    pub fn is_generator_of(&self, declared: &Self) -> bool {
        matches!(
            (self.as_value(), declared.as_value()),
            (Some(actual), Some(expected)) if crate::types::generator_converts(actual, expected)
        )
    }

    /// The runtime value domain, when this is one.
    pub const fn as_value(&self) -> Option<&Ty> {
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
            CtValue::UInt(_) => Self::value(Ty::uint()),
            CtValue::Index(_) => Self::value(Ty::index()),
            CtValue::Float(_) => Self::value(Ty::float64()),
            CtValue::IntLiteral(value) => Self::value(Ty::int_literal_of(value)),
            CtValue::FloatLiteral(value) => Self::value(Ty::float_literal_of(value)),
            CtValue::Bool(_) => Self::bool(),
            CtValue::None => Self::value(Ty::none()),
            CtValue::Str(text) => Self::value(Ty::string_literal_of(text)),
            CtValue::Tuple(values) => Self::Tuple(all(values)),
            CtValue::List(values) => Self::List(all(values)),
            CtValue::Dict { entries, .. } => Self::Dict(
                entries
                    .iter()
                    .map(|(key, value)| (Self::of_value(key), Self::of_value(value)))
                    .collect(),
            ),
            CtValue::Set { elements, .. } => Self::Set(all(elements)),
            CtValue::Dtype(_) => Self::value(Ty::kgen_dtype()),
            CtValue::Simd { dtype, lanes } => {
                Self::value(crate::types::canonical_simd_ty(*dtype, lanes.len() as i64))
            }
            CtValue::Struct { name, .. } => {
                Self::value(TyKind::Struct(name.clone(), Vec::new()).into())
            }
            CtValue::Variant { alternatives, .. } => {
                Self::value(TyKind::Variant(alternatives.clone()).into())
            }
            CtValue::Pointer { .. } | CtValue::StoreToMem(_) => Self::value(Ty::infer()),
            CtValue::Type(_) => Self::Type,
            CtValue::Trait(traits) => Self::value(TyKind::AnyTrait(traits.clone()).into()),
            CtValue::KgenString(_) => Self::value(TyKind::KgenString.into()),
            CtValue::PopIntLiteral(_) => Self::value(TyKind::PopIntLiteral.into()),
            CtValue::PopFloatLiteral(_) => Self::value(TyKind::PopFloatLiteral.into()),
            CtValue::MlirAttr(_) => Self::value(TyKind::KgenDeferred.into()),
            CtValue::KgenTarget(_) => Self::value(TyKind::KgenTarget.into()),
            CtValue::PopArray { element, elements } => Self::value(
                TyKind::PopArray {
                    size: crate::types::ArraySize::Known(elements.len() as u64),
                    element: element.clone(),
                }
                .into(),
            ),
            CtValue::Generator(generator) => Self::value(generator.ty()),
            CtValue::Expr(expr) => expr.meta().clone(),
            // A deferred slot has no value yet, so no domain either.
            CtValue::Deferred(_) | CtValue::Marker(_) | CtValue::RemovedArg => {
                Self::value(Ty::infer())
            }
        }
    }

    /// The binder meta-type of a variadic type pack.
    pub fn type_list() -> Self {
        Self::ParamList(Box::new(Self::Type))
    }

    pub fn is_integer(&self) -> bool {
        self.as_value().is_some_and(Ty::is_int_literal)
            || self
                .as_value()
                .is_some_and(|ty| ty.kind().is_int() || ty.kind().is_uint())
    }

    /// A constant aggregate bound to a parameter list: every member has the
    /// list's element meta-type, a frozen struct by its bare name.
    fn is_list_of(&self, declared: &Self) -> bool {
        matches!(
            (self, declared),
            (Self::Tuple(members), Self::ParamList(element))
                if members.iter().all(|member| {
                    member == &**element || member.is_unresolved_struct(element)
                })
        )
    }

    fn is_literal(&self) -> bool {
        self.as_value().is_some_and(Ty::is_numeric_literal)
    }

    /// This meta-type with the type binders `subst` solves replaced.
    #[must_use]
    pub fn substitute(&self, subst: &crate::types::TySubst) -> Self {
        if subst.is_empty() {
            return self.clone();
        }
        match self {
            Self::Value(ty) => Self::value(crate::types::substitute(ty, subst)),
            Self::ParamList(element) => Self::ParamList(Box::new(element.substitute(subst))),
            Self::Tuple(elements) => {
                Self::Tuple(elements.iter().map(|e| e.substitute(subst)).collect())
            }
            Self::List(elements) => {
                Self::List(elements.iter().map(|e| e.substitute(subst)).collect())
            }
            Self::Set(elements) => {
                Self::Set(elements.iter().map(|e| e.substitute(subst)).collect())
            }
            Self::Dict(entries) => Self::Dict(
                entries
                    .iter()
                    .map(|(key, value)| (key.substitute(subst), value.substitute(subst)))
                    .collect(),
            ),
            Self::Type => self.clone(),
        }
    }

    /// Whether a value domain in this meta-type names a type parameter.
    pub fn has_free_parameters(&self) -> bool {
        match self {
            Self::Value(ty) => matches!(*ty.kind(), TyKind::Param { .. }),
            Self::ParamList(element) => element.has_free_parameters(),
            Self::Tuple(elements) | Self::List(elements) | Self::Set(elements) => {
                elements.iter().any(Self::has_free_parameters)
            }
            Self::Dict(entries) => entries
                .iter()
                .any(|(key, value)| key.has_free_parameters() || value.has_free_parameters()),
            Self::Type => false,
        }
    }

    /// A frozen struct constant is typed by bare name, while its parameter
    /// declares the resolved instance type.
    pub fn is_unresolved_struct(&self, declared: &Self) -> bool {
        matches!(
            (self.as_value().map(Ty::kind), declared.as_value().map(Ty::kind)),
            (Some(TyKind::Struct(found, _)), Some(TyKind::Struct(expected, _)))
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
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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
    /// `_get_type_index[T, *Ts]()`: the first position of a Type-meta-type
    /// element in the pack, else `-1` (upstream's `_InvalidTypeIndex`).
    IndexOf(ParamExpr),
}

/// The reflection queries of a symbolic subject.
///
/// Each carries the meta-type of its answer: `is_struct()` a `Bool`;
/// `field_count()` and `field_index[name]()` an `Int`; `field_types()` a type
/// list, whose element under an index is a dependent type; `field_names()` a
/// list of string literals; `field[name].T` a type; `name()` and
/// `base_name()` the `!kgen.string` of upstream's `#kgen.get_type_name` and
/// `#kgen.get_base_type_name`, which the handle's method wraps in a
/// `StaticString`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ReflectQuery {
    IsStruct,
    FieldCount,
    FieldNames,
    FieldTypes,
    FieldIndex(String),
    FieldNamed(String),
    TypeName(TypeNameForm),
}

/// Which spelling of a type a type-name query asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TypeNameForm {
    /// `name[qualified_builtins=…]()`: the module-qualified name with its
    /// parameters, the library's most common types (`SIMD`, `Bool`,
    /// `String`, `List`) bare unless `qualified_builtins`.
    Name { qualified_builtins: bool },
    /// `base_name()`: the unqualified name of the type's base type.
    Base,
    /// `_unqualified_type_name[T]()`: the name with its parameters, each
    /// struct unqualified by its module.
    Unqualified,
}

/// How a type-name query's answerer spells a closed type: the caller's,
/// which knows each struct's declaring module.
pub type TypeNamer<'a> = dyn Fn(&Ty, TypeNameForm) -> String + 'a;

impl ReflectQuery {
    /// The inverse of the `Display` spelling, which MIR text carries.
    pub fn from_spelling(spelling: &str) -> Option<Self> {
        let bracketed = |prefix: &str, suffix: &str| {
            spelling
                .strip_prefix(prefix)?
                .strip_suffix(suffix)?
                .strip_prefix('"')?
                .strip_suffix('"')
                .map(str::to_string)
        };
        match spelling {
            "is_struct()" => Some(Self::IsStruct),
            "field_count()" => Some(Self::FieldCount),
            "field_names()" => Some(Self::FieldNames),
            "field_types()" => Some(Self::FieldTypes),
            "name()" => Some(Self::TypeName(TypeNameForm::Name {
                qualified_builtins: false,
            })),
            "name[qualified_builtins=True]()" => Some(Self::TypeName(TypeNameForm::Name {
                qualified_builtins: true,
            })),
            "base_name()" => Some(Self::TypeName(TypeNameForm::Base)),
            "unqualified_name()" => Some(Self::TypeName(TypeNameForm::Unqualified)),
            _ => bracketed("field_index[", "]()")
                .map(Self::FieldIndex)
                .or_else(|| bracketed("field[", "].T").map(Self::FieldNamed)),
        }
    }

    /// The meta-type of the query's answer.
    pub fn meta(&self) -> MetaTy {
        match self {
            Self::IsStruct => MetaTy::bool(),
            Self::FieldCount | Self::FieldIndex(_) => MetaTy::int(),
            Self::FieldTypes => MetaTy::type_list(),
            Self::FieldNames => {
                MetaTy::ParamList(Box::new(MetaTy::value(TyKind::KgenString.into())))
            }
            Self::FieldNamed(_) => MetaTy::Type,
            Self::TypeName(_) => MetaTy::value(TyKind::KgenString.into()),
        }
    }

    /// The answer for a closed `subject`. `fields` are a struct's fields,
    /// their types at the subject's arguments; any other type passes `None`
    /// and answers `is_struct()` alone, `True` (only an MLIR primitive, which
    /// Mojito never spells, answers `False` at the pin), and the type-name
    /// queries, which every type answers through `type_name`.
    pub fn answer(
        &self,
        subject: &Ty,
        fields: Option<&[(String, Ty)]>,
        type_name: &TypeNamer<'_>,
    ) -> Result<CtValue, ReflectError> {
        let Some(fields) = fields else {
            return match self {
                Self::IsStruct => Ok(CtValue::Bool(true)),
                Self::TypeName(form) => Ok(CtValue::KgenString(type_name(subject, *form))),
                query => Err(ReflectError::NotStruct {
                    subject: subject.to_string(),
                    query: query.to_string(),
                }),
            };
        };
        let position = |field: &str| {
            fields
                .iter()
                .position(|(declared, _)| declared == field)
                .ok_or_else(|| ReflectError::NoField {
                    owner: match subject.kind() {
                        TyKind::Struct(name, _) => name.clone(),
                        other => other.to_string(),
                    },
                    field: field.to_string(),
                })
        };
        let field_ty = |(_, ty): &(String, Ty)| CtValue::Type(ty.clone());
        Ok(match self {
            Self::IsStruct => CtValue::Bool(true),
            Self::FieldCount => CtValue::Int(fields.len() as i64),
            Self::FieldNames => CtValue::Tuple(
                fields
                    .iter()
                    .map(|(declared, _)| CtValue::Str(declared.clone()))
                    .collect(),
            ),
            Self::FieldTypes => CtValue::Tuple(fields.iter().map(field_ty).collect()),
            Self::FieldIndex(field) => CtValue::Int(position(field)? as i64),
            Self::FieldNamed(field) => field_ty(&fields[position(field)?]),
            Self::TypeName(form) => CtValue::KgenString(type_name(subject, *form)),
        })
    }
}

impl fmt::Display for ReflectQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IsStruct => write!(f, "is_struct()"),
            Self::FieldCount => write!(f, "field_count()"),
            Self::FieldNames => write!(f, "field_names()"),
            Self::FieldTypes => write!(f, "field_types()"),
            Self::FieldIndex(name) => write!(f, "field_index[{name:?}]()"),
            Self::FieldNamed(name) => write!(f, "field[{name:?}].T"),
            Self::TypeName(TypeNameForm::Name {
                qualified_builtins: false,
            }) => write!(f, "name()"),
            Self::TypeName(TypeNameForm::Name {
                qualified_builtins: true,
            }) => write!(f, "name[qualified_builtins=True]()"),
            Self::TypeName(TypeNameForm::Base) => write!(f, "base_name()"),
            Self::TypeName(TypeNameForm::Unqualified) => write!(f, "unqualified_name()"),
        }
    }
}

/// What answers a reflection query over a subject at an instance:
/// `Ok(None)` leaves the query as it is.
pub type ReflectOracle<'a> =
    dyn FnMut(&ParamExpr, &ReflectQuery) -> Result<Option<CtValue>, ParamError> + 'a;

/// What answers a `conforms_to` query over a subject at an instance:
/// whether the subject conforms to the named trait, or `None` to leave the
/// query as it is.
pub type ConformsOracle<'a> = dyn FnMut(&ParamExpr, &str) -> Option<bool> + 'a;

/// What answers a type member read over a subject at an instance: the
/// subject, the member's name, and a parameterized member's bound
/// arguments, to its value, or `None` to leave the read as it is.
pub type TypeMemberOracle<'a> =
    dyn FnMut(&ParamExpr, &str, &[ParamExpr]) -> Result<Option<CtValue>, ParamError> + 'a;

/// What answers a type member read with the member's expression.
///
/// [`ParamContext::expand_type_members`] passes the subject, the member's
/// name, and a parameterized member's bound arguments; the answer is the
/// member's expression at the instance, or `None` to leave the read.
pub type TypeMemberBodyOracle<'a> =
    dyn FnMut(&ParamExpr, &str, &[ParamExpr]) -> Result<Option<ParamExpr>, ParamError> + 'a;

/// What answers a compile-time application for
/// [`ParamContext::answer_applications`]: the applied function and its
/// arguments, to its value, or `None` to leave the application as it is.
pub type ApplicationOracle<'a> =
    dyn FnMut(&str, &[ParamExpr]) -> Result<Option<CtValue>, ParamError> + 'a;

/// Why a reflection query over a closed subject has no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReflectError {
    /// `field_index[name]()` or `field[name]` of a field the struct lacks.
    NoField { owner: String, field: String },
    /// A field query of a type that is not a struct.
    NotStruct { subject: String, query: String },
}

impl fmt::Display for ReflectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoField { owner, field } => {
                write!(f, "struct '{owner}' has no field named '{field}'")
            }
            Self::NotStruct { subject, query } => {
                write!(f, "reflect[{subject}].{query} requires a struct type")
            }
        }
    }
}

/// The two reserved hole states: an explicitly unknown contextual value, and
/// one particular inference hole.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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
    types: HashMap<ParamId, Ty>,
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
        bindings.bind_named_values(context, values);
        bindings
    }

    /// Add a declaration's own name → value map beneath the identity
    /// entries, as [`Self::from_named_values`] builds it.
    pub fn bind_named_values<'a>(
        &mut self,
        context: &ParamContext,
        values: impl IntoIterator<Item = (&'a String, &'a CtValue)>,
    ) {
        for (name, value) in values {
            if let Ok(value) = context.constant(value.clone()) {
                self.bind_name(name, value);
            }
        }
    }

    pub fn bind(&mut self, id: ParamId, value: ParamExpr) {
        self.by_id.insert(id, value);
    }

    /// Bind each value parameter of `decls` to its positional argument, as
    /// its residual expression or its constant. A type, origin, or deferred
    /// argument binds nothing.
    pub fn bind_value_arguments(
        &mut self,
        context: &ParamContext,
        decls: &[crate::types::ParamDecl],
        arguments: &[crate::types::TyArg],
    ) {
        for (decl, argument) in decls.iter().zip(arguments) {
            if let (crate::types::ParamDecl::Value { id, .. }, crate::types::TyArg::Val(value)) =
                (decl, argument)
                && let Some(value) = match value {
                    CtValue::Expr(expr) => Some(expr.clone()),
                    value => context.constant(value.clone()).ok(),
                }
            {
                self.bind(id.clone(), value);
            }
        }
    }

    pub fn bind_name(&mut self, name: &str, value: ParamExpr) {
        self.by_name
            .insert(name.trim_start_matches('*').to_string(), value);
    }

    pub fn bind_type(&mut self, id: ParamId, ty: Ty) {
        self.types.insert(id, ty);
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

    /// Drop the name entries a nested signature's binders shadow.
    pub fn mask<'a>(&mut self, names: impl IntoIterator<Item = &'a str>) {
        for name in names {
            self.by_name.remove(name.trim_start_matches('*'));
        }
    }

    /// Drop the type entries of a nested signature's own binders.
    pub fn mask_types<'a>(&mut self, ids: impl IntoIterator<Item = &'a ParamId>) {
        for id in ids {
            self.types.remove(id);
        }
    }

    pub const fn types(&self) -> &HashMap<ParamId, Ty> {
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
    /// literal power, a negative literal exponent.
    Arithmetic(String),
    /// An operator the compile-time surface does not define.
    Unsupported(String),
    /// A constant was required and a parameter, hole, or query remains.
    NotConstant(String),
    /// The canonical polynomial would exceed [`MAX_MONOMIALS`].
    Budget {
        limit: usize,
    },
    /// A reflection query over a closed subject that has no answer.
    Reflect(ReflectError),
    /// A builtin application whose callee's `comptime assert` fails at its
    /// arguments ([`builtin_application_value`]).
    Constraint(String),
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
            Self::Reflect(error) => write!(f, "{error}"),
            Self::Constraint(text) => write!(f, "constraint failed: {text}"),
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
    pub types_interned: u64,
    pub type_hits: u64,
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
    /// The context's types, by digest, as MLIR's context uniques types
    /// beside attributes.
    types: Mutex<HashMap<u64, Vec<Ty>>>,
    binders: Mutex<HashMap<ParamId, BinderInfo>>,
    interned: AtomicU64,
    hits: AtomicU64,
    types_interned: AtomicU64,
    type_hits: AtomicU64,
    constant_folds: AtomicU64,
    replacements: AtomicU64,
}

impl ContextState {
    /// The interned twin of `ty`, or `None` after recording `ty` as the
    /// node of its class. The table is locked for exactly this lookup.
    fn lookup_or_insert_ty(&self, ty: &Ty) -> Option<Ty> {
        let mut table = self
            .types
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bucket = table.entry(ty.digest()).or_default();
        let found = bucket.iter().find(|candidate| *candidate == ty).cloned();
        if found.is_none() {
            bucket.push(ty.clone());
        }
        drop(table);
        found
    }

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

impl Node {
    /// The node of `meta` and `kind`, with its structural fingerprint.
    fn new(meta: MetaTy, kind: ParamKind) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        meta.hash(&mut hasher);
        kind.hash(&mut hasher);
        Self {
            fingerprint: hasher.finish(),
            meta,
            kind,
        }
    }
}

/// The integer domains the canonical polynomial ranges over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntDomain {
    Int,
    UInt,
    /// `Int`'s `!kgen.scalar<index>` storage, its `_mlir_value`.
    IntStorage,
    /// `UInt`'s `!kgen.scalar<ui64>` storage.
    UIntStorage,
    /// The MLIR builtin `index` of the 64-bit host target.
    Index,
    Literal,
}

impl IntDomain {
    /// The domain of a value of type `meta`. A scalar's `!kgen.scalar`
    /// storage is a domain of its own, which `struct.extract` and
    /// `struct.create` map to and from the scalar's.
    fn of(meta: &MetaTy) -> Option<Self> {
        let ty = meta.as_value()?;
        let storage = matches!(ty.kind(), TyKind::KgenSimd { .. });
        let value = crate::types::storage_value_ty(ty);
        match value.kind() {
            kind if kind.is_int() => Some(if storage { Self::IntStorage } else { Self::Int }),
            kind if kind.is_uint() => Some(if storage {
                Self::UIntStorage
            } else {
                Self::UInt
            }),
            TyKind::Index if !storage => Some(Self::Index),
            kind if kind.is_int_literal() && !storage => Some(Self::Literal),
            _ => None,
        }
    }

    /// The type of a value of a machine domain. The literal domain has no
    /// one type: each of its values is `IntLiteral[v]` over its own value
    /// ([`ParamContext::literal_node_meta`]).
    fn ty(self) -> Option<Ty> {
        match self {
            Self::Int => Some(Ty::int()),
            Self::UInt => Some(Ty::uint()),
            Self::IntStorage => Some(crate::types::storage_ty(&Ty::int())),
            Self::UIntStorage => Some(crate::types::storage_ty(&Ty::uint())),
            Self::Index => Some(Ty::index()),
            Self::Literal => None,
        }
    }

    /// The domain's name in a diagnostic.
    fn name(self) -> String {
        self.ty().map_or_else(
            || crate::types::INT_LITERAL_TYPE_NAME.to_string(),
            |ty| ty.to_string(),
        )
    }

    /// The meta-type of a value of a machine domain.
    fn meta(self) -> MetaTy {
        self.ty().map_or_else(MetaTy::int, MetaTy::value)
    }

    /// The storage domain of a scalar domain, its `_mlir_value`'s.
    const fn storage(self) -> Option<Self> {
        match self {
            Self::Int => Some(Self::IntStorage),
            Self::UInt => Some(Self::UIntStorage),
            _ => None,
        }
    }

    /// Reduce a coefficient into the domain: machine domains wrap at 64 bits,
    /// the literal domain is exact.
    fn reduce(self, value: &IntLiteral) -> IntLiteral {
        match self {
            Self::Int | Self::IntStorage | Self::Index => value
                .wrapping_signed(64)
                .map_or_else(|| value.clone(), IntLiteral::from),
            Self::UInt | Self::UIntStorage => value
                .wrapping_unsigned(64)
                .map_or_else(|| value.clone(), IntLiteral::from),
            Self::Literal => value.clone(),
        }
    }

    fn constant(self, value: &IntLiteral) -> CtValue {
        match self {
            Self::Int | Self::IntStorage => {
                CtValue::Int(value.wrapping_signed(64).unwrap_or_default())
            }
            Self::UInt | Self::UIntStorage => {
                CtValue::UInt(value.wrapping_unsigned(64).unwrap_or_default())
            }
            Self::Index => CtValue::Index(value.wrapping_signed(64).unwrap_or_default()),
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
        Self::of_with(domain, expr, ZeroTerms::Drop)
    }

    fn of_with(domain: IntDomain, expr: &ParamExpr, zeros: ZeroTerms) -> Result<Self, ParamError> {
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
                    sum = sum.add(&Self::of_with(domain, operand, zeros)?);
                }
                Ok(sum)
            }
            ParamKind::Op {
                op: ParamOp::Mul,
                operands,
            } if IntDomain::of(expr.meta()).is_some() => Self::product(domain, operands, zeros),
            _ => {
                let mut atom = Self::zero(domain);
                atom.insert(vec![expr.clone()], &IntLiteral::from(1_i64));
                Ok(atom)
            }
        }
    }

    /// The product of `factors`. Keeping zero terms, the constant factors
    /// scale the product of the rest last, so a zero among them leaves a
    /// monomial that holds a partial atom at coefficient zero.
    fn product(
        domain: IntDomain,
        factors: &[ParamExpr],
        zeros: ZeroTerms,
    ) -> Result<Self, ParamError> {
        let mut product = Self::constant(domain, &IntLiteral::from(1_i64));
        let mut scale = IntLiteral::from(1_i64);
        for factor in factors {
            let factor = Self::of_with(domain, factor, zeros)?;
            match factor.as_constant().filter(|_| zeros == ZeroTerms::Keep) {
                Some(value) => scale = scale.mul(&value),
                None => product = product.mul(&factor)?,
            }
        }
        let mut scaled = Self::zero(domain);
        for (monomial, coefficient) in &product.terms {
            scaled.insert(monomial.clone(), &coefficient.mul(&scale));
        }
        Ok(scaled)
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

    /// The one atom this polynomial is, at coefficient one.
    fn as_atom(&self) -> Option<&ParamExpr> {
        let mut terms = self.terms.iter();
        match (terms.next(), terms.next()) {
            (Some((monomial, coefficient)), None)
                if monomial.len() == 1 && *coefficient == IntLiteral::from(1_i64) =>
            {
                monomial.first()
            }
            _ => None,
        }
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

/// Whether a product by a constant zero drops a monomial that holds a
/// partial atom ([`ParamContext::op`]) or keeps it as a cancellation left it
/// ([`ParamContext::rebuild`]).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ZeroTerms {
    Drop,
    Keep,
}

static CONTEXTS_CREATED: AtomicU64 = AtomicU64::new(0);
static NEXT_HOLE: AtomicU64 = AtomicU64::new(0);

/// Fold one primitive over concrete operands through the shared folder.
fn fold_primitive(op: ParamOp, operands: &[&CtValue]) -> Result<CtValue, ParamError> {
    match (op, operands) {
        (ParamOp::Neg, [operand]) => fold::fold_neg(operand),
        (ParamOp::CastToBuiltin, [operand]) => fold::fold_cast_to_builtin(operand),
        (ParamOp::CastFromBuiltin, [operand]) => fold::fold_cast_from_builtin(operand),
        (ParamOp::DataToStr, [string, rest]) => fold::fold_data_to_str(string, rest),
        (ParamOp::StringConcat, [left, right]) => fold::fold_string_concat(left, right),
        (ParamOp::StringSize, [string]) => fold::fold_string_size(string),
        (ParamOp::StringAddress, [string]) => fold::fold_string_address(string),
        (ParamOp::TargetGetField, [target, name]) => fold::fold_target_get_field(target, name),
        (ParamOp::TargetHasFeature, [target, name]) => fold::fold_target_has_feature(target, name),
        (ParamOp::GetEnv, [CtValue::Type(ty), name]) => fold::fold_get_env(ty, name),
        (ParamOp::DtypeToUi8, [dtype]) => fold::fold_dtype_to_ui8(dtype),
        (ParamOp::StoreToMem, [referent]) => Ok(CtValue::StoreToMem(Box::new((*referent).clone()))),
        (
            ParamOp::IntLiteralConvert | ParamOp::FloatLiteralConvert,
            [CtValue::Type(ty), literal],
        ) => fold::fold_literal_convert(op, ty, literal),
        (
            ParamOp::IntToFloatLiteral
            | ParamOp::FloatToIntLiteral
            | ParamOp::FloatLiteralIsNan
            | ParamOp::FloatLiteralIsNegZero
            | ParamOp::FloatLiteralIsNormal,
            [literal],
        ) => fold::fold_literal_attribute(op, literal),
        (ParamOp::SimdSplat | ParamOp::Cast, [CtValue::Type(ty), value]) => {
            fold::fold_lane_op(op, ty, value)
        }
        (
            ParamOp::SimdCmpEq | ParamOp::SimdCmpNe | ParamOp::SimdCmpLt | ParamOp::SimdCmpLe,
            [left, right],
        ) => fold::fold_lane_compare(op, left, right),
        (ParamOp::SimdReduceAnd | ParamOp::SimdReduceOr, [mask]) => {
            fold::fold_mask_reduction(op, mask)
        }
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
                if matches!(op, ParamOp::Eq | ParamOp::Lt | ParamOp::Le) {
                    fold::fold_comparison(infix, &left, right)
                } else {
                    fold::fold_infix(infix, &left, right)
                }
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

/// Whether `op` applies to `!pop.int_literal` or `!pop.float_literal`
/// operands, a `#pop<…_literal_bin>` or `…_cmp` attribute
/// ([`ParamContext::build_pop_literal`]).
fn is_pop_literal_operation(op: ParamOp, operands: &[ParamExpr]) -> bool {
    op.infix().is_some()
        && operands.first().is_some_and(|operand| {
            matches!(operand.meta(), MetaTy::Value(ty)
                if matches!(ty.kind(), TyKind::PopIntLiteral | TyKind::PopFloatLiteral))
        })
}

fn is_scalar_domain(ty: &Ty) -> bool {
    matches!(
        ty.kind(),
        TyKind::Index | TyKind::I1 | TyKind::Ui8 | TyKind::KgenDtype
    ) || crate::types::is_dtype(ty)
        || ty.kind().is_simd()
        || ty.is_numeric_literal()
        || ty.is_string_literal()
        || ty.is_bool()
}

/// Whether `ty` is a function type, whose compile-time value is the bound
/// function's symbol.
fn is_function(ty: &Ty) -> bool {
    matches!(ty.kind(), TyKind::Func(_) | TyKind::GenericFunc(_))
}

fn carries_transfers(ty: &Ty) -> bool {
    ty.callable()
        .is_some_and(|func| !func.transfers.0.is_empty())
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
        ParamOp::CastToBuiltin
        | ParamOp::CastFromBuiltin
        | ParamOp::DataToStr
        | ParamOp::StringConcat
        | ParamOp::StringSize
        | ParamOp::StringAddress
        | ParamOp::SimdSplat
        | ParamOp::Cast
        | ParamOp::TargetGetField
        | ParamOp::TargetHasFeature
        | ParamOp::GetEnv
        | ParamOp::DtypeToUi8
        | ParamOp::StoreToMem
        | ParamOp::IntLiteralConvert
        | ParamOp::FloatLiteralConvert
        | ParamOp::IntToFloatLiteral
        | ParamOp::FloatToIntLiteral
        | ParamOp::FloatLiteralIsNan
        | ParamOp::FloatLiteralIsNegZero
        | ParamOp::FloatLiteralIsNormal
        | ParamOp::SimdCmpEq
        | ParamOp::SimdCmpNe
        | ParamOp::SimdCmpLt
        | ParamOp::SimdCmpLe
        | ParamOp::SimdReduceAnd
        | ParamOp::SimdReduceOr => 14,
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
        ParamOp::CastToBuiltin => ".__mlir_index__()",
        ParamOp::CastFromBuiltin => "Int(mlir_value=)",
        ParamOp::DataToStr => "data_to_str",
        ParamOp::StringConcat => "pop.string_concat",
        ParamOp::StringSize => "pop.string.size",
        ParamOp::StringAddress => "pop.string.address",
        ParamOp::SimdSplat => "pop.simd.splat",
        ParamOp::Cast => "pop.cast",
        ParamOp::TargetGetField => "target_get_field",
        ParamOp::TargetHasFeature => "target_has_feature",
        ParamOp::GetEnv => "get_env",
        ParamOp::DtypeToUi8 => "pop.dtype.to_ui8",
        ParamOp::StoreToMem => "store_to_mem",
        ParamOp::IntLiteralConvert => "pop.int_literal_convert",
        ParamOp::FloatLiteralConvert => "pop.float_literal_convert",
        ParamOp::IntToFloatLiteral => "pop.int_to_float_literal",
        ParamOp::FloatToIntLiteral => "pop.float_to_int_literal",
        ParamOp::FloatLiteralIsNan => "pop.float_literal_isa<nan>",
        ParamOp::FloatLiteralIsNegZero => "pop.float_literal_isa<neg_zero>",
        ParamOp::FloatLiteralIsNormal => "pop.float_literal_isa<normal>",
        ParamOp::SimdCmpEq => "eq",
        ParamOp::SimdCmpNe => "ne",
        ParamOp::SimdCmpLt => "lt",
        ParamOp::SimdCmpLe => "le",
        ParamOp::SimdReduceAnd => "reduce_and",
        ParamOp::SimdReduceOr => "reduce_or",
    }
}

/// Write the application of an operator method over its operands, as the
/// pin shows it: `__add__` over `a` and `b` as `(a + b)`, its reflected
/// `__radd__` over `self` and `lhs` as `(lhs + self)`, and `__neg__` over
/// `a` as `-a`. `false`, writing nothing, for any other application.
fn write_operator_application(
    f: &mut fmt::Formatter<'_>,
    function: &str,
    values: &[&ParamExpr],
) -> Result<bool, fmt::Error> {
    let Some((_, method)) = function
        .split('$')
        .next()
        .and_then(|symbol| symbol.rsplit_once('.'))
    else {
        return Ok(false);
    };
    match (InfixOp::of_dunder(method), values) {
        (Some((op, reflected)), [first, second]) => {
            let (left, right) = if reflected {
                (second, first)
            } else {
                (first, second)
            };
            write!(f, "({left} {} {right})", op.symbol())?;
            return Ok(true);
        }
        (None, [operand]) => {
            for op in [PrefixOp::Neg, PrefixOp::Invert] {
                if op.dunder() == method {
                    write!(f, "{}", op.symbol())?;
                    write_expr(f, operand, precedence(ParamOp::Neg))?;
                    return Ok(true);
                }
            }
        }
        _ => {}
    }
    Ok(false)
}

fn write_expr(f: &mut fmt::Formatter<'_>, expr: &ParamExpr, parent: u8) -> fmt::Result {
    match expr.kind() {
        ParamKind::Constant(CtValue::Bool(value)) => {
            write!(f, "{}", if *value { "True" } else { "False" })
        }
        ParamKind::Constant(CtValue::StoreToMem(referent)) => write!(f, "{referent}"),
        ParamKind::Constant(value) => write!(f, "{value}"),
        ParamKind::DeclRef(reference) => write!(f, "{}", reference.name),
        ParamKind::IndexRef { depth, index } => write!(f, "${depth}.{index}"),
        // A `SIMDLength`'s lane count shows as the length, as the pin
        // spells a width (`SIMD[DType.int32, w]`).
        _ if let Some(length) = crate::types::simd_length_of_image(expr) => {
            write_expr(f, length, parent)
        }
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
                // An `Int`'s storage cast shows as `Int.__mlir_index__()`,
                // which reads it.
                (ParamOp::CastToBuiltin, [operand]) => {
                    write_expr(f, scalar_of_storage(operand).unwrap_or(operand), level)?;
                    write!(f, ".__mlir_index__()")?;
                }
                (ParamOp::CastFromBuiltin, [operand]) => {
                    write!(f, "Int(mlir_value=")?;
                    write_expr(f, operand, 0)?;
                    write!(f, ")._mlir_value")?;
                }
                (ParamOp::DtypeToUi8, [operand]) => {
                    write_expr(f, operand, level)?;
                    write!(f, "._as_ui8()")?;
                }
                // A reference shows as its referent, as the pin spells the
                // call that passes it (`bc(String("ab"))`).
                (ParamOp::StoreToMem, [operand]) => write_expr(f, operand, parent)?,
                // An `index` sum or product of casts shows the cast pulled
                // outward, as the pin does: `(k + 1).__mlir_index__()`.
                (ParamOp::Add | ParamOp::Mul, _) if let Some(pulled) = pulled_cast(expr) => {
                    write_expr(f, &pulled, precedence(ParamOp::CastToBuiltin))?;
                    write!(f, ".__mlir_index__()")?;
                }
                // `x ^ True` is the canonical negation.
                (ParamOp::BoolXor, [operand, constant]) if constant.as_bool() == Some(true) => {
                    write!(f, "not ")?;
                    write_expr(f, operand, level + 1)?;
                }
                // A lane compare shows as the `SIMD` member computing it
                // (`v.lt(w)`), a reduction as a call over its mask.
                (
                    ParamOp::SimdCmpEq
                    | ParamOp::SimdCmpNe
                    | ParamOp::SimdCmpLt
                    | ParamOp::SimdCmpLe,
                    [left, right],
                ) => {
                    write_expr(f, storage_owner_shown(left), level)?;
                    write!(f, ".{}(", source_operator(*op))?;
                    write_expr(f, storage_owner_shown(right), 0)?;
                    write!(f, ")")?;
                }
                (ParamOp::SimdReduceAnd | ParamOp::SimdReduceOr, [mask]) => {
                    write!(f, "{}(", source_operator(*op))?;
                    write_expr(f, storage_owner_shown(mask), 0)?;
                    write!(f, ")")?;
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
        ParamKind::ListGet { list, index } => write!(f, "{list}[{index}]"),
        ParamKind::Field { base, name } => write!(f, "{base}.{name}"),
        ParamKind::Reflect { subject, query } => write!(f, "reflect[{subject}].{query}"),
        ParamKind::TypeMember {
            subject,
            name,
            args,
        } => {
            write!(f, "{subject}.{name}")?;
            if !args.is_empty() {
                write!(f, "[")?;
                for (position, arg) in args.iter().enumerate() {
                    if position > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{arg}")?;
                }
                write!(f, "]")?;
            }
            Ok(())
        }
        ParamKind::PackQuery { pack, query } => match query {
            PackQuery::IndexOf(element) => {
                if let Some(binder) = pack.as_decl_ref() {
                    return write!(f, "_get_type_index[{element}, *{binder}]()");
                }
                write!(f, "_get_type_index[{element}, ")?;
                write_pack_elements(f, pack)?;
                write!(f, "]()")
            }
            PackQuery::Length => write!(f, "TypeList[{}]().length", PackList(pack)),
            PackQuery::Conforms(trait_name) => {
                write!(f, "conforms_to({}, {trait_name})", PackList(pack))
            }
            PackQuery::Predicate { predicate, all } => {
                let reduction = if *all { "all" } else { "any" };
                let predicate = match predicate {
                    PackPredicateRef::Trivial(kind) => {
                        crate::types::trivial_predicate_spelling(*kind)
                    }
                    PackPredicateRef::Alias(name) => name,
                };
                write!(
                    f,
                    "TypeList[{}]().{reduction}[{predicate}]()",
                    PackList(pack)
                )
            }
            PackQuery::Contains(element) => {
                write!(f, "TypeList[{}]().contains[{element}]()", PackList(pack))
            }
        },
        // Mojo-shaped: a type argument in brackets, a value in parentheses.
        // A construction spells its struct, or the generic instance it
        // constructs, its leading type argument (`Extent(2, 3)`, `G[2](n)`);
        // a function lifted from a constructor (`$comptime$S.__init__$c0`)
        // constructs nothing.
        ParamKind::Apply { function, args, .. } => {
            // A pack's visible binder is its zero-sized `ParameterList` or
            // `TypeList`, which its list beside it spells.
            let (mut types, values): (Vec<_>, Vec<_>) = args
                .iter()
                .filter(|arg| {
                    !arg.as_constant()
                        .is_some_and(crate::types::ParamDecl::is_pack_value)
                })
                .partition(|arg| matches!(arg.meta(), MetaTy::Type));
            if write_operator_application(f, function, &values)? {
                return Ok(());
            }
            let constructed = crate::symbol_ref::member_owner(function, "__init__")
                .filter(|_| !function.starts_with('$'));
            match constructed {
                Some(_) if !types.is_empty() => write!(f, "{}", types.remove(0))?,
                Some(owner) => write!(f, "{owner}")?,
                None => write!(f, "{function}")?,
            }
            if !types.is_empty() {
                write!(f, "[")?;
                write_operands(f, &types)?;
                write!(f, "]")?;
            }
            write!(f, "(")?;
            write_operands(f, &values)?;
            write!(f, ")")
        }
        // A scalar created from a storage atom shows as the `__init__` that
        // creates it, `Int(mlir_value=i)` over the `index` its storage casts.
        ParamKind::StructValue { spelling, .. }
            if let ParamKind::Apply { function, .. } = spelling.0.kind()
                && function == SCALAR_FROM_STORAGE_FUNCTION
                && let Some(storage) = scalar_storage_field(expr) =>
        {
            write!(f, "{}(mlir_value=", expr.meta())?;
            match storage.kind() {
                ParamKind::Op {
                    op: ParamOp::CastFromBuiltin,
                    operands,
                } => write_expr(f, &operands[0], 0)?,
                _ => write_expr(f, storage, 0)?,
            }
            write!(f, ")")
        }
        ParamKind::StructValue { spelling, .. } => write!(f, "{}", spelling.0),
        ParamKind::Hole { kind, token } => match kind {
            HoleKind::Unknown => write!(f, "?unknown{token}"),
            HoleKind::Unbound => write!(f, "?unbound{token}"),
        },
        ParamKind::ListTabulate { count, element } => {
            write!(f, "param_list.tabulate({count}, [idx] {element})")
        }
        ParamKind::ListConcat { lists } => {
            write!(f, "param_list.concat(")?;
            write_operands(f, &lists.iter().collect::<Vec<_>>())?;
            write!(f, ")")
        }
        ParamKind::BindParams { generator, args } => {
            write!(f, "{generator}[")?;
            for (position, arg) in args.iter().enumerate() {
                if position > 0 {
                    write!(f, ", ")?;
                }
                match arg {
                    Some(arg) => write!(f, "{arg}")?,
                    None => write!(f, "_")?,
                }
            }
            write!(f, "]")
        }
    }
}

/// The `Int` an `index` sum or product is the cast of, when every atom it
/// holds is a cast: what [`write_expr`] shows with the cast pulled outward.
fn pulled_cast(expr: &ParamExpr) -> Option<ParamExpr> {
    if IntDomain::of(expr.meta()) != Some(IntDomain::Index) {
        return None;
    }
    let casts_only = Polynomial::of(IntDomain::Index, expr)
        .ok()?
        .terms
        .keys()
        .flatten()
        .all(|atom| {
            matches!(
                atom.kind(),
                ParamKind::Op {
                    op: ParamOp::CastToBuiltin,
                    operands,
                } if IntDomain::of(operands[0].meta()) == Some(IntDomain::IntStorage)
            )
        });
    casts_only
        .then(|| ParamContext::detached().int_of_index(expr).ok())
        .flatten()
}

/// The `bool` `!kgen.simd` mask type of the lane compare `op` over
/// `operands`, two `!kgen.simd` storages, at their lane count: a closed
/// count where either operand's is, since a substituted field read keeps
/// its template's storage type.
fn lane_mask_ty(op: ParamOp, operands: &[ParamExpr]) -> Result<Ty, ParamError> {
    let widths = operands
        .iter()
        .map(|operand| {
            operand
                .meta()
                .as_value()
                .and_then(|ty| crate::types::kgen_simd_slots(ty.kind()))
                .map(|(_, width)| width)
                .ok_or_else(|| ParamError::TypeMismatch {
                    operation: op.name().to_string(),
                    expected: "a '!kgen.simd' storage".to_string(),
                    found: operand.meta().to_string(),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let width = widths
        .iter()
        .find(|width| matches!(width, crate::types::SimdWidth::Known(_)))
        .unwrap_or(&widths[0])
        .clone();
    Ok(TyKind::KgenSimd {
        dtype: crate::types::SimdDtype::Known(Dtype::Bool),
        width,
    }
    .into())
}

/// The ordering compare `x.lt(y)` (or `le`) and its storages, when `left`
/// and `right`, a `ne` compare's operands, are it and the all-`False`
/// mask: `SIMD.__bool__`'s `self != 0` over a mask.
fn truth_tested_order<'a>(
    left: &'a ParamExpr,
    right: &'a ParamExpr,
) -> Option<(ParamOp, &'a ParamExpr, &'a ParamExpr)> {
    let tested = if is_false_mask(right) {
        left
    } else if is_false_mask(left) {
        right
    } else {
        return None;
    };
    match tested.kind() {
        ParamKind::Op {
            op: compare @ (ParamOp::SimdCmpLt | ParamOp::SimdCmpLe),
            operands,
        } if let [x, y] = operands.as_slice() => {
            let ordering = if *compare == ParamOp::SimdCmpLt {
                ParamOp::Lt
            } else {
                ParamOp::Le
            };
            Some((ordering, x, y))
        }
        _ => None,
    }
}

/// Whether `expr` is the all-`False` mask: a closed mask no lane of which
/// is set, or the conversion of the literal `0` into one whose lane count
/// is still open (`Self(0)._mlir_value` in a `bool` `SIMD`'s body).
fn is_false_mask(expr: &ParamExpr) -> bool {
    match expr.kind() {
        ParamKind::Constant(CtValue::Bool(false)) => true,
        ParamKind::Constant(CtValue::Simd {
            dtype: Dtype::Bool,
            lanes,
        }) => lanes
            .iter()
            .all(|lane| matches!(lane, crate::ct::CtLane::Bool(false))),
        ParamKind::Op {
            op: ParamOp::IntLiteralConvert,
            operands,
        } => operands
            .get(1)
            .and_then(ParamExpr::as_constant)
            .is_some_and(
                |literal| matches!(literal, CtValue::PopIntLiteral(value) if value.is_zero()),
            ),
        _ => false,
    }
}

/// The value a lane compare's operand shows as: the `SIMD` whose
/// `_mlir_value` it reads, or the storage itself.
fn storage_owner_shown(expr: &ParamExpr) -> &ParamExpr {
    match expr.kind() {
        ParamKind::Field { base, name } if name == crate::ct::MLIR_VALUE_FIELD => base,
        _ => expr,
    }
}

/// The scalar whose `_mlir_value` `expr` reads, when it is a field read.
fn scalar_of_storage(expr: &ParamExpr) -> Option<&ParamExpr> {
    match expr.kind() {
        ParamKind::Field { base, name }
            if name == crate::ct::MLIR_VALUE_FIELD
                && matches!(
                    IntDomain::of(base.meta()),
                    Some(IntDomain::Int | IntDomain::UInt)
                ) =>
        {
            Some(base)
        }
        _ => None,
    }
}

/// The bindings a generator's application installs: each bound slot's
/// argument for its declaration, a type slot's as its type too.
fn generator_bindings(
    params: &[crate::types::ParamDecl],
    args: &[Option<ParamExpr>],
) -> ParamBindings {
    let mut bindings = ParamBindings::new();
    for (decl, arg) in params.iter().zip(args) {
        let Some(arg) = arg else {
            continue;
        };
        if matches!(decl, crate::types::ParamDecl::Type { .. }) && *arg.meta() == MetaTy::Type {
            bindings.bind_type(
                decl.id().clone(),
                crate::types::DependentType::resolve(arg.clone()),
            );
        }
        bindings.bind(decl.id().clone(), arg.clone());
    }
    bindings
}

/// A partial binding's open parameters, their declared types read at the
/// bound slots' arguments.
fn bound_generator_params(
    params: &[crate::types::ParamDecl],
    bindings: &ParamBindings,
) -> Vec<crate::types::ParamDecl> {
    params
        .iter()
        .map(|decl| {
            let mut decl = decl.clone();
            if let crate::types::ParamDecl::Value { ty, .. } = &mut decl {
                *ty = crate::types::substitute(ty, bindings.types());
            }
            decl
        })
        .collect()
}

/// The storage a scalar struct value holds, when `expr` is an `Int` or a
/// `UInt` created from its `_mlir_value` ([`ParamContext::struct_value`]).
fn scalar_storage_field(expr: &ParamExpr) -> Option<&ParamExpr> {
    let ParamKind::StructValue { fields, .. } = expr.kind() else {
        return None;
    };
    match (fields.as_slice(), IntDomain::of(expr.meta())) {
        ([(name, field)], Some(IntDomain::Int | IntDomain::UInt))
            if name == crate::ct::MLIR_VALUE_FIELD =>
        {
            Some(field)
        }
        _ => None,
    }
}

/// A struct value's fields with each value rebuilt by `rebuild`.
fn rebuilt_fields(
    fields: &[(String, ParamExpr)],
    mut rebuild: impl FnMut(&ParamExpr) -> Result<ParamExpr, ParamError>,
) -> Result<Vec<(String, ParamExpr)>, ParamError> {
    fields
        .iter()
        .map(|(name, value)| Ok((name.clone(), rebuild(value)?)))
        .collect()
}

/// The one value every field of a construction reads its own field of, in
/// order (`fields` are `x.a, x.b`): the construction is `x` itself.
fn own_fields_base(fields: &[(String, ParamExpr)]) -> Option<&ParamExpr> {
    let mut bases = fields.iter().map(|(name, value)| match value.kind() {
        ParamKind::Field { base, name: read } if read == name => Some(base),
        _ => None,
    });
    let first = bases.next()??;
    bases.all(|base| base == Some(first)).then_some(first)
}

/// A pack query's pack as upstream spells its list: a binder's
/// `Ts.values`, or a bound list's elements.
struct PackList<'a>(&'a ParamExpr);

impl fmt::Display for PackList<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.as_decl_ref() {
            Some(binder) => write!(f, "{binder}.values"),
            None => write_pack_elements(f, self.0),
        }
    }
}

/// A list operand's elements, comma-separated, or the operand itself when
/// it is not a constant list.
fn write_pack_elements(f: &mut fmt::Formatter<'_>, pack: &ParamExpr) -> fmt::Result {
    let Some(elements) = pack
        .as_constant()
        .and_then(CtValue::comptime_iteration_elements)
    else {
        return write!(f, "{pack}");
    };
    for (index, element) in elements.iter().enumerate() {
        if index > 0 {
            write!(f, ", ")?;
        }
        write!(f, "{element}")?;
    }
    Ok(())
}

fn write_operands(f: &mut fmt::Formatter<'_>, operands: &[&ParamExpr]) -> fmt::Result {
    for (index, operand) in operands.iter().enumerate() {
        if index > 0 {
            write!(f, ", ")?;
        }
        write!(f, "{operand}")?;
    }
    Ok(())
}

/// Answers the queries in each parameter expression a type embeds
/// (`ParamContext::answer_queries`).
struct Answering<'a> {
    context: &'a ParamContext,
    answer: &'a mut dyn FnMut(&ParamExpr) -> Result<Option<ParamExpr>, ParamError>,
}

impl crate::types::TyRewrite for Answering<'_> {
    fn expr(&mut self, expr: &ParamExpr) -> Result<ParamExpr, ParamError> {
        self.context.answer_queries(expr, self.answer)
    }
}

#[cfg(test)]
mod tests;
