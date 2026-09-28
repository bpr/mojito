//! Verification of `mojito_core`: the per-operation rules the registry
//! dispatches to, the effect chain of each block, and the legality of a
//! whole module at a conversion boundary.

use pliron::attribute::Attribute;
use pliron::basic_block::BasicBlock;
use pliron::builtin::ops::ModuleOp;
use pliron::context::{Context, Ptr};
use pliron::identifier::Identifier;
use pliron::linked_list::ContainsLinkedList;
use pliron::location::Located;
use pliron::operation::Operation;
use pliron::region::Region;
use pliron::result::Result;
use pliron::r#type::{TypeHandle, Typed};
use pliron::value::Value;
use pliron::verify_err;

use super::attrs::{
    CallAttr, CleanupAttr, ConstAttr, CoreCallKind, CoreLifecycle, CoreRole, CoreStepKind,
    CoreStorage, CoreSubscriptCall, DeadTermAttr, ExitAttr, IdentityAttr, IndexAttr, InfixAttr,
    InvalidateAttr, IterInitAttr, IterNextAttr, LifecycleAttr, LoansAttr, ModuleAttr, MultiSetAttr,
    OrphansAttr, OutcomeAttr, OutcomeKind, PointerStorageAttr, PrefixAttr, ProjectionAttr,
    ProvenanceAttr, RegAttr, ResolvedAttr, SignatureAttr, SimdConvertAttr, SlotAttr, StoreAttr,
    TryAttr, UseModeAttr,
};
use super::inventory::{CoreOpKind, EffectClass, Stage};
use super::ops::{
    KEY_CALL, KEY_CLEANUP, KEY_CONSTANT, KEY_DEAD_TERM, KEY_EXIT, KEY_IDENTITY, KEY_INFIX,
    KEY_INVALIDATION, KEY_ITER_INIT, KEY_ITER_NEXT, KEY_LIFECYCLE, KEY_LOANS, KEY_MULTI_SET,
    KEY_ORPHANS, KEY_OUTCOME, KEY_POINTER_STORAGE, KEY_PREFIX, KEY_PROJECTION, KEY_PROVENANCE,
    KEY_REG, KEY_RESOLVED, KEY_SIGNATURE, KEY_SIMD_CONVERT, KEY_SLOT, KEY_STORE, KEY_SUBSCRIPT,
    KEY_TABLES, KEY_TRY, KEY_USE_MODE,
};
use super::types::{
    BoolType, EffectType, ErrorType, IntLiteralType, IntType, NominalType, NoneType, OutcomeType,
    PlaceType, PointerType, RefType, SimdType, StringLiteralType, UIntType,
};
use super::{A1Error, A1ErrorKind};

/// The builtin callees a direct call may name without a declaration.
pub const BUILTIN_CALLEES: [&str; 10] = [
    "print",
    "Error",
    "Int",
    "UInt",
    "String",
    "len",
    "external_call",
    "_mojito_abort",
    "UnsafePointer.alloc",
    "UnsafePointer.alloc_aligned",
];

/// The methods of compiler-private values a method call may name without
/// a checker-selected symbol.
pub const BUILTIN_METHODS: [&str; 6] = [
    "byte_length",
    "copy",
    "__hash__",
    "ptr",
    "unsafe_free",
    "write",
];

