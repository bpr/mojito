//! Parameter-expression attributes: the front end's canonical
//! [`ParamExpr`] graphs re-homed as context-owned payloads.
//!
//! The payload is storage, not a second normal form: construction,
//! folding, and replacement stay with [`ParamContext`], and export rebuilds
//! through its constructors. A payload holds no `Rc`, no source context,
//! and only context-local child keys, so it never crosses a context except
//! by structural clone.

use pliron::combine::Parser;
use pliron::context::Context;
use pliron::derive::{format, pliron_attr};
use pliron::parsable::{Parsable, ParseResult, StateStream};
use pliron::printable::{self, Printable};
use pliron::r#type::TypeHandle;
use pliron::uniqued_any::{self, UniquedKey};

use mojito_common::literal::IntLiteral;
use mojito_types::ct::CtValue;
use mojito_types::origin::{CallableEnvironment, SigOrigin};
use mojito_types::param_expr::{
    HoleKind, MetaTy, PackQuery, ParamContext, ParamExpr, ParamId, ParamKind, ParamOp, ParamRef,
    ReflectQuery,
};
use mojito_types::types::{TransferEffect, TransferSet, TrivialLifecycle, Ty};

use super::attrs::{CoreConvention, Text};
use super::types::{CoreDtype, export_type, import_type};
use super::{A1Error, A1ErrorKind};

/// A parameter expression carried on an operation or a nominal type.
#[pliron_attr(name = "mojito_core.param_expr", format = "$0", verifier = "succ")]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub struct ParamExprAttr(pub NodeKey);

/// The context-local handle of one payload node. Equal keys of one
/// context are one canonical node; keys of two contexts never compare.
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub struct NodeKey(UniquedKey<ParamExprPayload>);

impl NodeKey {
    pub fn payload(self, ctx: &Context) -> &ParamExprPayload {
        uniqued_any::get(ctx, self.0)
    }
}

impl Printable for NodeKey {
    fn fmt(
        &self,
        ctx: &Context,
        state: &printable::State,
        f: &mut core::fmt::Formatter<'_>,
    ) -> core::fmt::Result {
        self.payload(ctx).fmt(ctx, state, f)
    }
}

impl Parsable for NodeKey {
    type Arg = ();
    type Parsed = Self;

    fn parse<'a>(
        state_stream: &mut StateStream<'a>,
        (): Self::Arg,
    ) -> ParseResult<'a, Self::Parsed> {
        let parsed = ParamExprPayload::parser(()).parse_stream(state_stream);
        parsed
            .map(|payload| Self(uniqued_any::save(state_stream.state.ctx, payload)))
            .into()
    }
}

/// One node: its meta-type and its closed node kind.
#[format("`<` $meta ` | ` $node `>`")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct ParamExprPayload {
    pub meta: PayloadMeta,
    pub node: PayloadNode,
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub enum PayloadMeta {
    Value(PayloadTy),
    Type,
    ReflectedType,
    #[format("`[` vec($0, CharSpace(`,`)) `]`")]
    Tuple(Vec<Self>),
    #[format("`[` vec($0, CharSpace(`,`)) `]`")]
    List(Vec<Self>),
    #[format("`[` vec($0, CharSpace(`,`)) `]`")]
    Set(Vec<Self>),
    /// Keys and values, alternating.
    #[format("`[` vec($0, CharSpace(`,`)) `]`")]
    Dict(Vec<Self>),
    /// The one element meta-type of a parameter list.
    #[format("`[` vec($0, CharSpace(`,`)) `]`")]
    ParamList(Vec<Self>),
}

/// A binder by identity: its owning declaration and slot. The spelling is
/// diagnostic metadata, as it is on [`ParamRef`].
#[format("$owner ` ` $slot ` ` $name")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct PayloadBinder {
    pub owner: Text,
    pub slot: u64,
    pub name: Text,
}

impl PayloadBinder {
    pub fn from_ref(reference: &ParamRef) -> Self {
        Self {
            owner: reference.id.owner.as_ref().into(),
            slot: reference.id.slot as u64,
            name: reference.name.as_ref().into(),
        }
    }

