//! The shared compile-time value model.
//!
//! `CtValue` is the one representation of a compile-time value across the
//! compiler: the [`comptime`](crate::comptime) elaboration pass builds and folds
//! them (`comptime` constants, `comptime if`/`for`, CTFE), and the
//! [`checker`](crate::checker) uses them for value-parameter arguments
//! (`FixedBuffer[8]`, `SIMD[DType.int32, 4]`). Consolidating the two former
//! representations (comptime's own value enum and the checker's former `CtVal`)
//! here keeps the two phases speaking the same language — a prerequisite for
//! type-valued compile-time members.
//!
//! Scalar values and recursively materializable tuples/lists/dicts/sets have a
//! runtime literal form; `Type`, `Reflected`, `Expr`, `Deferred`, and `Marker`
//! are compile-time-only.

use crate::origin::{Mutability, OriginParamId};
use crate::param_expr::{ParamExpr, ParamRef};
use crate::types::{
    SimdDtype, SimdWidth, Ty, TyArg, dict_elements, list_element, set_element, tuple_elements,
};
use mojito_ast::ast::{Expr, ExprKind, KwArg, ParamArg, Type};
use mojito_common::literal::{FloatLiteral, IntLiteral};
use mojito_common::token::Span;
use std::fmt;

/// One lane of a compile-time SIMD value: integer lanes hold the post-wrap
/// mathematical value (an unsigned lane is non-negative), float lanes their
/// IEEE bits (so equality is structural).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CtLane {
    Int(i128),
    Float(u64),
    Bool(bool),
}

impl CtLane {
    /// Wrap a compile-time scalar into one lane of `dtype`, or `None` when
    /// the value kind does not fit the lane (a float into an integer lane).
    pub fn from_value(value: &CtValue, dtype: mojito_ast::ast::Dtype) -> Option<Self> {
        use mojito_ast::ast::Dtype;
        match dtype {
            Dtype::Bool => match value {
                CtValue::Bool(value) => Some(Self::Bool(*value)),
                _ => None,
            },
            Dtype::Float16 | Dtype::Float32 | Dtype::Float64 => {
                let value = match value {
                    CtValue::Float(bits) => dtype.round_lane(f64::from_bits(*bits)),
                    CtValue::FloatLiteral(value) => dtype.float_literal_lane(value)?,
                    CtValue::IntLiteral(value) => dtype.round_lane(value.to_f64()?),
                    CtValue::Int(value) => dtype.round_lane(*value as f64),
                    CtValue::UInt(value) => dtype.round_lane(*value as f64),
                    _ => return None,
                };
                Some(Self::Float(value.to_bits()))
            }
            integer => {
                let wide: i128 = match value {
                    CtValue::Int(value) => i128::from(*value),
                    CtValue::UInt(value) => i128::from(*value),
                    CtValue::IntLiteral(value) => i128::from(value.wrapping_signed(64)?),
                    CtValue::Bool(value) => i128::from(*value),
                    _ => return None,
                };
                Some(Self::Int(wrap_lane(integer, wide)))
            }
        }
    }
}

/// Wrap an integer to the mathematical value of a `dtype` lane (two's
/// complement for signed lanes, modulo 2^bits for unsigned ones).
pub fn wrap_lane(dtype: mojito_ast::ast::Dtype, value: i128) -> i128 {
    use mojito_ast::ast::Dtype;
    match dtype {
        Dtype::Int | Dtype::Int64 => i128::from(value as i64),
        Dtype::Int8 => i128::from(value as i8),
        Dtype::Int16 => i128::from(value as i16),
        Dtype::Int32 => i128::from(value as i32),
        Dtype::UInt8 => i128::from(value as u8),
        Dtype::UInt16 => i128::from(value as u16),
        Dtype::UInt32 => i128::from(value as u32),
        Dtype::UInt64 => i128::from(value as u64),
        Dtype::Float16 | Dtype::Float32 | Dtype::Float64 | Dtype::Bool => value,
    }
}