/// Verify `op` against the registry rule of `kind`.
pub fn verify_core_op(ctx: &Context, op: Ptr<Operation>, kind: CoreOpKind) -> Result<()> {
    let shape = Shape::of(ctx, op);
    shape.require_attr::<IdentityAttr>(ctx, &KEY_IDENTITY, "identity")?;
    shape.require_attr::<ProvenanceAttr>(ctx, &KEY_PROVENANCE, "provenance")?;
    if kind.effect() != EffectClass::Terminator {
        shape.successors(0)?;
    }
    match kind.effect() {
        EffectClass::Pure | EffectClass::Structural => shape.forbid_effects(ctx)?,
        EffectClass::Effectful => {
            shape.effect_operand_last(ctx)?;
            shape.effect_result_last(ctx)?;
        }
        EffectClass::Terminator => {
            shape.effect_operand_last(ctx)?;
            shape.results(0)?;
        }
    }
    match kind {
        CoreOpKind::Func => verify_func(ctx, &shape),
        CoreOpKind::Slot => verify_slot(ctx, &shape),
        CoreOpKind::Const => verify_const(ctx, &shape),
        CoreOpKind::Materialize => {
            shape.operands(1)?;
            shape.value_result(ctx)?;
            shape.require(
                shape.operand_types[0].deref(ctx).is::<IntLiteralType>(),
                "materialize reads an exact literal",
            )?;
            shape.require(
                !shape.result_types[0].deref(ctx).is::<IntLiteralType>(),
                "materialize yields a machine scalar, not a literal",
            )
        }
        CoreOpKind::Binary => {
            shape.operands(2)?;
            shape.value_operands(ctx, 0..2)?;
            shape.value_result(ctx)?;
            shape.require_attr::<InfixAttr>(ctx, &KEY_INFIX, "infix")?;
            shape
                .require_attr::<ResolvedAttr>(ctx, &KEY_RESOLVED, "resolved")
                .map(drop)
        }
        CoreOpKind::SimdMake => verify_simd_make(ctx, &shape),
        CoreOpKind::Unary => {
            shape.operands(1)?;
            shape.value_operands(ctx, 0..1)?;
            shape.value_result(ctx)?;
            shape
                .require_attr::<PrefixAttr>(ctx, &KEY_PREFIX, "prefix")
                .map(drop)
        }
        CoreOpKind::SimdConvert => {
            shape.operands(1)?;
            shape.value_result(ctx)?;
            shape.require_attr::<SimdConvertAttr>(ctx, &KEY_SIMD_CONVERT, "conversion")?;
            shape.value_operands(ctx, 0..1)?;
            shape.require(
                shape.result_types[0].deref(ctx).is::<SimdType>(),
                "simd_convert yields a SIMD value",
            )
        }
        CoreOpKind::Use => {
            shape.operands(2)?;
            shape.place_operand(ctx, 0)?;
            shape.results(2)?;
            shape.value_result_with_effect(ctx)?;
            shape
                .require_attr::<UseModeAttr>(ctx, &KEY_USE_MODE, "use mode")
                .map(drop)
        }
        CoreOpKind::Store => {
            shape.operands(3)?;
            shape.place_operand(ctx, 0)?;
            shape.value_operands(ctx, 1..2)?;
            shape.results(1)?;
            shape
                .require_attr::<StoreAttr>(ctx, &KEY_STORE, "store kind")
                .map(drop)
        }
        CoreOpKind::Project => verify_project(ctx, &shape),
        CoreOpKind::Load => {
            shape.operands(2)?;
            shape.place_operand(ctx, 0)?;
            shape.results(2)?;
            shape.value_result_with_effect(ctx)
        }
        CoreOpKind::RefMake => {
            shape.operands(1)?;
            shape.place_operand(ctx, 0)?;
            shape.value_result(ctx)?;
            shape.require(
                shape.result_types[0].deref(ctx).is::<RefType>(),
                "ref_make yields a reference",
            )
        }
        CoreOpKind::RefStore => {
            shape.operands(3)?;
            shape.place_operand(ctx, 0)?;
            shape.results(1)?;
            shape.require(
                shape.operand_types[1].deref(ctx).is::<RefType>(),
                "ref_store initializes storage with a reference handle",
            )
        }
        CoreOpKind::RefRead | CoreOpKind::Copy => {
            shape.operands(2)?;
            shape.value_operands(ctx, 0..1)?;
            shape.results(2)?;
            shape.value_result_with_effect(ctx)
        }
        CoreOpKind::Move => {
            shape.operands(2)?;
            shape.place_operand(ctx, 0)?;
            shape.results(2)?;
            shape.value_result_with_effect(ctx)
        }
        CoreOpKind::KeepAlive => {
            shape.operands(2)?;
            shape.place_operand(ctx, 0)?;
            shape.results(1)?;
            shape.require(
                variable_slot(ctx, shape.operand_values[0]),
                "keep_alive names a variable slot",
            )
        }
        CoreOpKind::Index => verify_index(ctx, &shape),
        CoreOpKind::MultiSet => verify_multi_set(ctx, &shape),
        CoreOpKind::PointerStorage => {
            shape.operands(3)?;
            shape.value_operands(ctx, 0..2)?;
            shape.results(2)?;
            shape.value_result_with_effect(ctx)?;
            shape.require_attr::<PointerStorageAttr>(ctx, &KEY_POINTER_STORAGE, "access")?;
            shape.require(
                shape.operand_types[0].deref(ctx).is::<PointerType>(),
                "pointer_storage reads through a pointer",
            )
        }
        CoreOpKind::IterInit => {
            shape.operands(3)?;
            shape.results(1)?;
            shape.require_attr::<IterInitAttr>(ctx, &KEY_ITER_INIT, "iteration")?;
            shape.require(
                (0..2).all(|index| variable_slot(ctx, shape.operand_values[index])),
                "iter_init names the source and iterator variable slots",
            )
        }
        CoreOpKind::IterNext => {
            shape.operands(2)?;
            shape.results(3)?;
            shape.value_result_with_effect(ctx)?;
            shape.require_attr::<IterNextAttr>(ctx, &KEY_ITER_NEXT, "iterator call")?;
            shape.require(
                variable_slot(ctx, shape.operand_values[0]),
                "iter_next names the iterator variable slot",
            )?;
            shape.require(
                shape.result_types[1].deref(ctx).is::<BoolType>(),
                "iter_next yields the element, then whether there was one",
            )
        }
        CoreOpKind::Call => {
            shape.results(2)?;
            shape.value_result_with_effect(ctx)?;
            shape.successors(0)?;
            verify_call_operands(ctx, &shape).map(drop)
        }
        CoreOpKind::Invoke => verify_invoke(ctx, &shape),
        CoreOpKind::Drop => verify_lifecycle(
            ctx,
            &shape,
            &[
                CoreLifecycle::DropVar,
                CoreLifecycle::DropPlace,
                CoreLifecycle::DropReg,
            ],
        ),
        CoreOpKind::Consume => verify_lifecycle(
            ctx,
            &shape,
            &[CoreLifecycle::ConsumeVar, CoreLifecycle::ConsumePlace],
        ),
        CoreOpKind::Loans => verify_loans(ctx, &shape),
        CoreOpKind::Invalidate => {
            shape.operands(1)?;
            shape.results(2)?;
            shape.marker_result(ctx)?;
            shape
                .require_attr::<InvalidateAttr>(ctx, &KEY_INVALIDATION, "invalidation")
                .map(drop)
        }
        CoreOpKind::Br => {
            shape.successors(1)?;
            shape.forwards(ctx, 0, &shape.operand_types)
        }
        CoreOpKind::CondBr => {
            shape.operands(2)?;
            shape.successors(2)?;
            shape.require(
                shape.operand_types[0].deref(ctx).is::<BoolType>(),
                "cond_br tests a Bool",
            )?;
            let forwarded = &shape.operand_types[1..];
            shape.forwards(ctx, 0, forwarded)?;
            shape.forwards(ctx, 1, forwarded)
        }
        CoreOpKind::Return => {
            shape.successors(0)?;
            let cleanup = attr::<CleanupAttr>(ctx, op, &KEY_CLEANUP)
                .and_then(|cleanup| usize::try_from(cleanup.0).ok())
                .unwrap_or(0);
            let values = (shape.operand_types.len() - 1).checked_sub(cleanup);
            let Some(values @ (0 | 1)) = values else {
                return shape.fail(
                    ctx,
                    "return takes an optional value, its cleanup slots, and the effect token",
                );
            };
            shape.value_operands(ctx, 0..values)?;
            shape.require(
                (values..values + cleanup)
                    .all(|index| variable_slot(ctx, shape.operand_values[index])),
                "return carries variable slots as cleanup",
            )
        }
        CoreOpKind::Raise => verify_raise(ctx, &shape),
        CoreOpKind::Outcome => verify_outcome(ctx, &shape),
        CoreOpKind::Resume => verify_resume(ctx, &shape),
        CoreOpKind::TryBridge => verify_try_bridge(ctx, &shape),
        CoreOpKind::RegionExit => {
            shape.operands(1)?;
            shape.successors(0)?;
            shape.require_attr::<ExitAttr>(ctx, &KEY_EXIT, "exit kind")?;
            shape.require(
                enclosing_kind(ctx, op) == Some(CoreOpKind::TryBridge),
                "region_exit ends a block of a structured try region",
            )
        }
    }
}

