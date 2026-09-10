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
//! runtime literal form; `Type`, `Reflected`, and `Param` are compile-time-only.

use crate::types::{Ty, TyArg, dict_elements, list_element, set_element, tuple_elements};
use mojito_ast::ast::{Expr, ExprKind, KwArg, ParamArg, Type};
use mojito_common::literal::{FloatLiteral, IntLiteral};
use mojito_common::token::Span;
use std::fmt;

/// One lane of a compile-time SIMD value: integer lanes hold the post-wrap
/// mathematical value (an unsigned lane is non-negative), float lanes their
/// IEEE bits (so equality is structural).
#[derive(Debug, Clone, PartialEq, Eq)]
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
            Dtype::Float32 | Dtype::Float64 => {
                let value = match value {
                    CtValue::Float(bits) => f64::from_bits(*bits),
                    CtValue::FloatLiteral(value) => value.to_f64()?,
                    CtValue::IntLiteral(value) => value.to_f64()?,
                    CtValue::Int(value) => *value as f64,
                    CtValue::UInt(value) => *value as f64,
                    _ => return None,
                };
                let value = if dtype == Dtype::Float32 {
                    value as f32 as f64
                } else {
                    value
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
        Dtype::Float32 | Dtype::Float64 | Dtype::Bool => value,
    }
}

/// A compile-time value.
///
/// Scalar values drive folding; `Tuple`/`List` let `comptime for` iterate
/// compile-time collections; `Type` carries a semantic type for associated
/// comptime members; `Param` is a symbolic value parameter while a generic
/// body is being checked.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    Param(String),
}

/// A canonical dependent compile-time expression retained in generic metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CtExpr {
    Value(CtValue),
    Param(String),
    Neg(Box<Self>),
    Add(Box<Self>, Box<Self>),
    Sub(Box<Self>, Box<Self>),
    Mul(Box<Self>, Box<Self>),
    FloorDiv(Box<Self>, Box<Self>),
    Mod(Box<Self>, Box<Self>),
    Pow(Box<Self>, Box<Self>),
}

impl CtExpr {
    /// Collect the symbolic compile-time binders referenced by this expression.
    /// The verifier uses this to reject dependent types whose index escaped its
    /// generic declaration scope.
    pub fn referenced_parameters(&self, output: &mut std::collections::HashSet<String>) {
        use CtExpr::{Add, FloorDiv, Mod, Mul, Neg, Param, Pow, Sub, Value};
        match self {
            Param(name) => {
                output.insert(name.clone());
            }
            Value(_) => {}
            Neg(value) => value.referenced_parameters(output),
            Add(left, right)
            | Sub(left, right)
            | Mul(left, right)
            | FloorDiv(left, right)
            | Mod(left, right)
            | Pow(left, right) => {
                left.referenced_parameters(output);
                right.referenced_parameters(output);
            }
        }
    }

    /// Alpha-rename symbolic binders while preserving the expression tree.
    #[must_use]
    pub fn rename_parameters(&self, names: &std::collections::HashMap<String, String>) -> Self {
        use CtExpr::{Add, FloorDiv, Mod, Mul, Neg, Param, Pow, Sub, Value};
        match self {
            Value(value) => Value(value.clone()),
            Param(name) => Param(names.get(name).cloned().unwrap_or_else(|| name.clone())),
            Neg(value) => Neg(Box::new(value.rename_parameters(names))),
            Add(left, right) => Add(
                Box::new(left.rename_parameters(names)),
                Box::new(right.rename_parameters(names)),
            ),
            Sub(left, right) => Sub(
                Box::new(left.rename_parameters(names)),
                Box::new(right.rename_parameters(names)),
            ),
            Mul(left, right) => Mul(
                Box::new(left.rename_parameters(names)),
                Box::new(right.rename_parameters(names)),
            ),
            FloorDiv(left, right) => FloorDiv(
                Box::new(left.rename_parameters(names)),
                Box::new(right.rename_parameters(names)),
            ),
            Mod(left, right) => Mod(
                Box::new(left.rename_parameters(names)),
                Box::new(right.rename_parameters(names)),
            ),
            Pow(left, right) => Pow(
                Box::new(left.rename_parameters(names)),
                Box::new(right.rename_parameters(names)),
            ),
        }
    }