/// A compile-time value.
///
/// Scalar values drive folding; `Tuple`/`List` let `comptime for` iterate
/// compile-time collections; `Type` carries a semantic type for associated
/// comptime members; `Expr` is a symbolic parameter expression while a generic
/// body is being checked.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CtValue {
    /// An already-materialized machine `Int` compile-time value. Compiler
    /// generated indices, lengths, and value parameters use this variant.
    Int(i64),
    UInt(u64),
    /// The bits of an already-materialized `Float64` compile-time value.
    Float(u64),
    /// An arbitrary-precision integer literal which has not yet been
    /// materialized into a fixed-width scalar.
    IntLiteral(IntLiteral),
    /// An exact finite floating literal which has not yet been materialized.
    FloatLiteral(FloatLiteral),
    Bool(bool),
    Str(String),
    Tuple(Vec<Self>),
    List(Vec<Self>),
    /// A compile-time dictionary display (`comptime M = {"a": 1}`):
    /// insertion-ordered entries deduplicated by structural key equality (a
    /// later duplicate key replaces the value in place, as `Dict.__setitem__`
    /// does). `spelling` is the explicit nominal type when one was given — an
    /// annotation (`comptime E: Dict[String, Int] = {}`) or the explicit
    /// literal constructor (`Dict[K, V, H](keys, values, None)`) — so
    /// materialization spells that constructor; `None` is the bare display,
    /// which the checker types with the default hasher.
    Dict {
        spelling: Option<Box<Ty>>,
        entries: Vec<(Self, Self)>,
    },
    /// A compile-time set display (`comptime S = {1, 2, 3}`); see `Dict`.
    Set {
        spelling: Option<Box<Ty>>,
        elements: Vec<Self>,
    },
    /// A `DType.<dt>` compile-time value — the binding of a `[dtype: DType]`
    /// value parameter. Materializes as the member spelling, which type
    /// resolution already accepts inside `SIMD[...]`/`Scalar[...]` brackets.
    Dtype(mojito_ast::ast::Dtype),
    /// A compile-time SIMD vector — the binding of a `[key: SIMD[DType.d, w]]`
    /// value parameter (`AHasher[key: U256]`). Lanes are already wrapped to
    /// `dtype`; it displays as upstream's parameter rendering
    /// (`[0, 0, 0, 0] : SIMD[DType.uint64, 4]`) and materializes as the
    /// explicit `SIMD[DType.d, w](lanes...)` construction.
    Simd {
        dtype: mojito_ast::ast::Dtype,
        lanes: Vec<CtLane>,
    },
    /// A frozen struct instance (declaration-ordered fields) — the binding of
    /// a struct-typed value parameter such as `[e: Extent]`. Freezing is
    /// restricted to structs constructible fieldwise from recursively
    /// freezable fields, so materialization is always the fieldwise
    /// construction call.
    Struct {
        name: String,
        fields: Vec<(String, Self)>,
    },
    Type(Box<Ty>),
    /// The zero-sized compile-time handle produced by current Mojo's
    /// `reflect[T]` API. Field selection returns another handle, allowing
    /// `.field[name]` / `.field_at[index]` chains to terminate in `.T`.
    Reflected(Box<Ty>),
    /// A residual parameter expression: a declared parameter, or an operator
    /// over one, while a generic declaration is checked. Never a constant — a
    /// folded expression is its ordinary concrete variant
    /// ([`ParamExpr::into_value`]).
    Expr(ParamExpr),
    /// The slot of a declared value parameter whose value arrives later and
    /// takes no part in generic identity: a callable-value parameter the VM
    /// reifies under the binder's spelling, or a value argument only the
    /// elaborator can fold. It names the binder whose slot it fills, is not a
    /// reference to a parameter in scope, and never enters a specialization
    /// key.
    Deferred(ParamRef),
    /// An elaborator-private classification of a name. It stands for no
    /// parameter and no value.
    Marker(CtMarker),
}

/// What the elaborator knows about a name that has no compile-time value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CtMarker {
    /// A runtime local: a parameter or a declared variable.
    RuntimeLocal,
    /// A declared struct name.
    TypeName,
    /// An origin parameter of the struct being walked, by its
    /// declaration-order identity and the permission it grants.
    TupleOrigin {
        id: OriginParamId,
        mutability: Mutability,
    },
    /// A module constant whose initializer applies a callable, asks a
    /// layout, or reads such a constant: its identity is the application
    /// (decision D3), which the check binds and the elaborator below MIR
    /// evaluates on first demand. It has no value above the check.
    Applied,
}