/// The attribute of type `T` under `key`, cloned out of `op`.
pub fn attr<T: Attribute + Clone>(
    ctx: &Context,
    op: Ptr<Operation>,
    key: &Identifier,
) -> Option<T> {
    op.deref(ctx).attributes.get::<T>(key).cloned()
}

/// Every operation of `module` in nesting order: an operation, then the
/// operations of its regions.
pub fn walk(ctx: &Context, root: Ptr<Operation>) -> Vec<Ptr<Operation>> {
    let mut out = Vec::new();
    let mut pending = vec![root];
    while let Some(op) = pending.pop() {
        out.push(op);
        let mut nested = Vec::new();
        for region in op.deref(ctx).regions() {
            for block in region.deref(ctx).iter(ctx) {
                nested.extend(block.deref(ctx).iter(ctx));
            }
        }
        pending.extend(nested.into_iter().rev());
    }
    out
}

/// Run Pliron's verifier and the core legality inspection at `stage`.
pub fn verify_module(
    ctx: &Context,
    module: Ptr<Operation>,
    stage: Stage,
) -> std::result::Result<(), A1Error> {
    pliron::operation::verify_operation(module, ctx).map_err(|error| {
        A1Error::new(
            A1ErrorKind::Verification,
            pliron::printable::Printable::disp(&error, ctx).to_string(),
        )
    })?;
    check_legality(ctx, module, stage)
}

/// The legality of `module` at `stage`: only the approved container and
/// registered operations legal there remain, every type is a core type,
/// and every symbol resolves.
pub fn check_legality(
    ctx: &Context,
    module: Ptr<Operation>,
    stage: Stage,
) -> std::result::Result<(), A1Error> {
    match legality_violations(ctx, module, stage).into_iter().next() {
        Some(violation) => Err(violation),
        None => Ok(()),
    }
}

/// Every violation of `module`'s legality at `stage`, in operation order.
pub fn legality_violations(ctx: &Context, module: Ptr<Operation>, stage: Stage) -> Vec<A1Error> {
    let illegal = |message: String| A1Error::new(A1ErrorKind::Legality, message);
    if !Operation::is_op::<ModuleOp>(module, ctx) {
        return vec![illegal("the container is not `builtin.module`".into())];
    }
    let Some(tables) = attr::<ModuleAttr>(ctx, module, &KEY_TABLES) else {
        return vec![illegal("the module carries no tables".into())];
    };
    let mut violations = Vec::new();
    if tables.schema.as_str() != super::attrs::SCHEMA {
        violations.push(illegal(format!(
            "schema `{}` is not `{}`",
            tables.schema,
            super::attrs::SCHEMA
        )));
    }
    let operations = walk(ctx, module);
    let mut symbols = Vec::new();
    for op in operations.iter().skip(1) {
        let context = describe(ctx, *op);
        let operation = op.deref(ctx);
        let block_arguments = operation.regions().flat_map(|region| {
            region
                .deref(ctx)
                .iter(ctx)
                .flat_map(|block| block.deref(ctx).arguments().collect::<Vec<_>>())
                .collect::<Vec<_>>()
        });
        let foreign = operation
            .results()
            .chain(operation.operands())
            .chain(block_arguments)
            .any(|value| !is_core_type(ctx, value.get_type(ctx)));
        if foreign {
            violations.push(illegal(format!(
                "a value outside the core types ({context})"
            )));
        }
        let Some(kind) = CoreOpKind::of(ctx, *op) else {
            violations.push(illegal(format!(
                "operation `{}` is not registered in `mojito_core`",
                Operation::get_opid(*op, ctx)
            )));
            continue;
        };
        if !kind.legal_at(stage) {
            violations.push(illegal(format!(
                "`mojito_core.{}` is illegal at {stage:?} ({context})",
                kind.name()
            )));
        }
        if kind == CoreOpKind::Func {
            if operation.get_parent_op(ctx) != Some(module) {
                violations.push(illegal(format!("a nested function ({context})")));
            }
            if let Some(signature) = attr::<SignatureAttr>(ctx, *op, &KEY_SIGNATURE) {
                symbols.push(signature.symbol);
            }
        }
    }
    for entry in &tables.entries {
        if !symbols.contains(&entry.concrete) {
            violations.push(illegal(format!(
                "entry `{}` names no function of the module",
                entry.concrete
            )));
        }
    }
    for op in operations.iter().skip(1) {
        if let Err(violation) = check_projection(ctx, *op, &tables) {
            violations.push(violation);
        }
        if stage == Stage::ExecutableCore && CoreOpKind::of(ctx, *op) == Some(CoreOpKind::Func) {
            let checked = super::outcomes::FnPlan::build(ctx, *op)
                .and_then(|_| super::lifecycle::check_contract(ctx, *op));
            if let Err(violation) = checked {
                violations.push(violation);
            }
        }
        let Some(call) = attr::<CallAttr>(ctx, *op, &KEY_CALL) else {
            continue;
        };
        let target = call.resolved.as_ref().unwrap_or(&call.target).as_str();
        let intrinsic = call.kind == CoreCallKind::Method && call.resolved.is_none();
        let known = if intrinsic {
            BUILTIN_METHODS.contains(&target)
        } else {
            symbols.iter().any(|symbol| symbol.as_str() == target)
                || tables
                    .structs
                    .iter()
                    .any(|decl| decl.name.as_str() == target)
                || BUILTIN_CALLEES.contains(&target)
        };
        if !known {
            violations.push(illegal(format!(
                "call target `{target}` resolves to no function, struct, or builtin ({})",
                describe(ctx, *op)
            )));
        }
    }
    violations
}