    pub fn reference(&self) -> Result<ParamRef, A1Error> {
        let slot = usize::try_from(self.slot)
            .map_err(|_| A1Error::new(A1ErrorKind::Export, "a binder slot beyond usize"))?;
        Ok(ParamRef {
            id: ParamId::new(self.owner.as_str(), slot),
            name: self.name.as_str().into(),
        })
    }
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub enum PayloadTy {
    /// A closed type of the core vocabulary.
    Core(TypeHandle),
    /// A type parameter, symbolic: attribute-only, never an executable
    /// core type.
    #[format("`<` $binder ` [` vec($bounds, CharSpace(`,`)) `]>`")]
    Param {
        binder: PayloadBinder,
        bounds: Vec<Text>,
    },
    Callable(PayloadCallable),
}

#[format("$name ` : ` $ty ` ` $required ` convention` opt($convention, delimiters(`(`, `)`))")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct PayloadParam {
    pub name: Text,
    pub ty: PayloadTy,
    pub required: bool,
    pub convention: Option<CoreConvention>,
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum PayloadSigOrigin {
    Receiver,
    Param(u64),
    Static,
}

/// One inferred transfer effect of a callable occurrence.
#[format("$dest ` <- ` $src ` ` $src_is_place ` ` $mutable")]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub struct PayloadTransfer {
    pub dest: PayloadSigOrigin,
    pub src: PayloadSigOrigin,
    pub src_is_place: bool,
    pub mutable: bool,
}