    pub fn evaluate(
        &self,
        parameters: &std::collections::HashMap<String, CtValue>,
    ) -> Option<CtValue> {
        use CtExpr::{Add, FloorDiv, Mod, Mul, Neg, Param, Pow, Sub, Value};
        match self {
            Value(value) => Some(value.clone()),
            Param(name) => parameters.get(name).cloned(),
            Neg(value) => match value.evaluate(parameters)? {
                CtValue::Int(value) => value.checked_neg().map(CtValue::Int),
                CtValue::IntLiteral(value) => Some(CtValue::IntLiteral(value.neg())),
                _ => None,
            },
            Add(left, right) => match (left.evaluate(parameters)?, right.evaluate(parameters)?) {
                (CtValue::Int(left), CtValue::Int(right)) => {
                    left.checked_add(right).map(CtValue::Int)
                }
                (CtValue::IntLiteral(left), CtValue::IntLiteral(right)) => {
                    Some(CtValue::IntLiteral(left.add(&right)))
                }
                (CtValue::Int(left), CtValue::IntLiteral(right)) => {
                    Some(CtValue::IntLiteral(IntLiteral::from(left).add(&right)))
                }
                (CtValue::IntLiteral(left), CtValue::Int(right)) => {
                    Some(CtValue::IntLiteral(left.add(&IntLiteral::from(right))))
                }
                (CtValue::Str(left), CtValue::Str(right)) => Some(CtValue::Str(left + &right)),
                _ => None,
            },
            Sub(left, right) => int_binary(left, right, parameters, i64::checked_sub, |a, b| {
                Some(a.sub(b))
            }),
            Mul(left, right) => int_binary(left, right, parameters, i64::checked_mul, |a, b| {
                Some(a.mul(b))
            }),
            FloorDiv(left, right) => int_binary(
                left,
                right,
                parameters,
                i64::checked_div_euclid,
                IntLiteral::floor_div,
            ),
            Mod(left, right) => int_binary(
                left,
                right,
                parameters,
                i64::checked_rem_euclid,
                IntLiteral::floor_mod,
            ),
            Pow(left, right) => int_binary(
                left,
                right,
                parameters,
                |a, b| u32::try_from(b).ok().and_then(|b| a.checked_pow(b)),
                IntLiteral::pow,
            ),
        }
    }
}

fn int_binary(
    left: &CtExpr,
    right: &CtExpr,
    parameters: &std::collections::HashMap<String, CtValue>,
    operation: impl FnOnce(i64, i64) -> Option<i64>,
    literal_operation: impl FnOnce(&IntLiteral, &IntLiteral) -> Option<IntLiteral>,
) -> Option<CtValue> {
    match (left.evaluate(parameters)?, right.evaluate(parameters)?) {
        (CtValue::Int(left), CtValue::Int(right)) => operation(left, right).map(CtValue::Int),
        (CtValue::IntLiteral(left), CtValue::IntLiteral(right)) => {
            literal_operation(&left, &right).map(CtValue::IntLiteral)
        }
        (CtValue::Int(left), CtValue::IntLiteral(right)) => {
            literal_operation(&left.into(), &right).map(CtValue::IntLiteral)
        }
        (CtValue::IntLiteral(left), CtValue::Int(right)) => {
            literal_operation(&left, &right.into()).map(CtValue::IntLiteral)
        }
        _ => None,
    }
}

impl CtValue {
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
            Self::Type(_) | Self::Reflected(_) | Self::Param(_) => return None,
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
                    dtype: target,
                    width,
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
    /// runtime form (a symbolic `Param`, or a collection containing one).
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
            Self::Type(_) | Self::Reflected(_) => return None,
            Self::Param(_) => return None,
        };
        Some(Expr {
            kind,
            span,
            source: None,
            syntax_id: mojito_common::token::SyntaxId::fresh(),
        })
    }
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

/// The source spelling of an explicit collection type (`Dict[String, Int,
/// Fnv1a]`): scalars, nominal structs over type/value arguments, and SIMD.
/// Origin arguments and callables have no spelling here.
fn source_type(ty: &Ty, span: Span) -> Option<Type> {
    Some(match ty {
        Ty::Int | Ty::IntLiteral => Type::Int,
        Ty::UInt => Type::UInt,
        Ty::Bool => Type::Bool,
        Ty::StringLiteral => Type::StringLiteral,
        Ty::Float64 | Ty::FloatLiteral => Type::Float64,
        Ty::None => Type::None,
        Ty::Simd { dtype, width } => Type::Named(
            "SIMD".to_string(),
            vec![
                ParamArg::Value(CtValue::Dtype(*dtype).materialize(span)?),
                ParamArg::Value(CtValue::Int(*width).materialize(span)?),
            ],
        ),
        Ty::Struct(name, arguments) => Type::Named(
            name.clone(),
            arguments
                .iter()
                .map(|argument| match argument {
                    TyArg::Ty(ty) => Some(ParamArg::Type(source_type(ty, span)?)),
                    TyArg::Val(value) => Some(ParamArg::Value(value.materialize(span)?)),
                    TyArg::Origin(_) => None,
                })
                .collect::<Option<Vec<_>>>()?,
        ),
        _ => return None,
    })
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
            Self::Param(name) => write!(f, "{name}"),
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