/// Each field step of a projection names a field its nominal base
/// declares, at the type the declaration gives it.
fn check_projection(
    ctx: &Context,
    op: Ptr<Operation>,
    tables: &ModuleAttr,
) -> std::result::Result<(), A1Error> {
    let Some(projection) = attr::<ProjectionAttr>(ctx, op, &KEY_PROJECTION) else {
        return Ok(());
    };
    let Some(mut base) = projection.root_ty else {
        return Ok(());
    };
    for step in &projection.steps {
        if let CoreStepKind::Field(field) = &step.kind {
            let mut object = base;
            while let Some(reference) = object.deref(ctx).downcast_ref::<RefType>() {
                object = reference.referent;
            }
            let nominal = object
                .deref(ctx)
                .downcast_ref::<NominalType>()
                .map(|nominal| nominal.name.clone());
            let declared = nominal.as_ref().and_then(|name| {
                tables
                    .structs
                    .iter()
                    .find(|decl| decl.name == *name)
                    .and_then(|decl| decl.fields.iter().find(|entry| entry.name == *field))
            });
            if declared.map(|entry| entry.ty) != Some(step.ty) {
                return Err(A1Error::new(
                    A1ErrorKind::Legality,
                    format!(
                        "projection step `{field}` names no field of its base at that type ({})",
                        describe(ctx, op)
                    ),
                ));
            }
        }
        base = step.ty;
    }
    Ok(())
}

/// Whether `ty` belongs to the core vocabulary.
pub fn is_core_type(ctx: &Context, ty: TypeHandle) -> bool {
    let object = ty.deref(ctx);
    if let Some(place) = object.downcast_ref::<PlaceType>() {
        return is_core_type(ctx, place.target);
    }
    object.is::<EffectType>() || object.is::<OutcomeType>() || is_value_type(ctx, ty)
}

/// Whether `ty` is the type of a runtime or literal value.
pub fn is_value_type(ctx: &Context, ty: TypeHandle) -> bool {
    super::types::export_type(ctx, ty).is_ok()
}

/// The operation's stable identity, for diagnostics.
pub fn describe(ctx: &Context, op: Ptr<Operation>) -> String {
    attr::<IdentityAttr>(ctx, op, &KEY_IDENTITY).map_or_else(
        || format!("`{}` without identity", Operation::get_opid(op, ctx)),
        |identity| format!("`{}` at {}", Operation::get_opid(op, ctx), identity.key()),
    )
}

/// The registry kind of the operation whose region holds `op`.
pub fn enclosing_kind(ctx: &Context, op: Ptr<Operation>) -> Option<CoreOpKind> {
    op.deref(ctx)
        .get_parent_op(ctx)
        .and_then(|parent| CoreOpKind::of(ctx, parent))
}

#[derive(Debug)]
struct CoreVerifyError(String);

impl std::fmt::Display for CoreVerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CoreVerifyError {}

/// The operand, result, successor, and region shape of one operation.
struct Shape {
    op: Ptr<Operation>,
    context: String,
    operand_types: Vec<TypeHandle>,
    operand_values: Vec<Value>,
    result_types: Vec<TypeHandle>,
    successors: Vec<Ptr<BasicBlock>>,
    regions: Vec<Ptr<Region>>,
}

impl Shape {
    fn of(ctx: &Context, op: Ptr<Operation>) -> Self {
        let operation = op.deref(ctx);
        Self {
            op,
            context: describe(ctx, op),
            operand_types: operation
                .operands()
                .map(|value| value.get_type(ctx))
                .collect(),
            operand_values: operation.operands().collect(),
            result_types: operation.result_types().collect(),
            successors: operation.successors().collect(),
            regions: operation.regions().collect(),
        }
    }

    fn fail<T>(&self, ctx: &Context, message: impl std::fmt::Display) -> Result<T> {
        verify_err!(
            self.op.deref(ctx).loc(),
            CoreVerifyError(format!("{message} ({})", self.context))
        )
    }

    fn require(&self, holds: bool, message: &str) -> Result<()> {
        if holds {
            return Ok(());
        }
        verify_err!(
            pliron::location::Location::Unknown,
            CoreVerifyError(format!("{message} ({})", self.context))
        )
    }

    fn require_attr<T: Attribute + Clone>(
        &self,
        ctx: &Context,
        key: &Identifier,
        what: &str,
    ) -> Result<T> {
        match attr::<T>(ctx, self.op, key) {
            Some(value) => Ok(value),
            None => self.fail(ctx, format!("missing or mistyped {what} attribute")),
        }
    }

    fn operands(&self, count: usize) -> Result<()> {
        self.require(
            self.operand_types.len() == count,
            &format!(
                "expected {count} operands, found {}",
                self.operand_types.len()
            ),
        )
    }