/// A callable type occurrence. Type identity ignores `transfers`; the
/// payload does not, so two occurrences that differ only by their effects
/// are two nodes and neither inherits the other's.
#[format(
    "`(` vec($params, CharSpace(`,`)) `) -> [` vec($ret, CharSpace(`,`)) `] ` $raises ` effects [` vec($transfers, CharSpace(`,`)) `]`"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct PayloadCallable {
    pub params: Vec<PayloadParam>,
    /// The one result type.
    pub ret: Vec<PayloadTy>,
    pub raises: bool,
    pub transfers: Vec<PayloadTransfer>,
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub enum PayloadValue {
    Int(i64),
    UInt(u64),
    /// An exact integer literal in decimal.
    IntLiteral(Text),
    Bool(bool),
    Str(Text),
    Dtype(CoreDtype),
    Type(PayloadTy),
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum PayloadOp {
    Add,
    Mul,
    Neg,
    Sub,
    Div,
    FloorDiv,
    Mod,
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

impl PayloadOp {
    pub const fn from_op(op: ParamOp) -> Self {
        match op {
            ParamOp::Add => Self::Add,
            ParamOp::Mul => Self::Mul,
            ParamOp::Neg => Self::Neg,
            ParamOp::Sub => Self::Sub,
            ParamOp::Div => Self::Div,
            ParamOp::FloorDiv => Self::FloorDiv,
            ParamOp::Mod => Self::Mod,
            ParamOp::Pow => Self::Pow,
            ParamOp::Shl => Self::Shl,
            ParamOp::Shr => Self::Shr,
            ParamOp::BitAnd => Self::BitAnd,
            ParamOp::BitOr => Self::BitOr,
            ParamOp::BitXor => Self::BitXor,
            ParamOp::Eq => Self::Eq,
            ParamOp::Lt => Self::Lt,
            ParamOp::Le => Self::Le,
            ParamOp::BoolAnd => Self::BoolAnd,
            ParamOp::BoolOr => Self::BoolOr,
            ParamOp::BoolXor => Self::BoolXor,
            ParamOp::Cond => Self::Cond,
        }
    }

    pub const fn op(self) -> ParamOp {
        match self {
            Self::Add => ParamOp::Add,
            Self::Mul => ParamOp::Mul,
            Self::Neg => ParamOp::Neg,
            Self::Sub => ParamOp::Sub,
            Self::Div => ParamOp::Div,
            Self::FloorDiv => ParamOp::FloorDiv,
            Self::Mod => ParamOp::Mod,
            Self::Pow => ParamOp::Pow,
            Self::Shl => ParamOp::Shl,
            Self::Shr => ParamOp::Shr,
            Self::BitAnd => ParamOp::BitAnd,
            Self::BitOr => ParamOp::BitOr,
            Self::BitXor => ParamOp::BitXor,
            Self::Eq => ParamOp::Eq,
            Self::Lt => ParamOp::Lt,
            Self::Le => ParamOp::Le,
            Self::BoolAnd => ParamOp::BoolAnd,
            Self::BoolOr => ParamOp::BoolOr,
            Self::BoolXor => ParamOp::BoolXor,
            Self::Cond => ParamOp::Cond,
        }
    }
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum PayloadLifecycle {
    Movable,
    Copyable,
    Deinitable,
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub enum PayloadReflect {
    IsStruct,
    FieldCount,
    FieldNames,
    FieldTypes,
    FieldIndex(Text),
    FieldNamed(Text),
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub enum PayloadPackQuery {
    Length,
    Conforms(Text),
    Contains(NodeKey),
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum PayloadHole {
    Unknown,
    Unbound,
}

/// The closed node kinds, one per [`ParamKind`].
#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub enum PayloadNode {
    Constant(PayloadValue),
    DeclRef(PayloadBinder),
    #[format("`<` $depth `, ` $index `>`")]
    IndexRef {
        depth: u32,
        index: u32,
    },
    #[format("`<` $op ` [` vec($operands, CharSpace(`,`)) `]>`")]
    Op {
        op: PayloadOp,
        operands: Vec<NodeKey>,
    },
    #[format("`<` $0 `, ` $1 `>`")]
    Identical(NodeKey, NodeKey),
    #[format("`<` $subject `, ` $trait_name `>`")]
    Conforms {
        subject: NodeKey,
        trait_name: Text,
    },
    #[format("`<` $lifecycle `, ` $subject `>`")]
    Trivial {
        lifecycle: PayloadLifecycle,
        subject: NodeKey,
    },
    TypeShape(PayloadTy),
    #[format("`<[` vec($elements, CharSpace(`,`)) `], ` $index `>`")]
    Select {
        elements: Vec<PayloadTy>,
        index: NodeKey,
    },
    #[format("`<` $list `, ` $index `>`")]
    ListGet {
        list: NodeKey,
        index: NodeKey,
    },
    #[format("`<` $subject `, ` $query `>`")]
    Reflect {
        subject: NodeKey,
        query: PayloadReflect,
    },
    #[format("`<` $pack `, ` $query `>`")]
    PackQuery {
        pack: PayloadBinder,
        query: PayloadPackQuery,
    },
    #[format("`<` $kind `, ` $token `>`")]
    Hole {
        kind: PayloadHole,
        token: u64,
    },
}

/// Save `expr` in `ctx`, children first.
pub fn import_param(ctx: &mut Context, expr: &ParamExpr) -> Result<NodeKey, A1Error> {
    let node = match expr.kind() {
        ParamKind::Constant(value) => PayloadNode::Constant(import_value(ctx, value)?),
        ParamKind::DeclRef(reference) => PayloadNode::DeclRef(PayloadBinder::from_ref(reference)),
        ParamKind::IndexRef { depth, index } => PayloadNode::IndexRef {
            depth: *depth,
            index: *index,
        },
        ParamKind::Op { op, operands } => PayloadNode::Op {
            op: PayloadOp::from_op(*op),
            operands: operands
                .iter()
                .map(|operand| import_param(ctx, operand))
                .collect::<Result<Vec<_>, _>>()?,
        },
        ParamKind::Identical(left, right) => {
            PayloadNode::Identical(import_param(ctx, left)?, import_param(ctx, right)?)
        }
        ParamKind::Conforms {
            subject,
            trait_name,
        } => PayloadNode::Conforms {
            subject: import_param(ctx, subject)?,
            trait_name: trait_name.into(),
        },
        ParamKind::Trivial { lifecycle, subject } => PayloadNode::Trivial {
            lifecycle: match lifecycle {
                TrivialLifecycle::Movable => PayloadLifecycle::Movable,
                TrivialLifecycle::Copyable => PayloadLifecycle::Copyable,
                TrivialLifecycle::Deinitable => PayloadLifecycle::Deinitable,
            },
            subject: import_param(ctx, subject)?,
        },
        ParamKind::TypeShape(ty) => PayloadNode::TypeShape(import_payload_type(ctx, ty)?),
        ParamKind::Select { elements, index } => PayloadNode::Select {
            elements: elements
                .iter()
                .map(|ty| import_payload_type(ctx, ty))
                .collect::<Result<Vec<_>, _>>()?,
            index: import_param(ctx, index)?,
        },
        ParamKind::ListGet { list, index } => PayloadNode::ListGet {
            list: import_param(ctx, list)?,
            index: import_param(ctx, index)?,
        },
        ParamKind::Reflect { subject, query } => PayloadNode::Reflect {
            subject: import_param(ctx, subject)?,
            query: match query {
                ReflectQuery::IsStruct => PayloadReflect::IsStruct,
                ReflectQuery::FieldCount => PayloadReflect::FieldCount,
                ReflectQuery::FieldNames => PayloadReflect::FieldNames,
                ReflectQuery::FieldTypes => PayloadReflect::FieldTypes,
                ReflectQuery::FieldIndex(name) => PayloadReflect::FieldIndex(name.into()),
                ReflectQuery::FieldNamed(name) => PayloadReflect::FieldNamed(name.into()),
            },
        },
        ParamKind::PackQuery { pack, query } => PayloadNode::PackQuery {
            pack: PayloadBinder::from_ref(pack),
            query: match query {
                PackQuery::Length => PayloadPackQuery::Length,
                PackQuery::Conforms(name) => PayloadPackQuery::Conforms(name.into()),
                PackQuery::Contains(element) => {
                    PayloadPackQuery::Contains(import_param(ctx, element)?)
                }
                PackQuery::Predicate { .. } => {
                    return Err(outside("a pack predicate query"));
                }
            },
        },
        ParamKind::Hole { kind, token } => PayloadNode::Hole {
            kind: match kind {
                HoleKind::Unknown => PayloadHole::Unknown,
                HoleKind::Unbound => PayloadHole::Unbound,
            },
            token: *token,
        },
    };
    let meta = import_meta(ctx, expr.meta())?;
    Ok(NodeKey(uniqued_any::save(
        ctx,
        ParamExprPayload { meta, node },
    )))
}

/// Save the closed compile-time value `value` as a constant node.
pub fn import_constant(ctx: &mut Context, value: &CtValue) -> Result<NodeKey, A1Error> {
    let node = PayloadNode::Constant(import_value(ctx, value)?);
    let meta = import_meta(ctx, &MetaTy::of_value(value))?;
    Ok(NodeKey(uniqued_any::save(
        ctx,
        ParamExprPayload { meta, node },
    )))
}

/// The closed compile-time value the constant node `key` names.
pub fn export_constant(ctx: &Context, key: NodeKey) -> Result<CtValue, A1Error> {
    match &key.payload(ctx).node {
        PayloadNode::Constant(value) if is_executable(ctx, key) => export_value(ctx, value),
        _ => Err(A1Error::new(
            A1ErrorKind::Export,
            "a nominal type argument that is no closed constant",
        )),
    }
}

/// Rebuild the expression `key` names through `params`' constructors.
pub fn export_param(
    ctx: &Context,
    params: &ParamContext,
    key: NodeKey,
) -> Result<ParamExpr, A1Error> {
    let payload = key.payload(ctx);
    let meta = export_meta(ctx, &payload.meta)?;
    let built = |result: Result<ParamExpr, mojito_types::param_expr::ParamError>| {
        result.map_err(|error| A1Error::new(A1ErrorKind::Export, error.to_string()))
    };
    let child = |key: &NodeKey| export_param(ctx, params, *key);
    match &payload.node {
        PayloadNode::Constant(value) => built(params.constant(export_value(ctx, value)?)),
        PayloadNode::DeclRef(binder) => {
            let reference = binder.reference()?;
            Ok(params.decl_ref(reference.id, &reference.name, meta))
        }
        PayloadNode::IndexRef { depth, index } => Ok(params.index_ref(*depth, *index, meta)),
        PayloadNode::Op { op, operands } => {
            let operands = operands.iter().map(child).collect::<Result<Vec<_>, _>>()?;
            let rebuilt = built(params.op(op.op(), &operands))?;
            if same_node(ctx, key, &rebuilt) {
                return Ok(rebuilt);
            }
            Err(A1Error::new(
                A1ErrorKind::Export,
                format!(
                    "the front end's constructor rebuilds the stored `{:?}` node as `{rebuilt}`: an unfolded closed atom has no public constructor",
                    op.op()
                ),
            ))
        }
        PayloadNode::Identical(left, right) => Ok(params.identical(&child(left)?, &child(right)?)),
        PayloadNode::Conforms {
            subject,
            trait_name,
        } => built(params.conforms(&child(subject)?, trait_name.as_str())),
        PayloadNode::Trivial { lifecycle, subject } => {
            let lifecycle = match lifecycle {
                PayloadLifecycle::Movable => TrivialLifecycle::Movable,
                PayloadLifecycle::Copyable => TrivialLifecycle::Copyable,
                PayloadLifecycle::Deinitable => TrivialLifecycle::Deinitable,
            };
            built(params.trivial(lifecycle, &child(subject)?))
        }
        PayloadNode::TypeShape(ty) => Ok(params.type_shape(export_payload_type(ctx, ty)?)),
        PayloadNode::Select { elements, index } => {
            let elements = elements
                .iter()
                .map(|ty| export_payload_type(ctx, ty))
                .collect::<Result<Vec<_>, _>>()?;
            built(params.select(elements, &child(index)?))
        }
        PayloadNode::ListGet { list, index } => {
            built(params.list_get(&child(list)?, &child(index)?))
        }
        PayloadNode::Reflect { subject, query } => {
            let query = match query {
                PayloadReflect::IsStruct => ReflectQuery::IsStruct,
                PayloadReflect::FieldCount => ReflectQuery::FieldCount,
                PayloadReflect::FieldNames => ReflectQuery::FieldNames,
                PayloadReflect::FieldTypes => ReflectQuery::FieldTypes,
                PayloadReflect::FieldIndex(name) => ReflectQuery::FieldIndex(name.0.clone()),
                PayloadReflect::FieldNamed(name) => ReflectQuery::FieldNamed(name.0.clone()),
            };
            Ok(params.reflect_query(&child(subject)?, query))
        }
        PayloadNode::PackQuery { pack, query } => {
            let query = match query {
                PayloadPackQuery::Length => PackQuery::Length,
                PayloadPackQuery::Conforms(name) => PackQuery::Conforms(name.0.clone()),
                PayloadPackQuery::Contains(element) => PackQuery::Contains(child(element)?),
            };
            Ok(params.pack_query(&pack.reference()?, query))
        }
        PayloadNode::Hole { kind, .. } => {
            let kind = match kind {
                PayloadHole::Unknown => HoleKind::Unknown,
                PayloadHole::Unbound => HoleKind::Unbound,
            };
            Ok(params.hole(kind, meta))
        }
    }
}

/// Whether `expr` is the node `key` names, operator by operator. Leaves
/// are compared by kind; their values were rebuilt from the payload.
fn same_node(ctx: &Context, key: NodeKey, expr: &ParamExpr) -> bool {
    match (&key.payload(ctx).node, expr.kind()) {
        (
            PayloadNode::Op { op, operands },
            ParamKind::Op {
                op: rebuilt,
                operands: rebuilt_operands,
            },
        ) => {
            op.op() == *rebuilt
                && operands.len() == rebuilt_operands.len()
                && operands
                    .iter()
                    .zip(rebuilt_operands)
                    .all(|(key, expr)| same_node(ctx, *key, expr))
        }
        (PayloadNode::Op { .. }, _) | (_, ParamKind::Op { .. }) => false,
        (PayloadNode::Constant(_), kind) => matches!(kind, ParamKind::Constant(_)),
        (_, ParamKind::Constant(_)) => false,
        _ => true,
    }
}

/// Clone the node `key` names from `source` into `target` by value: the
/// payload is looked up, its children are cloned first, and nothing of
/// `source`'s keys survives in the result.
pub fn clone_param(
    source: &Context,
    key: NodeKey,
    target: &mut Context,
) -> Result<NodeKey, A1Error> {
    let payload = key.payload(source);
    let child = |key: &NodeKey, target: &mut Context| clone_param(source, *key, target);
    let node = match &payload.node {
        PayloadNode::Constant(value) => PayloadNode::Constant(clone_value(source, value, target)?),
        PayloadNode::DeclRef(binder) => PayloadNode::DeclRef(binder.clone()),
        PayloadNode::IndexRef { depth, index } => PayloadNode::IndexRef {
            depth: *depth,
            index: *index,
        },
        PayloadNode::Op { op, operands } => PayloadNode::Op {
            op: *op,
            operands: operands
                .iter()
                .map(|key| child(key, target))
                .collect::<Result<Vec<_>, _>>()?,
        },
        PayloadNode::Identical(left, right) => {
            PayloadNode::Identical(child(left, target)?, child(right, target)?)
        }
        PayloadNode::Conforms {
            subject,
            trait_name,
        } => PayloadNode::Conforms {
            subject: child(subject, target)?,
            trait_name: trait_name.clone(),
        },
        PayloadNode::Trivial { lifecycle, subject } => PayloadNode::Trivial {
            lifecycle: *lifecycle,
            subject: child(subject, target)?,
        },
        PayloadNode::TypeShape(ty) => PayloadNode::TypeShape(clone_type(source, ty, target)?),
        PayloadNode::Select { elements, index } => PayloadNode::Select {
            elements: elements
                .iter()
                .map(|ty| clone_type(source, ty, target))
                .collect::<Result<Vec<_>, _>>()?,
            index: child(index, target)?,
        },
        PayloadNode::ListGet { list, index } => PayloadNode::ListGet {
            list: child(list, target)?,
            index: child(index, target)?,
        },
        PayloadNode::Reflect { subject, query } => PayloadNode::Reflect {
            subject: child(subject, target)?,
            query: query.clone(),
        },
        PayloadNode::PackQuery { pack, query } => PayloadNode::PackQuery {
            pack: pack.clone(),
            query: match query {
                PayloadPackQuery::Contains(element) => {
                    PayloadPackQuery::Contains(child(element, target)?)
                }
                other => other.clone(),
            },
        },
        PayloadNode::Hole { kind, token } => PayloadNode::Hole {
            kind: *kind,
            token: *token,
        },
    };
    let meta = clone_meta(source, &payload.meta, target)?;
    Ok(NodeKey(uniqued_any::save(
        target,
        ParamExprPayload { meta, node },
    )))
}

/// Whether the node `key` names may sit in executable core: closed, with
/// no hole, no residual reference, and no symbolic type.
pub fn is_executable(ctx: &Context, key: NodeKey) -> bool {
    let payload = key.payload(ctx);
    let closed_type = |ty: &PayloadTy| matches!(ty, PayloadTy::Core(_));
    match &payload.node {
        PayloadNode::Constant(PayloadValue::Type(ty)) => closed_type(ty),
        PayloadNode::Constant(_) => true,
        PayloadNode::Op { operands, .. } => operands.iter().all(|key| is_executable(ctx, *key)),
        PayloadNode::Identical(left, right) => {
            is_executable(ctx, *left) && is_executable(ctx, *right)
        }
        PayloadNode::DeclRef(_)
        | PayloadNode::IndexRef { .. }
        | PayloadNode::Conforms { .. }
        | PayloadNode::Trivial { .. }
        | PayloadNode::TypeShape(_)
        | PayloadNode::Select { .. }
        | PayloadNode::ListGet { .. }
        | PayloadNode::Reflect { .. }
        | PayloadNode::PackQuery { .. }
        | PayloadNode::Hole { .. } => false,
    }
}

fn outside(what: &str) -> A1Error {
    A1Error::new(
        A1ErrorKind::UnsupportedType,
        format!("{what} is outside the parameter payload"),
    )
}

fn import_meta(ctx: &mut Context, meta: &MetaTy) -> Result<PayloadMeta, A1Error> {
    let list = |ctx: &mut Context, elements: &[MetaTy]| {
        elements
            .iter()
            .map(|element| import_meta(ctx, element))
            .collect::<Result<Vec<_>, _>>()
    };
    Ok(match meta {
        MetaTy::Value(ty) => PayloadMeta::Value(import_payload_type(ctx, ty)?),
        MetaTy::Type => PayloadMeta::Type,
        MetaTy::ReflectedType => PayloadMeta::ReflectedType,
        MetaTy::Tuple(elements) => PayloadMeta::Tuple(list(ctx, elements)?),
        MetaTy::List(elements) => PayloadMeta::List(list(ctx, elements)?),
        MetaTy::Set(elements) => PayloadMeta::Set(list(ctx, elements)?),
        MetaTy::Dict(entries) => {
            let mut flat = Vec::new();
            for (key, value) in entries {
                flat.push(import_meta(ctx, key)?);
                flat.push(import_meta(ctx, value)?);
            }
            PayloadMeta::Dict(flat)
        }
        MetaTy::ParamList(element) => PayloadMeta::ParamList(vec![import_meta(ctx, element)?]),
    })
}

fn export_meta(ctx: &Context, meta: &PayloadMeta) -> Result<MetaTy, A1Error> {
    let list = |elements: &[PayloadMeta]| {
        elements
            .iter()
            .map(|element| export_meta(ctx, element))
            .collect::<Result<Vec<_>, _>>()
    };
    Ok(match meta {
        PayloadMeta::Value(ty) => MetaTy::value(export_payload_type(ctx, ty)?),
        PayloadMeta::Type => MetaTy::Type,
        PayloadMeta::ReflectedType => MetaTy::ReflectedType,
        PayloadMeta::Tuple(elements) => MetaTy::Tuple(list(elements)?),
        PayloadMeta::List(elements) => MetaTy::List(list(elements)?),
        PayloadMeta::Set(elements) => MetaTy::Set(list(elements)?),
        PayloadMeta::Dict(flat) => {
            if flat.len() % 2 != 0 {
                return Err(A1Error::new(
                    A1ErrorKind::Export,
                    "a dictionary meta-type with a key and no value",
                ));
            }
            MetaTy::Dict(
                flat.chunks(2)
                    .map(|pair| Ok((export_meta(ctx, &pair[0])?, export_meta(ctx, &pair[1])?)))
                    .collect::<Result<Vec<_>, A1Error>>()?,
            )
        }
        PayloadMeta::ParamList(element) => match element.as_slice() {
            [element] => MetaTy::ParamList(Box::new(export_meta(ctx, element)?)),
            _ => {
                return Err(A1Error::new(
                    A1ErrorKind::Export,
                    "a parameter list has one element meta-type",
                ));
            }
        },
    })
}

fn clone_meta(
    source: &Context,
    meta: &PayloadMeta,
    target: &mut Context,
) -> Result<PayloadMeta, A1Error> {
    let mut list = |elements: &[PayloadMeta]| {
        elements
            .iter()
            .map(|element| clone_meta(source, element, target))
            .collect::<Result<Vec<_>, _>>()
    };
    Ok(match meta {
        PayloadMeta::Value(ty) => PayloadMeta::Value(clone_type(source, ty, target)?),
        PayloadMeta::Type => PayloadMeta::Type,
        PayloadMeta::ReflectedType => PayloadMeta::ReflectedType,
        PayloadMeta::Tuple(elements) => PayloadMeta::Tuple(list(elements)?),
        PayloadMeta::List(elements) => PayloadMeta::List(list(elements)?),
        PayloadMeta::Set(elements) => PayloadMeta::Set(list(elements)?),
        PayloadMeta::Dict(elements) => PayloadMeta::Dict(list(elements)?),
        PayloadMeta::ParamList(elements) => PayloadMeta::ParamList(list(elements)?),
    })
}

fn import_payload_type(ctx: &mut Context, ty: &Ty) -> Result<PayloadTy, A1Error> {
    match ty {
        Ty::Param {
            binder,
            bounds,
            callable_bound: None,
        } => Ok(PayloadTy::Param {
            binder: PayloadBinder::from_ref(binder),
            bounds: bounds.iter().map(Text::from).collect(),
        }),
        Ty::Func {
            environment,
            params,
            names,
            ret,
            required,
            variadic: None,
            kw_variadic: None,
            positional_only: None,
            keyword_only: None,
            raises,
            error: None,
            conventions,
            ref_params,
            ref_return: None,
            transfers,
        } if *environment == CallableEnvironment::default()
            && ref_params.iter().all(Option::is_none)
            && [names.len(), required.len(), conventions.len()]
                .iter()
                .all(|length| *length == params.len()) =>
        {
            let params = (0..params.len())
                .map(|index| {
                    Ok(PayloadParam {
                        name: Text::from(&names[index]),
                        ty: import_payload_type(ctx, &params[index])?,
                        required: required[index],
                        convention: conventions[index].map(CoreConvention::from_convention),
                    })
                })
                .collect::<Result<Vec<_>, A1Error>>()?;
            Ok(PayloadTy::Callable(PayloadCallable {
                params,
                ret: vec![import_payload_type(ctx, ret)?],
                raises: *raises,
                transfers: transfers
                    .iter()
                    .map(import_transfer)
                    .collect::<Result<Vec<_>, _>>()?,
            }))
        }
        other => import_type(ctx, other).map(PayloadTy::Core),
    }
}

fn export_payload_type(ctx: &Context, ty: &PayloadTy) -> Result<Ty, A1Error> {
    match ty {
        PayloadTy::Core(ty) => export_type(ctx, *ty),
        PayloadTy::Param { binder, bounds } => Ok(Ty::Param {
            binder: binder.reference()?,
            bounds: bounds.iter().map(|bound| bound.0.clone()).collect(),
            callable_bound: None,
        }),
        PayloadTy::Callable(callable) => {
            let [ret] = callable.ret.as_slice() else {
                return Err(A1Error::new(
                    A1ErrorKind::Export,
                    "a callable has one result type",
                ));
            };
            let params = &callable.params;
            Ok(Ty::Func {
                environment: CallableEnvironment::default(),
                params: params
                    .iter()
                    .map(|param| export_payload_type(ctx, &param.ty))
                    .collect::<Result<Vec<_>, _>>()?,
                names: params.iter().map(|param| param.name.0.clone()).collect(),
                ret: Box::new(export_payload_type(ctx, ret)?),
                required: params.iter().map(|param| param.required).collect(),
                variadic: None,
                kw_variadic: None,
                positional_only: None,
                keyword_only: None,
                raises: callable.raises,
                error: None,
                conventions: params
                    .iter()
                    .map(|param| param.convention.map(CoreConvention::convention))
                    .collect(),
                ref_params: Box::new(params.iter().map(|_| None).collect()),
                ref_return: None,
                transfers: TransferSet(
                    callable
                        .transfers
                        .iter()
                        .map(export_transfer)
                        .collect::<Result<Vec<_>, _>>()?,
                ),
            })
        }
    }
}

fn clone_type(
    source: &Context,
    ty: &PayloadTy,
    target: &mut Context,
) -> Result<PayloadTy, A1Error> {
    Ok(match ty {
        PayloadTy::Core(handle) => {
            let checked = export_type(source, *handle)?;
            PayloadTy::Core(import_type(target, &checked)?)
        }
        PayloadTy::Param { binder, bounds } => PayloadTy::Param {
            binder: binder.clone(),
            bounds: bounds.clone(),
        },
        PayloadTy::Callable(callable) => PayloadTy::Callable(PayloadCallable {
            params: callable
                .params
                .iter()
                .map(|param| {
                    Ok(PayloadParam {
                        name: param.name.clone(),
                        ty: clone_type(source, &param.ty, target)?,
                        required: param.required,
                        convention: param.convention,
                    })
                })
                .collect::<Result<Vec<_>, A1Error>>()?,
            ret: callable
                .ret
                .iter()
                .map(|ty| clone_type(source, ty, target))
                .collect::<Result<Vec<_>, _>>()?,
            raises: callable.raises,
            transfers: callable.transfers.clone(),
        }),
    })
}

fn import_value(ctx: &mut Context, value: &CtValue) -> Result<PayloadValue, A1Error> {
    Ok(match value {
        CtValue::Int(value) => PayloadValue::Int(*value),
        CtValue::UInt(value) => PayloadValue::UInt(*value),
        CtValue::IntLiteral(literal) => {
            PayloadValue::IntLiteral(literal.as_bigint().to_string().into())
        }
        CtValue::Bool(value) => PayloadValue::Bool(*value),
        CtValue::Str(value) => PayloadValue::Str(value.into()),
        CtValue::Dtype(dtype) => PayloadValue::Dtype(CoreDtype::from_dtype(*dtype)),
        CtValue::Type(ty) => PayloadValue::Type(import_payload_type(ctx, ty)?),
        other => return Err(outside(&format!("the constant `{other:?}`"))),
    })
}

fn export_value(ctx: &Context, value: &PayloadValue) -> Result<CtValue, A1Error> {
    Ok(match value {
        PayloadValue::Int(value) => CtValue::Int(*value),
        PayloadValue::UInt(value) => CtValue::UInt(*value),
        PayloadValue::IntLiteral(digits) => {
            CtValue::IntLiteral(IntLiteral::parse_radix(digits.as_str(), 10).ok_or_else(|| {
                A1Error::new(
                    A1ErrorKind::Export,
                    format!("`{digits}` is not a decimal integer literal"),
                )
            })?)
        }
        PayloadValue::Bool(value) => CtValue::Bool(*value),
        PayloadValue::Str(value) => CtValue::Str(value.0.clone()),
        PayloadValue::Dtype(dtype) => CtValue::Dtype(dtype.dtype()),
        PayloadValue::Type(ty) => CtValue::Type(Box::new(export_payload_type(ctx, ty)?)),
    })
}

fn clone_value(
    source: &Context,
    value: &PayloadValue,
    target: &mut Context,
) -> Result<PayloadValue, A1Error> {
    Ok(match value {
        PayloadValue::Type(ty) => PayloadValue::Type(clone_type(source, ty, target)?),
        other => other.clone(),
    })
}

fn import_transfer(effect: &TransferEffect) -> Result<PayloadTransfer, A1Error> {
    Ok(PayloadTransfer {
        dest: import_sig_origin(&effect.dest)?,
        src: import_sig_origin(&effect.src)?,
        src_is_place: effect.src_is_place,
        mutable: effect.mutable,
    })
}

fn export_transfer(effect: &PayloadTransfer) -> Result<TransferEffect, A1Error> {
    Ok(TransferEffect {
        dest: export_sig_origin(effect.dest)?,
        src: export_sig_origin(effect.src)?,
        src_is_place: effect.src_is_place,
        mutable: effect.mutable,
    })
}

fn import_sig_origin(origin: &SigOrigin) -> Result<PayloadSigOrigin, A1Error> {
    match origin {
        SigOrigin::Self_ => Ok(PayloadSigOrigin::Receiver),
        SigOrigin::Param(index) => Ok(PayloadSigOrigin::Param(*index as u64)),
        SigOrigin::Static => Ok(PayloadSigOrigin::Static),
        other => Err(outside(&format!("the signature origin `{other:?}`"))),
    }
}

fn export_sig_origin(origin: PayloadSigOrigin) -> Result<SigOrigin, A1Error> {
    Ok(match origin {
        PayloadSigOrigin::Receiver => SigOrigin::Self_,
        PayloadSigOrigin::Param(index) => SigOrigin::Param(
            usize::try_from(index)
                .map_err(|_| A1Error::new(A1ErrorKind::Export, "a parameter index beyond usize"))?,
        ),
        PayloadSigOrigin::Static => SigOrigin::Static,
    })
}