impl fmt::Display for CtMarker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeLocal => f.write_str("$local"),
            Self::TypeName => f.write_str("$type"),
            Self::Applied => f.write_str("$applied"),
            Self::TupleOrigin { id, mutability } => {
                let permission = match mutability {
                    Mutability::Immutable => "imm",
                    Mutability::Mutable => "mut",
                    Mutability::Param(_) => "param",
                };
                write!(f, "$tuple-origin:{}:{permission}", id.0)
            }
        }
    }
}

impl CtValue {
    /// What a `comptime for` over this value binds, in order: a list's or a
    /// set's elements, a dictionary's keys, and a bound value pack's
    /// elements; `None` for any other value. One rule for both unrollers,
    /// the AST elaborator's and `native::mono`'s.
    pub fn comptime_iteration_elements(&self) -> Option<Vec<Self>> {
        match self {
            Self::Tuple(values)
            | Self::List(values)
            | Self::Set {
                elements: values, ..
            } => Some(values.clone()),
            Self::Dict { entries, .. } => {
                Some(entries.iter().map(|(key, _)| key.clone()).collect())
            }
            _ => None,
        }
    }

    /// A compile-time dictionary from display-ordered entries: a repeated key
    /// keeps its first position and takes the last value.
    pub fn dict(spelling: Option<Ty>, entries: Vec<(Self, Self)>) -> Self {
        let mut deduplicated: Vec<(Self, Self)> = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            match deduplicated
                .iter_mut()
                .find(|(existing, _)| *existing == key)
            {
                Some(entry) => entry.1 = value,
                None => deduplicated.push((key, value)),
            }
        }
        Self::Dict {
            spelling: spelling.map(Box::new),
            entries: deduplicated,
        }
    }

    /// A compile-time set from display-ordered elements, first occurrence kept.
    pub fn set(spelling: Option<Ty>, elements: Vec<Self>) -> Self {
        let mut deduplicated: Vec<Self> = Vec::with_capacity(elements.len());
        for element in elements {
            if !deduplicated.contains(&element) {
                deduplicated.push(element);
            }
        }
        Self::Set {
            spelling: spelling.map(Box::new),
            elements: deduplicated,
        }
    }

    /// Whether this value is a collection that is not implicitly copyable at
    /// runtime (`Array`/`List`, `Dict`, `Set`): it never crosses to runtime
    /// implicitly and needs an explicit `materialize[...]()`.
    pub const fn is_runtime_collection(&self) -> bool {
        matches!(self, Self::List(_) | Self::Dict { .. } | Self::Set { .. })
    }

    /// Whether this value is a runtime collection ([`Self::is_runtime_collection`])
    /// whose iteration yields values a `comptime for` binder holds: an
    /// `Int`, a `Float64`, a `Bool`, or a `String`, or a tuple or struct
    /// over such values, closed ([`Self::is_closed_aggregate`]) or holding a
    /// string an instance constructs
    /// ([`Self::is_constructed_parameter_value`]). A dictionary yields its
    /// keys.
    pub fn is_parameter_value_collection(&self) -> bool {
        let element = |value: &Self| {
            matches!(
                value,
                Self::Int(_)
                    | Self::IntLiteral(_)
                    | Self::Float(_)
                    | Self::FloatLiteral(_)
                    | Self::Bool(_)
                    | Self::Str(_)
            ) || value.is_closed_aggregate()
                || value.is_constructed_parameter_value()
        };
        match self {
            Self::List(elements) | Self::Set { elements, .. } => elements.iter().all(element),
            Self::Dict { entries, .. } => entries.iter().all(|(key, _)| element(key)),
            _ => false,
        }
    }

    /// Whether this value is a tuple or struct whose leaves are numbers and
    /// booleans: a parameter constant a backend materializes without
    /// constructing anything.
    pub fn is_closed_aggregate(&self) -> bool {
        let leaf = |value: &Self| {
            matches!(
                value,
                Self::Int(_)
                    | Self::UInt(_)
                    | Self::IntLiteral(_)
                    | Self::Float(_)
                    | Self::FloatLiteral(_)
                    | Self::Bool(_)
            ) || value.is_closed_aggregate()
        };
        match self {
            Self::Tuple(elements) => !elements.is_empty() && elements.iter().all(leaf),
            Self::Struct { fields, .. } => fields.iter().all(|(_, field)| leaf(field)),
            _ => false,
        }
    }

    /// Whether this value is one a backend materializes as a parameter
    /// constant: a vector, or a struct or tuple whose leaves are scalars,
    /// vectors, or such aggregates.
    pub fn is_closed_parameter_value(&self) -> bool {
        match self {
            Self::Simd { .. } => true,
            Self::Struct { fields, .. } => fields.iter().all(|(_, field)| field.is_closed_leaf()),
            Self::Tuple(elements) => elements.iter().all(Self::is_closed_leaf),
            _ => false,
        }
    }

    /// Whether this value is a closed struct or tuple: every leaf a closed
    /// parameter leaf or a string.
    pub fn is_closed_aggregate_value(&self) -> bool {
        matches!(self, Self::Struct { .. } | Self::Tuple(_))
            && (self.is_closed_parameter_value() || self.is_constructed_parameter_value())
    }

    /// Whether a bracket argument solved to this value has no run-time form
    /// at its call, which carries it as compile-time data alone: a closed
    /// scalar, vector, or aggregate. A string is not one: `external_call`
    /// reads its callee off the argument's register.
    pub fn is_folded_parameter_argument(&self) -> bool {
        self.is_closed_leaf() || self.is_closed_aggregate_value()
    }

    /// Whether this value is a struct or tuple an instance constructs at run
    /// time rather than folds: every leaf is a closed parameter leaf or a
    /// string, and at least one is a string, which owns a buffer no constant
    /// can hold.
    pub fn is_constructed_parameter_value(&self) -> bool {
        fn leaf(value: &CtValue) -> bool {
            matches!(value, CtValue::Str(_))
                || value.is_closed_leaf()
                || value.is_constructed_parameter_value()
        }
        let leaves: Vec<&Self> = match self {
            Self::Struct { fields, .. } => fields.iter().map(|(_, field)| field).collect(),
            Self::Tuple(elements) => elements.iter().collect(),
            _ => return false,
        };
        leaves.iter().all(|value| leaf(value))
            && leaves.iter().any(|value| {
                matches!(value, Self::Str(_)) || value.is_constructed_parameter_value()
            })
    }

    /// The spelling of the runtime type an un-annotated binding of this value
    /// has (upstream's wording in the materialization diagnostic: a list is
    /// `Array[T, Int(n)]`), or `None` for a compile-time-only value.
    pub fn runtime_type_text(&self) -> Option<String> {
        Some(match self {
            Self::Int(_) | Self::IntLiteral(_) => "Int".to_string(),
            Self::UInt(_) => "UInt".to_string(),
            Self::Float(_) | Self::FloatLiteral(_) => "Float64".to_string(),
            Self::Bool(_) => "Bool".to_string(),
            Self::Str(_) => "String".to_string(),
            Self::Dtype(_) => "DType".to_string(),
            Self::Simd { dtype, lanes } => {
                format!("SIMD[DType.{}, {}]", dtype.name(), lanes.len())
            }
            Self::Struct { name, .. } => name.clone(),
            Self::Tuple(values) => format!(
                "Tuple[{}]",
                values
                    .iter()
                    .map(Self::runtime_type_text)
                    .collect::<Option<Vec<_>>>()?
                    .join(", ")
            ),
            Self::List(values) => format!(
                "Array[{}, Int({})]",
                values.first()?.runtime_type_text()?,
                values.len()
            ),
            Self::Dict { spelling, entries } => {
                if let Some(ty) = spelling {
                    ty.to_string()
                } else {
                    let (key, value) = entries.first()?;
                    format!(
                        "Dict[{}, {}]",
                        key.runtime_type_text()?,
                        value.runtime_type_text()?
                    )
                }
            }
            Self::Set { spelling, elements } => match spelling {
                Some(ty) => ty.to_string(),
                None => format!("Set[{}]", elements.first()?.runtime_type_text()?),
            },
            Self::Type(_)
            | Self::Reflected(_)
            | Self::Expr(_)
            | Self::Deferred(_)
            | Self::Marker(_) => return None,
        })
    }

    /// Materialize an exact literal into the compile-time representation of a
    /// declared scalar type. Values which are already materialized are kept as
    /// is. This is the checked boundary used by value parameters and defaults;
    /// it deliberately leaves uncontextualized literal values exact.
    ///
    /// # Panics
    ///
    /// Panics if the `List` guard above admits a target without an element
    /// type, which the type lattice forbids.
    pub fn materialize_as(self, ty: &Ty) -> Option<Self> {
        match (self, ty) {
            (value @ Self::Int(_), Ty::Int)
            | (value @ Self::UInt(_), Ty::UInt)
            | (value @ Self::Float(_), Ty::Float64)
            | (value @ Self::IntLiteral(_), Ty::IntLiteral)
            | (value @ Self::FloatLiteral(_), Ty::FloatLiteral)
            | (value @ Self::Bool(_), Ty::Bool)
            | (value @ Self::Str(_), Ty::StringLiteral) => Some(value),
            // A string literal is the nominal `String`'s compile-time form
            // (`comptime E: Dict[String, Int] = {"a": 1}`).
            (value @ Self::Str(_), Ty::Struct(name, args))
                if args.is_empty() && crate::types::is_stdlib_string_struct(name) =>
            {
                Some(value)
            }
            (value @ Self::Dtype(_), Ty::Dtype) => Some(value),
            (
                value @ Self::Simd { .. },
                Ty::Simd {
                    dtype: SimdDtype::Known(target),
                    width: SimdWidth::Known(width),
                },
            ) => {
                let Self::Simd { dtype, lanes } = &value else {
                    unreachable!("guard established a SIMD value");
                };
                (dtype == target && lanes.len() as i64 == *width).then_some(value)
            }
            (value @ Self::Struct { .. }, Ty::Struct(target, _)) => {
                let Self::Struct { name, .. } = &value else {
                    unreachable!("guard established a struct value");
                };
                (name == target).then_some(value)
            }
            (Self::IntLiteral(value), Ty::Int) => value.wrapping_signed(64).map(CtValue::Int),
            (Self::IntLiteral(value), Ty::UInt) => value.wrapping_unsigned(64).map(CtValue::UInt),
            (Self::IntLiteral(value), Ty::Float64) => {
                value.to_f64().map(|value| Self::Float(value.to_bits()))
            }
            (Self::FloatLiteral(value), Ty::Float64) => {
                value.to_f64().map(|value| Self::Float(value.to_bits()))
            }
            (Self::Tuple(values), Ty::Tuple(types)) if values.len() == types.len() => values
                .into_iter()
                .zip(types)
                .map(|(value, ty)| value.materialize_as(ty))
                .collect::<Option<Vec<_>>>()
                .map(CtValue::Tuple),
            // The nominal `Tuple[...]` holds its elements at its arguments.
            (Self::Tuple(values), target) if crate::types::tuple_elements(target).is_some() => {
                let types = crate::types::tuple_elements(target).expect("guard established Tuple");
                (values.len() == types.len())
                    .then(|| {
                        values
                            .into_iter()
                            .zip(types)
                            .map(|(value, ty)| value.materialize_as(ty))
                            .collect::<Option<Vec<_>>>()
                            .map(CtValue::Tuple)
                    })
                    .flatten()
            }
            (Self::List(values), Ty::ComptimeList(element)) => values
                .into_iter()
                .map(|value| value.materialize_as(element))
                .collect::<Option<Vec<_>>>()
                .map(CtValue::List),
            (Self::List(values), target) if list_element(target).is_some() => {
                let element = list_element(target).expect("guard established List element");
                values
                    .into_iter()
                    .map(|value| value.materialize_as(element))
                    .collect::<Option<Vec<_>>>()
                    .map(CtValue::List)
            }
            (Self::Dict { entries, .. }, target) if dict_elements(target).is_some() => {
                let (key_ty, value_ty) =
                    dict_elements(target).expect("guard established Dict elements");
                let entries = entries
                    .into_iter()
                    .map(|(key, value)| {
                        Some((key.materialize_as(key_ty)?, value.materialize_as(value_ty)?))
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(Self::dict(Some(target.clone()), entries))
            }
            (Self::Set { elements, .. }, target) if set_element(target).is_some() => {
                let element_ty = set_element(target).expect("guard established Set element");
                let elements = elements
                    .into_iter()
                    .map(|element| element.materialize_as(element_ty))
                    .collect::<Option<Vec<_>>>()?;
                Some(Self::set(Some(target.clone()), elements))
            }
            (Self::Tuple(values), target)
                if tuple_elements(target).is_some_and(|types| types.len() == values.len()) =>
            {
                values
                    .into_iter()
                    .zip(tuple_elements(target).expect("guard established Tuple elements"))
                    .map(|(value, ty)| value.materialize_as(ty))
                    .collect::<Option<Vec<_>>>()
                    .map(CtValue::Tuple)
            }
            _ => None,
        }
    }

    /// Materialize this value as a literal expression, or `None` when it has no
    /// runtime form (a symbolic `Expr`, or a collection containing one).
    pub fn materialize(&self, span: Span) -> Option<Expr> {
        let kind = match self {
            Self::Int(n) => ExprKind::Int(IntLiteral::from(*n)),
            Self::UInt(n) => ExprKind::Int(IntLiteral::from(*n)),
            Self::Float(bits) => ExprKind::Float(FloatLiteral::from_f64(f64::from_bits(*bits))?),
            Self::IntLiteral(value) => ExprKind::Int(value.clone()),
            Self::FloatLiteral(value) => ExprKind::Float(value.clone()),
            Self::Bool(b) => ExprKind::Bool(*b),
            Self::Str(s) => ExprKind::Str(s.clone()),
            Self::Tuple(vs) => ExprKind::TupleLit(materialize_all(vs, span)?),
            Self::List(vs) => ExprKind::ListLit(materialize_all(vs, span)?),
            // The bare display, or the explicit literal constructor
            // `Dict[K, V, H](keys, values, None)` when the value was spelled
            // with one (an empty explicit dict is `Dict[K, V]()`).
            Self::Dict { spelling, entries } => match spelling {
                None => ExprKind::BraceLit(
                    entries
                        .iter()
                        .map(|(key, value)| {
                            Some((key.materialize(span)?, Some(value.materialize(span)?)))
                        })
                        .collect::<Option<Vec<_>>>()?,
                ),
                Some(ty) => {
                    let Type::Named(name, param_args) = source_type(ty, span)? else {
                        return None;
                    };
                    // The key and value lists are explicit `List[K](...)`
                    // constructions rather than displays: a display argument
                    // takes no context from a parameterized constructor's
                    // `List[Self.K]` parameter.
                    let args = if entries.is_empty() {
                        Vec::new()
                    } else {
                        let [ParamArg::Type(key_ty), ParamArg::Type(value_ty), ..] =
                            param_args.as_slice()
                        else {
                            return None;
                        };
                        let keys: Vec<&Self> = entries.iter().map(|(key, _)| key).collect();
                        let values: Vec<&Self> = entries.iter().map(|(_, value)| value).collect();
                        vec![
                            list_construction(key_ty.clone(), materialize_refs(&keys, span)?, span),
                            list_construction(
                                value_ty.clone(),
                                materialize_refs(&values, span)?,
                                span,
                            ),
                            literal(ExprKind::None, span),
                        ]
                    };
                    ExprKind::Call {
                        name,
                        param_args,
                        args,
                        kwargs: Vec::new(),
                    }
                }
            },
            Self::Set { spelling, elements } => match spelling {
                None => ExprKind::BraceLit(
                    elements
                        .iter()
                        .map(|element| Some((element.materialize(span)?, None)))
                        .collect::<Option<Vec<_>>>()?,
                ),
                Some(ty) => {
                    let Type::Named(name, param_args) = source_type(ty, span)? else {
                        return None;
                    };
                    let kwargs = if elements.is_empty() {
                        Vec::new()
                    } else {
                        vec![KwArg {
                            name: "__set_literal__".to_string(),
                            value: literal(ExprKind::None, span),
                        }]
                    };
                    ExprKind::Call {
                        name,
                        param_args,
                        args: materialize_all(elements, span)?,
                        kwargs,
                    }
                }
            },
            Self::Dtype(dtype) => ExprKind::Member {
                object: Box::new(Expr {
                    kind: ExprKind::Identifier("DType".to_string()),
                    span,
                    source: None,
                    syntax_id: mojito_common::token::SyntaxId::fresh(),
                }),
                field: dtype.name().to_string(),
            },
            // The explicit construction `SIMD[DType.d, w](l0, l1, ...)`.
            Self::Simd { dtype, lanes } => ExprKind::Call {
                name: "SIMD".to_string(),
                param_args: vec![
                    ParamArg::Value(Self::Dtype(*dtype).materialize(span)?),
                    ParamArg::Value(Self::Int(lanes.len() as i64).materialize(span)?),
                ],
                args: lanes
                    .iter()
                    .map(|lane| {
                        match lane {
                            CtLane::Int(value) => Self::IntLiteral(lane_literal(*value)),
                            CtLane::Float(bits) => Self::Float(*bits),
                            CtLane::Bool(value) => Self::Bool(*value),
                        }
                        .materialize(span)
                    })
                    .collect::<Option<Vec<_>>>()?,
                kwargs: Vec::new(),
            },
            // The fieldwise construction call; freezing guaranteed a matching
            // constructor exists.
            Self::Struct { name, fields } => ExprKind::Call {
                name: name.clone(),
                param_args: Vec::new(),
                args: fields
                    .iter()
                    .map(|(_, value)| value.materialize(span))
                    .collect::<Option<Vec<_>>>()?,
                kwargs: Vec::new(),
            },
            Self::Type(_)
            | Self::Reflected(_)
            | Self::Expr(_)
            | Self::Deferred(_)
            | Self::Marker(_) => return None,
        };
        Some(Expr {
            kind,
            span,
            source: None,
            syntax_id: mojito_common::token::SyntaxId::fresh(),
        })
    }

    /// A field or element of a closed aggregate parameter value: a scalar,
    /// or itself a closed aggregate.
    fn is_closed_leaf(&self) -> bool {
        matches!(
            self,
            Self::Int(_)
                | Self::UInt(_)
                | Self::Float(_)
                | Self::IntLiteral(_)
                | Self::FloatLiteral(_)
                | Self::Bool(_)
                | Self::Dtype(_)
        ) || self.is_closed_parameter_value()
    }
}

/// The source spelling of a closed type (`Dict[String, Int, Fnv1a]`).
///
/// Scalars, nominal structs over type/value arguments, and SIMD, as a
/// materialized collection type or an initializer list's construction spells
/// it. Origin arguments and callables have no spelling here.
pub fn source_type(ty: &Ty, span: Span) -> Option<Type> {
    Some(match ty {
        Ty::Int | Ty::IntLiteral => Type::Int,
        Ty::UInt => Type::UInt,
        Ty::Bool => Type::Bool,
        Ty::StringLiteral => Type::StringLiteral,
        Ty::Float64 | Ty::FloatLiteral => Type::Float64,
        Ty::None => Type::None,
        Ty::Dtype => Type::Named("DType".to_string(), Vec::new()),
        Ty::Simd {
            dtype: SimdDtype::Known(dtype),
            width: SimdWidth::Known(width),
        } => Type::Named(
            "SIMD".to_string(),
            vec![
                ParamArg::Value(CtValue::Dtype(*dtype).materialize(span)?),
                ParamArg::Value(CtValue::Int(*width).materialize(span)?),
            ],
        ),
        // A symbolic slot has no source spelling: its declaration is still
        // a template.
        Ty::Simd { .. } => return None,
        // A generated public-Tuple specialization keeps its element types
        // behind an argument-less symbol: spell the canonical `Tuple[...]`
        // application, which the checker maps back onto the specialization.
        Ty::Struct(name, _)
            if name != crate::types::TUPLE_TYPE_NAME
                && let Some(elements) = crate::types::tuple_elements(ty) =>
        {
            Type::Named(
                crate::types::TUPLE_TYPE_NAME.to_string(),
                elements
                    .into_iter()
                    .map(|element| source_type(element, span).map(ParamArg::Type))
                    .collect::<Option<Vec<_>>>()?,
            )
        }
        Ty::Struct(name, arguments) => Type::Named(
            name.clone(),
            arguments
                .iter()
                .map(|argument| match argument {
                    TyArg::Ty(ty) => Some(ParamArg::Type(source_type(ty, span)?)),
                    TyArg::Val(value) => Some(ParamArg::Value(value.materialize(span)?)),
                    // An origin tail entry has no source spelling of its own;
                    // upstream's `_` placeholder leaves the slot to inference
                    // at the materialized spelling's use.
                    TyArg::Origin(_) => Some(ParamArg::Value(Expr::new(
                        ExprKind::Identifier("_".to_string()),
                        span,
                    ))),
                })
                .collect::<Option<Vec<_>>>()?,
        ),
        _ => return None,
    })
}

/// The exact literal of one integer lane (unsigned lanes exceed `i64`).
fn lane_literal(value: i128) -> IntLiteral {
    if let Ok(value) = i64::try_from(value) {
        IntLiteral::from(value)
    } else {
        IntLiteral::from(value as u64)
    }
}

fn materialize_all(vs: &[CtValue], span: Span) -> Option<Vec<Expr>> {
    vs.iter().map(|v| v.materialize(span)).collect()
}

fn materialize_refs(vs: &[&CtValue], span: Span) -> Option<Vec<Expr>> {
    vs.iter().map(|v| v.materialize(span)).collect()
}

/// `List[T](elements..., __list_literal__=None)`.
fn list_construction(element: Type, elements: Vec<Expr>, span: Span) -> Expr {
    literal(
        ExprKind::Call {
            name: "List".to_string(),
            param_args: vec![ParamArg::Type(element)],
            args: elements,
            kwargs: vec![KwArg {
                name: "__list_literal__".to_string(),
                value: literal(ExprKind::None, span),
            }],
        },
        span,
    )
}

fn literal(kind: ExprKind, span: Span) -> Expr {
    Expr {
        kind,
        span,
        source: None,
        syntax_id: mojito_common::token::SyntaxId::fresh(),
    }
}

impl fmt::Display for CtValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(n) => write!(f, "{n}"),
            Self::UInt(n) => write!(f, "{n}u"),
            Self::Float(bits) => write!(f, "{:?}", f64::from_bits(*bits)),
            Self::IntLiteral(value) => write!(f, "{value}"),
            Self::FloatLiteral(value) => write!(f, "{value}"),
            Self::Bool(b) => write!(f, "{b}"),
            Self::Str(s) => write!(f, "{s:?}"),
            Self::Dtype(dtype) => write!(f, "DType.{}", dtype.name()),
            // Upstream's rendering of a SIMD parameter value.
            Self::Simd { dtype, lanes } => {
                write!(f, "[")?;
                for (index, lane) in lanes.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    match lane {
                        CtLane::Int(value) => write!(f, "{value}")?,
                        CtLane::Float(bits) => write!(f, "{:?}", f64::from_bits(*bits))?,
                        CtLane::Bool(value) => {
                            write!(f, "{}", if *value { "True" } else { "False" })?;
                        }
                    }
                }
                write!(f, "] : SIMD[DType.{}, {}]", dtype.name(), lanes.len())
            }
            Self::Struct { name, fields } => {
                write!(f, "{name}(")?;
                for (index, (_, value)) in fields.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{value}")?;
                }
                write!(f, ")")
            }
            Self::Type(ty) => write!(f, "{ty}"),
            Self::Reflected(ty) => write!(f, "reflect[{ty}]"),
            Self::Expr(expr) => write!(f, "{expr}"),
            Self::Deferred(binder) => write!(f, "{binder}"),
            Self::Marker(marker) => write!(f, "{marker}"),
            Self::Dict { entries, .. } => {
                write!(f, "{{")?;
                for (index, (key, value)) in entries.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{key}: {value}")?;
                }
                write!(f, "}}")
            }
            Self::Set { elements, .. } => {
                write!(f, "{{")?;
                for (index, element) in elements.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{element}")?;
                }
                write!(f, "}}")
            }
            Self::Tuple(vs) | Self::List(vs) => {
                let (open, close) = match self {
                    Self::Tuple(_) => ('(', ')'),
                    _ => ('[', ']'),
                };
                write!(f, "{open}")?;
                for (i, v) in vs.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{v}")?;
                }
                write!(f, "{close}")
            }
        }
    }
}