    fn results(&self, count: usize) -> Result<()> {
        self.require(
            self.result_types.len() == count,
            &format!(
                "expected {count} results, found {}",
                self.result_types.len()
            ),
        )
    }

    fn successors(&self, count: usize) -> Result<()> {
        self.require(
            self.successors.len() == count,
            &format!(
                "expected {count} successors, found {}",
                self.successors.len()
            ),
        )
    }

    fn forbid_effects(&self, ctx: &Context) -> Result<()> {
        let effect = |ty: &TypeHandle| ty.deref(ctx).is::<EffectType>();
        self.require(
            !self.operand_types.iter().any(effect) && !self.result_types.iter().any(effect),
            "an operation outside the effect chain touches the effect token",
        )
    }

    fn effect_operand_last(&self, ctx: &Context) -> Result<()> {
        let effects = self
            .operand_types
            .iter()
            .filter(|ty| ty.deref(ctx).is::<EffectType>())
            .count();
        let last = self
            .operand_types
            .last()
            .is_some_and(|ty| ty.deref(ctx).is::<EffectType>());
        self.require(
            effects == 1 && last,
            "the effect token is the one, last operand",
        )
    }

    fn effect_result_last(&self, ctx: &Context) -> Result<()> {
        let effects = self
            .result_types
            .iter()
            .filter(|ty| ty.deref(ctx).is::<EffectType>())
            .count();
        let last = self
            .result_types
            .last()
            .is_some_and(|ty| ty.deref(ctx).is::<EffectType>());
        self.require(
            effects == 1 && last,
            "the effect token is the one, last result",
        )
    }

    fn place_operand(&self, ctx: &Context, index: usize) -> Result<()> {
        self.require(
            self.operand_types
                .get(index)
                .is_some_and(|ty| ty.deref(ctx).is::<PlaceType>()),
            &format!("operand {index} is a place"),
        )
    }

    fn value_operands(&self, ctx: &Context, range: std::ops::Range<usize>) -> Result<()> {
        for index in range {
            self.require(
                self.operand_types
                    .get(index)
                    .is_some_and(|ty| is_value_type(ctx, *ty)),
                &format!("operand {index} is a value"),
            )?;
        }
        Ok(())
    }

    /// One value result, carrying its register.
    fn value_result(&self, ctx: &Context) -> Result<()> {
        self.results(1)?;
        self.require(
            is_value_type(ctx, self.result_types[0]),
            "the result is a value",
        )?;
        self.require_attr::<RegAttr>(ctx, &KEY_REG, "register")
            .map(drop)
    }

    /// A value result carrying its register, then the effect token.
    fn value_result_with_effect(&self, ctx: &Context) -> Result<()> {
        self.require(
            self.result_types
                .first()
                .is_some_and(|ty| is_value_type(ctx, *ty)),
            "the first result is a value",
        )?;
        self.require_attr::<RegAttr>(ctx, &KEY_REG, "register")
            .map(drop)
    }

    /// A `None` marker result carrying its register, then the effect token.
    fn marker_result(&self, ctx: &Context) -> Result<()> {
        self.require(
            self.result_types
                .first()
                .is_some_and(|ty| ty.deref(ctx).is::<NoneType>()),
            "the marker result is None",
        )?;
        self.require_attr::<RegAttr>(ctx, &KEY_REG, "register")
            .map(drop)
    }

    /// Successor `index` takes exactly arguments of `types`.
    fn forwards(&self, ctx: &Context, index: usize, types: &[TypeHandle]) -> Result<()> {
        let arguments: Vec<TypeHandle> = self.successors[index]
            .deref(ctx)
            .arguments()
            .map(|value| value.get_type(ctx))
            .collect();
        self.require(
            arguments == types,
            &format!("successor {index} takes the forwarded operands, by type"),
        )
    }
}

fn verify_func(ctx: &Context, shape: &Shape) -> Result<()> {
    let signature: SignatureAttr = shape.require_attr(ctx, &KEY_SIGNATURE, "signature")?;
    shape.require_attr::<OrphansAttr>(ctx, &KEY_ORPHANS, "orphan register")?;
    shape.require(shape.regions.len() == 1, "a function has one region")?;
    let Some(entry) = shape.regions[0].deref(ctx).get_entry_block() else {
        return shape.fail(ctx, "a function has an entry block");
    };
    let arguments: Vec<TypeHandle> = entry
        .deref(ctx)
        .arguments()
        .map(|value| value.get_type(ctx))
        .collect();
    let Some((effect, parameters)) = arguments.split_last() else {
        return shape.fail(ctx, "the entry block takes the effect token");
    };
    shape.require(
        effect.deref(ctx).is::<EffectType>() && parameters == signature.params.as_slice(),
        "the entry block takes the signature's parameters, then the effect token",
    )?;
    for region in &shape.regions {
        verify_effect_chains(ctx, shape, *region)?;
    }
    Ok(())
}

/// Every block of `region`, and of the regions nested in it, threads one
/// effect chain from its last argument to its terminator.
fn verify_effect_chains(ctx: &Context, shape: &Shape, region: Ptr<Region>) -> Result<()> {
    for block in region.deref(ctx).iter(ctx) {
        let Some(mut token) = block.deref(ctx).arguments().last() else {
            return shape.fail(ctx, "a block takes no effect token");
        };
        if !token.get_type(ctx).deref(ctx).is::<EffectType>() {
            return shape.fail(ctx, "a block's last argument is not the effect token");
        }
        for op in block.deref(ctx).iter(ctx) {
            let Some(kind) = CoreOpKind::of(ctx, op) else {
                continue;
            };
            for nested in op.deref(ctx).regions() {
                verify_effect_chains(ctx, shape, nested)?;
            }
            if matches!(kind.effect(), EffectClass::Pure | EffectClass::Structural) {
                continue;
            }
            let consumed = op.deref(ctx).operands().last();
            if consumed != Some(token) {
                return shape.fail(
                    ctx,
                    format!(
                        "{} does not consume the current effect token",
                        describe(ctx, op)
                    ),
                );
            }
            if token.num_uses(ctx) != 1 {
                return shape.fail(
                    ctx,
                    format!(
                        "the effect token {} consumes is used {} times",
                        describe(ctx, op),
                        token.num_uses(ctx)
                    ),
                );
            }
            if kind.effect() == EffectClass::Effectful {
                let Some(next) = op.deref(ctx).results().last() else {
                    return shape.fail(ctx, "an effectful operation yields no token");
                };
                token = next;
            }
        }
    }
    Ok(())
}

fn verify_slot(ctx: &Context, shape: &Shape) -> Result<()> {
    let slot: SlotAttr = shape.require_attr(ctx, &KEY_SLOT, "slot")?;
    shape.results(1)?;
    shape.require(
        shape.result_types[0].deref(ctx).is::<PlaceType>(),
        "a slot is a place",
    )?;
    shape.require(
        enclosing_kind(ctx, shape.op) == Some(CoreOpKind::Func),
        "a slot is allocated in the function's own region",
    )?;
    let in_entry = shape.op.deref(ctx).get_parent_block().is_some_and(|block| {
        block
            .deref(ctx)
            .get_parent_region()
            .and_then(|region| region.deref(ctx).get_entry_block())
            == Some(block)
    });
    shape.require(in_entry, "a slot is allocated once, at function entry")?;
    match slot.param {
        Some(position) => {
            shape.operands(1)?;
            shape.require(
                slot.storage == CoreStorage::Variable,
                "only a variable slot binds a parameter",
            )?;
            let argument = shape.operand_values[0];
            let bound = argument.defining_block().is_some()
                && argument.try_find_index(ctx).ok() == usize::try_from(position).ok();
            shape.require(
                bound,
                "a parameter slot binds the entry argument of its position",
            )
        }
        None => shape.operands(0),
    }
}

fn verify_const(ctx: &Context, shape: &Shape) -> Result<()> {
    shape.operands(0)?;
    shape.value_result(ctx)?;
    let constant: ConstAttr = shape.require_attr(ctx, &KEY_CONSTANT, "constant")?;
    let result = shape.result_types[0].deref(ctx);
    let agrees = match &constant {
        ConstAttr::Int(_) => result.is::<IntType>(),
        ConstAttr::IntLiteral(digits) => {
            result.is::<IntLiteralType>()
                && mojito_common::literal::IntLiteral::parse_radix(digits.as_str(), 10).is_some()
        }
        ConstAttr::Bool(_) => result.is::<BoolType>(),
        ConstAttr::Str(_) => result.is::<StringLiteralType>(),
        ConstAttr::None => result.is::<NoneType>(),
    };
    shape.require(agrees, "the constant agrees with its result type")
}

fn verify_simd_make(ctx: &Context, shape: &Shape) -> Result<()> {
    shape.value_result(ctx)?;
    shape.value_operands(ctx, 0..shape.operand_types.len())?;
    let Some(width) = shape.result_types[0]
        .deref(ctx)
        .downcast_ref::<SimdType>()
        .map(|simd| simd.width)
    else {
        return shape.fail(ctx, "simd_make yields a SIMD value");
    };
    let lanes = shape.operand_types.len() as u64;
    shape.require(
        lanes == width || lanes == 1,
        "simd_make takes one lane per width, or one to splat",
    )
}

fn verify_project(ctx: &Context, shape: &Shape) -> Result<()> {
    let projection: ProjectionAttr = shape.require_attr(ctx, &KEY_PROJECTION, "projection")?;
    shape.results(1)?;
    shape.place_operand(ctx, 0)?;
    let indices = projection
        .steps
        .iter()
        .filter(|step| step.kind == CoreStepKind::Index)
        .count();
    shape.require(
        shape.operand_types.len() == indices + 1,
        "a projection takes its root and one operand per dynamic index",
    )?;
    shape.value_operands(ctx, 1..shape.operand_types.len())?;
    let Some(target) = shape.result_types[0]
        .deref(ctx)
        .downcast_ref::<PlaceType>()
        .map(|place| place.target)
    else {
        return shape.fail(ctx, "a projection yields a place");
    };
    let designated = projection
        .steps
        .last()
        .map(|step| step.ty)
        .or(projection.root_ty);
    let through = designated.and_then(|ty| {
        ty.deref(ctx)
            .downcast_ref::<RefType>()
            .map(|ty| ty.referent)
    });
    shape.require(
        projection.ty == Some(target) && [designated, through].contains(&Some(target)),
        "a projection yields the place its last step designates, or that reference's referent",
    )
}

/// The call facts and the operand segments they declare; the last operand
/// is the effect token.
fn verify_call_operands(ctx: &Context, shape: &Shape) -> Result<CallAttr> {
    let call: CallAttr = shape.require_attr(ctx, &KEY_CALL, "call facts")?;
    let segments = call.segments();
    let total = segments
        .iter()
        .try_fold(0usize, |total, segment| total.checked_add(*segment));
    let Some(total) = total.filter(|total| shape.operand_types.len() == total + 1) else {
        return shape.fail(
            ctx,
            "a call's operands fill its declared segments, then the effect token",
        );
    };
    let aligned =
        |places: &[bool], arguments: usize| places.is_empty() || places.len() == arguments;
    shape.require(
        aligned(&call.arg_places, segments[1]) && aligned(&call.kwarg_places, segments[2]),
        "a call records one place flag per argument, or no place table",
    )?;
    let values = segments[0] + segments[1] + segments[2] + segments[3];
    shape.value_operands(ctx, 0..values)?;
    for index in values..total {
        shape.place_operand(ctx, index)?;
    }
    shape.require(!call.target.as_str().is_empty(), "a call names its target")?;
    Ok(call)
}

/// The operand layout of a subscript: `values` value operands, the reified
/// compile-time arguments of `call`, then `places` retained places.
fn verify_subscript_operands(
    ctx: &Context,
    shape: &Shape,
    values: usize,
    call: Option<&CoreSubscriptCall>,
    places: usize,
) -> Result<()> {
    let values = values + call.map_or(0, CoreSubscriptCall::reified);
    shape.require(
        shape.operand_types.len() == values + places + 1,
        "a subscript's operands are its values, its retained places, then the effect token",
    )?;
    shape.value_operands(ctx, 0..values)?;
    for index in values..values + places {
        shape.place_operand(ctx, index)?;
    }
    shape.require(
        call.is_none_or(|call| !call.target.as_str().is_empty()),
        "a subscript call names its target",
    )
}

fn verify_index(ctx: &Context, shape: &Shape) -> Result<()> {
    shape.results(2)?;
    shape.value_result_with_effect(ctx)?;
    verify_index_operands(ctx, shape)
}

fn verify_index_operands(ctx: &Context, shape: &Shape) -> Result<()> {
    let index: IndexAttr = shape.require_attr(ctx, &KEY_SUBSCRIPT, "subscript")?;
    shape.require(
        index.call.is_none() || index.intrinsic.is_none(),
        "an index dispatches to a call or to a storage family, not both",
    )?;
    let places = usize::from(index.base_place) + usize::from(index.index_place);
    verify_subscript_operands(ctx, shape, 2, index.call.as_ref(), places)
}

fn verify_multi_set(ctx: &Context, shape: &Shape) -> Result<()> {
    shape.results(1)?;
    verify_multi_set_operands(ctx, shape)
}

fn verify_multi_set_operands(ctx: &Context, shape: &Shape) -> Result<()> {
    let store: MultiSetAttr = shape.require_attr(ctx, &KEY_MULTI_SET, "subscript store")?;
    let places = usize::from(store.receiver_place)
        + store.arg_places.iter().filter(|place| **place).count()
        + usize::from(store.value_place);
    let values = store.arg_places.len() + 2;
    verify_subscript_operands(ctx, shape, values, Some(&store.call), places)
}

fn verify_invoke(ctx: &Context, shape: &Shape) -> Result<()> {
    let Some(invoked) = invoked_kind(ctx, shape.op) else {
        return shape.fail(ctx, "an invoke carries the facts of the call it makes");
    };
    match invoked {
        CoreOpKind::Index => verify_index_operands(ctx, shape)?,
        CoreOpKind::MultiSet => verify_multi_set_operands(ctx, shape)?,
        _ => verify_call_operands(ctx, shape).map(drop)?,
    }
    shape.successors(2)?;
    let Some(error) = raised_by(ctx, shape.op) else {
        return shape.fail(ctx, "an invoke names the error type its callee raises");
    };
    let effect: TypeHandle = EffectType::get(ctx).into();
    let normal: Vec<TypeHandle> = shape.successors[0]
        .deref(ctx)
        .arguments()
        .map(|value| value.get_type(ctx))
        .collect();
    if invoked == CoreOpKind::MultiSet {
        shape.require(
            normal == [effect],
            "the normal successor of a store takes the effect token",
        )?;
    } else {
        shape.require_attr::<RegAttr>(ctx, &KEY_REG, "register")?;
        shape.require(
            normal.len() == 2 && is_value_type(ctx, normal[0]) && normal[1] == effect,
            "the normal successor takes the result and the effect token",
        )?;
    }
    shape.forwards(ctx, 1, &[error, effect])
}

fn verify_lifecycle(ctx: &Context, shape: &Shape, kinds: &[CoreLifecycle]) -> Result<()> {
    shape.operands(2)?;
    shape.results(1)?;
    let lifecycle: LifecycleAttr = shape.require_attr(ctx, &KEY_LIFECYCLE, "lifecycle")?;
    shape.require(
        kinds.contains(&lifecycle.kind),
        "the lifecycle kind belongs to this operation",
    )?;
    if lifecycle.kind == CoreLifecycle::DropReg {
        return shape.value_operands(ctx, 0..1);
    }
    shape.place_operand(ctx, 0)?;
    let owner = root_slot(ctx, shape.operand_values[0]);
    shape.require(
        owner.is_some_and(|slot| {
            slot.storage == CoreStorage::Variable && slot.id == lifecycle.owner
        }),
        "the lifecycle event names the owner of the place it destroys",
    )
}

fn verify_loans(ctx: &Context, shape: &Shape) -> Result<()> {
    let loans: LoansAttr = shape.require_attr(ctx, &KEY_LOANS, "loan set")?;
    shape.results(2)?;
    shape.marker_result(ctx)?;
    shape.require(
        shape.operand_types.len() == loans.loans.len() + 2,
        "a loan generation takes its holder, one place per loan, and the effect token",
    )?;
    for index in 0..=loans.loans.len() {
        shape.place_operand(ctx, index)?;
    }
    Ok(())
}

fn verify_raise(ctx: &Context, shape: &Shape) -> Result<()> {
    shape.operands(2)?;
    shape.value_operands(ctx, 0..1)?;
    let dead = attr::<DeadTermAttr>(ctx, shape.op, &KEY_DEAD_TERM);
    shape.require(dead.is_some(), "a raise records the terminator it cuts off")?;
    match shape.successors.len() {
        0 => Ok(()),
        1 => shape.forwards(ctx, 0, &shape.operand_types),
        _ => shape.fail(ctx, "a raise has at most one error successor"),
    }
}

fn verify_outcome(ctx: &Context, shape: &Shape) -> Result<()> {
    let outcome: OutcomeAttr = shape.require_attr(ctx, &KEY_OUTCOME, "outcome")?;
    shape.results(2)?;
    shape.require(
        shape.result_types[0].deref(ctx).is::<OutcomeType>(),
        "an outcome yields a pending outcome",
    )?;
    match outcome.kind {
        OutcomeKind::Normal => shape.operands(1),
        OutcomeKind::Error => {
            shape.operands(2)?;
            shape.require(
                shape.operand_types[0].deref(ctx).is::<ErrorType>()
                    || shape.operand_types[0].deref(ctx).is::<NominalType>(),
                "an error outcome carries the raised error",
            )
        }
    }
}

fn verify_resume(ctx: &Context, shape: &Shape) -> Result<()> {
    shape.operands(2)?;
    shape.require(
        shape.operand_types[0].deref(ctx).is::<OutcomeType>(),
        "a resume dispatches on a pending outcome",
    )?;
    let effect: TypeHandle = EffectType::get(ctx).into();
    match shape.successors.len() {
        1 => shape.forwards(ctx, 0, &[effect]),
        2 => {
            shape.forwards(ctx, 0, &[effect])?;
            let error: Vec<TypeHandle> = shape.successors[1]
                .deref(ctx)
                .arguments()
                .map(|value| value.get_type(ctx))
                .collect();
            shape.require(
                error.len() == 2 && is_value_type(ctx, error[0]) && error[1] == effect,
                "the error successor takes the raised error and the effect token",
            )
        }
        _ => shape.fail(
            ctx,
            "a resume continues normally and, when an error can be pending, to its error target",
        ),
    }
}

fn verify_try_bridge(ctx: &Context, shape: &Shape) -> Result<()> {
    let parts: TryAttr = shape.require_attr(ctx, &KEY_TRY, "try parts")?;
    shape.results(1)?;
    shape.successors(0)?;
    shape.require(
        shape.regions.len() == 4,
        "a structured try has four regions",
    )?;
    for index in 0..shape.operand_types.len() - 1 {
        shape.place_operand(ctx, index)?;
        let slot = root_slot(ctx, shape.operand_values[index]);
        shape.require(
            slot.is_some_and(|slot| slot.storage == CoreStorage::Variable),
            "a cleanup operand is a variable slot",
        )?;
    }
    shape.require(
        parts.handler || parts.error_var.is_none(),
        "only a handler binds the caught error",
    )?;
    let present = [true, parts.handler, parts.orelse, parts.finalbody];
    for (region, present) in shape.regions.iter().zip(present) {
        let entry = region.deref(ctx).get_entry_block();
        shape.require(
            entry.is_some() == present,
            "a structured try has exactly the regions its parts declare",
        )?;
        if let Some(entry) = entry {
            let arguments: Vec<TypeHandle> = entry
                .deref(ctx)
                .arguments()
                .map(|value| value.get_type(ctx))
                .collect();
            let effect: TypeHandle = EffectType::get(ctx).into();
            shape.require(
                arguments == [effect],
                "a structured region's entry takes the effect token",
            )?;
        }
    }
    Ok(())
}

/// The slot a place is rooted at, through any projection.
pub fn root_slot(ctx: &Context, place: Value) -> Option<SlotAttr> {
    let mut op = place.defining_op()?;
    while CoreOpKind::of(ctx, op)? == CoreOpKind::Project {
        op = op.deref(ctx).operands().next()?.defining_op()?;
    }
    attr::<SlotAttr>(ctx, op, &KEY_SLOT)
}

/// The operation an `invoke` stands for, or a call-like operation is: the
/// one whose facts it carries.
pub fn invoked_kind(ctx: &Context, op: Ptr<Operation>) -> Option<CoreOpKind> {
    let attributes = &op.deref(ctx).attributes;
    if attributes.get::<CallAttr>(&KEY_CALL).is_some() {
        return Some(CoreOpKind::Call);
    }
    if attributes.get::<IndexAttr>(&KEY_SUBSCRIPT).is_some() {
        return Some(CoreOpKind::Index);
    }
    attributes
        .get::<MultiSetAttr>(&KEY_MULTI_SET)
        .map(|_| CoreOpKind::MultiSet)
}

/// The error type the call `op` makes may raise.
pub fn raised_by(ctx: &Context, op: Ptr<Operation>) -> Option<TypeHandle> {
    match invoked_kind(ctx, op)? {
        CoreOpKind::Index => attr::<IndexAttr>(ctx, op, &KEY_SUBSCRIPT)?.call?.raises,
        CoreOpKind::MultiSet => attr::<MultiSetAttr>(ctx, op, &KEY_MULTI_SET)?.call.raises,
        _ => attr::<CallAttr>(ctx, op, &KEY_CALL)?.raises,
    }
}

/// Whether `place` is a variable slot itself, under no projection.
pub fn variable_slot(ctx: &Context, place: Value) -> bool {
    place
        .defining_op()
        .and_then(|op| attr::<SlotAttr>(ctx, op, &KEY_SLOT))
        .is_some_and(|slot| slot.storage == CoreStorage::Variable)
}

/// Whether `op`'s identity marks it as register transport.
pub fn is_transport(ctx: &Context, op: Ptr<Operation>) -> bool {
    attr::<IdentityAttr>(ctx, op, &KEY_IDENTITY).is_some_and(|identity| {
        matches!(
            identity.role,
            CoreRole::RegisterLoad(_) | CoreRole::RegisterStore | CoreRole::YieldStore
        )
    })
}

/// The integer and unsigned core types, as the scalar operand types of
/// indices and arithmetic.
pub fn is_machine_integer(ctx: &Context, ty: TypeHandle) -> bool {
    let object = ty.deref(ctx);
    object.is::<IntType>() || object.is::<UIntType>()
}
