//! Export: a bridge-stage `mojito_core` module back to a `MirProgram`,
//! read from the module's current operations and typed attributes alone.

use std::collections::{HashMap, HashSet};

use pliron::basic_block::BasicBlock;
use pliron::context::{Context, Ptr};
use pliron::linked_list::ContainsLinkedList;
use pliron::operation::Operation;
use pliron::region::Region;
use pliron::r#type::TypeHandle;
use pliron::value::Value;

use mojito_checked::checked::{CheckedIteratorCall, CheckedResultAdapter, IterationMode};
use mojito_mir::mir::{
    FuncRef, MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr, MirLoan,
    MirParamArg, MirPlace, MirProgram, MirStructDeclaration, MirSubscriptArg, MirSubscriptCall,
    MirTerm, Proj, Reg, SpanTable,
};
use mojito_types::param_expr::ParamContext;
use mojito_types::types::Ty;

use super::attrs::{
    CallAttr, CleanupAttr, ConstAttr, CoreCallKind, CoreIntrinsic, CoreIterationMode,
    CoreLifecycle, CoreParamArg, CorePointerAccess, CoreResultAdapter, CoreRole, CoreStepKind,
    CoreStorage, CoreSubscriptCall, DeadTermAttr, DeclarationAttr, IdentityAttr, IndexAttr,
    InfixAttr, InvalidateAttr, IterInitAttr, IterNextAttr, LifecycleAttr, LoansAttr, ModuleAttr,
    MultiSetAttr, OrphansAttr, PointerStorageAttr, PrefixAttr, ProjectionAttr, ProvenanceAttr,
    RegAttr, ResolvedAttr, SignatureAttr, SimdConvertAttr, SlotAttr, StoreAttr, TryAttr,
    UseModeAttr,
};
use super::inventory::CoreOpKind;
use super::ops::{
    KEY_CALL, KEY_CLEANUP, KEY_CONSTANT, KEY_DEAD_TERM, KEY_DECLARATION, KEY_IDENTITY, KEY_INFIX,
    KEY_INVALIDATION, KEY_ITER_INIT, KEY_ITER_NEXT, KEY_LIFECYCLE, KEY_LOANS, KEY_MULTI_SET,
    KEY_ORPHANS, KEY_POINTER_STORAGE, KEY_PREFIX, KEY_PROJECTION, KEY_PROVENANCE, KEY_REG,
    KEY_RESOLVED, KEY_SIGNATURE, KEY_SIMD_CONVERT, KEY_SLOT, KEY_STORE, KEY_SUBSCRIPT, KEY_TABLES,
    KEY_TRY, KEY_USE_MODE,
};
use super::params::{PayloadBinder, export_param};
use super::types::export_type;
use super::verify::{attr, describe};
use super::{A1Error, A1ErrorKind};

/// A program rebuilt from core, with the entry map the module carried.
#[derive(Debug, Clone)]
pub struct Exported {
    pub program: MirProgram,
    /// Each requested entry and the concrete function standing for it.
    pub entries: Vec<(String, String)>,
}

/// Export a bridge-stage module. The caller verifies the module first; the
/// result still owes `mir::verify`.
pub fn export_program(ctx: &Context, module: Ptr<Operation>) -> Result<Exported, A1Error> {
    let tables: ModuleAttr = required(ctx, module, &KEY_TABLES)?;
    let structs = tables
        .structs
        .iter()
        .map(|decl| {
            Ok(MirStructDeclaration {
                name: decl.name.0.clone(),
                fields: decl
                    .fields
                    .iter()
                    .map(|field| Ok((field.name.0.clone(), export_type(ctx, field.ty)?)))
                    .collect::<Result<Vec<_>, A1Error>>()?,
                mut_self_methods: decl
                    .mut_self_methods
                    .iter()
                    .map(|method| method.0.clone())
                    .collect::<HashSet<_>>(),
                fieldwise_init: decl.fieldwise_init,
                param_decls: Vec::new(),
                explicit_destroy_message: decl
                    .explicit_destroy_message
                    .as_ref()
                    .map(|message| message.0.clone()),
                explicit_destructors: decl
                    .explicit_destructors
                    .iter()
                    .map(|destructor| (destructor.name.0.clone(), destructor.raises))
                    .collect::<HashMap<_, _>>(),
            })
        })
        .collect::<Result<Vec<_>, A1Error>>()?;
    let mut functions = Vec::new();
    let mut declared: HashMap<String, MirFunctionDeclaration> = HashMap::new();
    let body = module.deref(ctx).get_region(0);
    for block in body.deref(ctx).iter(ctx) {
        for func in block.deref(ctx).iter(ctx) {
            if CoreOpKind::of(ctx, func) != Some(CoreOpKind::Func) {
                return Err(malformed(ctx, func, "only functions sit in the module"));
            }
            let (name, function) = FnExport::run(ctx, func)?;
            if let Some(declaration) = attr::<DeclarationAttr>(ctx, func, &KEY_DECLARATION) {
                declared.insert(name.clone(), export_declaration(ctx, &declaration)?);
            }
            functions.push((name, function));
        }
    }
    let declarations = tables
        .declaration_order
        .iter()
        .map(|name| {
            declared.remove(name.as_str()).ok_or_else(|| {
                A1Error::new(
                    A1ErrorKind::Export,
                    format!("declaration `{name}` names no declared function of the module"),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(name) = declared.keys().next() {
        return Err(A1Error::new(
            A1ErrorKind::Export,
            format!("function `{name}` is declared outside the declaration table"),
        ));
    }
    Ok(Exported {
        program: MirProgram {
            functions,
            declarations: MirDeclarations {
                structs,
                functions: declarations,
            },
            invariant_errors: Vec::new(),
        },
        entries: tables
            .entries
            .iter()
            .map(|entry| (entry.requested.0.clone(), entry.concrete.0.clone()))
            .collect(),
    })
}

fn required<T: pliron::attribute::Attribute + Clone>(
    ctx: &Context,
    op: Ptr<Operation>,
    key: &pliron::identifier::Identifier,
) -> Result<T, A1Error> {
    attr::<T>(ctx, op, key).ok_or_else(|| malformed(ctx, op, "a required attribute is missing"))
}

fn malformed(ctx: &Context, op: Ptr<Operation>, message: &str) -> A1Error {
    A1Error::new(
        A1ErrorKind::Export,
        format!("{message} ({})", describe(ctx, op)),
    )
}

/// The export of one function.
struct FnExport<'a> {
    ctx: &'a Context,
    reg_types: HashMap<u32, Ty>,
    spans: SpanTable,
}

impl<'a> FnExport<'a> {
    fn run(ctx: &'a Context, func: Ptr<Operation>) -> Result<(String, MirFunction), A1Error> {
        let signature: SignatureAttr = required(ctx, func, &KEY_SIGNATURE)?;
        let orphans: OrphansAttr = required(ctx, func, &KEY_ORPHANS)?;
        let name = signature.symbol.0.clone();
        let mut export = Self {
            ctx,
            reg_types: HashMap::new(),
            spans: SpanTable::default(),
        };
        let region = func.deref(ctx).get_region(0);
        let entry = region
            .deref(ctx)
            .get_entry_block()
            .ok_or_else(|| malformed(ctx, func, "a function without an entry block"))?;
        let mut var_names = Vec::new();
        let mut var_tys = HashMap::new();
        let mut owned_params = Vec::new();
        let mut deinit_params = Vec::new();
        let mut ref_params = Vec::new();
        for op in entry.deref(ctx).iter(ctx) {
            let Some(slot) = attr::<SlotAttr>(ctx, op, &KEY_SLOT) else {
                continue;
            };
            if slot.storage != CoreStorage::Variable {
                continue;
            }
            if slot.id as usize != var_names.len() {
                return Err(malformed(ctx, op, "variable slots are not in slot order"));
            }
            let place = op.deref(ctx).get_result(0);
            let target = super::types::place_target(ctx, place)
                .ok_or_else(|| malformed(ctx, op, "a slot that is not a place"))?;
            var_tys.insert(slot.id, export_type(ctx, target)?);
            var_names.push(slot.name.0.clone());
            if slot.param.is_some() {
                owned_params.push(slot.owned);
                deinit_params.push(slot.deinit);
                ref_params.push(slot.by_ref);
            }
        }
        let blocks = export.region(region)?;
        for orphan in &orphans.0 {
            if let Some(ty) = orphan.ty {
                export.reg_types.insert(orphan.reg, export_type(ctx, ty)?);
            }
            if let Some(span) = &orphan.provenance.span {
                export
                    .spans
                    .0
                    .insert(orphan.reg, (span.span()?, orphan.provenance.origin));
            }
        }
        let function = MirFunction {
            blocks,
            n_regs: signature.registers,
            n_vars: var_names.len(),
            var_names,
            n_params: signature.params.len(),
            param_types: signature
                .params
                .iter()
                .map(|ty| export_type(ctx, *ty))
                .collect::<Result<Vec<_>, _>>()?,
            owned_params,
            deinit_params,
            ref_params,
            returns_reference: signature.returns_reference,
            var_tys,
            ret_ty: signature.ret.map(|ty| export_type(ctx, ty)).transpose()?,
            raises: signature.raises,
            error_ty: signature.error.map(|ty| export_type(ctx, ty)).transpose()?,
            spans: export.spans,
            reg_types: export.reg_types,
        };
        Ok((name, function))
    }

    /// The MIR blocks of a region: every block after its synthesized entry.
    fn region(&mut self, region: Ptr<Region>) -> Result<Vec<MirBlock>, A1Error> {
        let ctx = self.ctx;
        let blocks: Vec<Ptr<BasicBlock>> = region.deref(ctx).iter(ctx).collect();
        let indices: HashMap<Ptr<BasicBlock>, usize> = blocks
            .iter()
            .skip(1)
            .enumerate()
            .map(|(index, block)| (*block, index))
            .collect();
        let mut out = Vec::new();
        for (index, block) in blocks.iter().skip(1).enumerate() {
            let mut instrs = Vec::new();
            let mut term = None;
            for op in block.deref(ctx).iter(ctx) {
                if term.is_some() {
                    return Err(malformed(
                        ctx,
                        op,
                        "an operation after its block's terminator",
                    ));
                }
                let identity: IdentityAttr = required(ctx, op, &KEY_IDENTITY)?;
                if identity.block != index as u64 {
                    return Err(malformed(
                        ctx,
                        op,
                        "an operation outside its recorded block",
                    ));
                }
                term = self.operation(op, &identity, &indices, &mut instrs)?;
            }
            let term = term
                .ok_or_else(|| A1Error::new(A1ErrorKind::Export, "a block without a terminator"))?;
            out.push(MirBlock { instrs, term });
        }
        Ok(out)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one exhaustive match is the conversion-totality contract"
    )]
    fn operation(
        &mut self,
        op: Ptr<Operation>,
        identity: &IdentityAttr,
        blocks: &HashMap<Ptr<BasicBlock>, usize>,
        instrs: &mut Vec<MirInstr>,
    ) -> Result<Option<MirTerm>, A1Error> {
        let ctx = self.ctx;
        let kind = CoreOpKind::of(ctx, op)
            .ok_or_else(|| malformed(ctx, op, "an operation outside the registry"))?;
        let operation = op.deref(ctx);
        let operands: Vec<Value> = operation.operands().collect();
        let successor = |index: usize| {
            blocks
                .get(&operation.get_successor(index))
                .copied()
                .ok_or_else(|| malformed(ctx, op, "a branch out of its region"))
        };
        match kind {
            CoreOpKind::Project => {}
            CoreOpKind::Load if matches!(identity.role, CoreRole::RegisterLoad(_)) => {
                let reg: RegAttr = required(ctx, op, &KEY_REG)?;
                let slot = self.register_slot(op, operands[0])?;
                if slot != reg.0 {
                    return Err(malformed(
                        ctx,
                        op,
                        "a register load of another register's slot",
                    ));
                }
                self.record_type(op, reg.0)?;
            }
            CoreOpKind::Store
                if matches!(
                    identity.role,
                    CoreRole::RegisterStore | CoreRole::YieldStore
                ) =>
            {
                let slot = self.register_slot(op, operands[0])?;
                if self.reg(op, operands[1])?.0 != slot {
                    return Err(malformed(
                        ctx,
                        op,
                        "a register store of another register's value",
                    ));
                }
            }
            CoreOpKind::Const => {
                let constant: ConstAttr = required(ctx, op, &KEY_CONSTANT)?;
                instrs.push(MirInstr::Const {
                    dest: self.dest(op)?,
                    k: constant.constant()?,
                });
            }
            CoreOpKind::Materialize => {
                let dest = self.dest(op)?;
                instrs.push(MirInstr::MaterializeLiteral {
                    dest,
                    value: self.reg(op, operands[0])?,
                    target: self.reg_types[&dest.0].clone(),
                });
            }
            CoreOpKind::Binary => {
                let infix: InfixAttr = required(ctx, op, &KEY_INFIX)?;
                let resolved: ResolvedAttr = required(ctx, op, &KEY_RESOLVED)?;
                instrs.push(MirInstr::BinOp {
                    op: infix.op(),
                    dest: self.dest(op)?,
                    a: self.reg(op, operands[0])?,
                    b: self.reg(op, operands[1])?,
                    resolved: resolved.0.map(|symbol| symbol.0),
                });
            }
            CoreOpKind::SimdMake => {
                let dest = self.dest(op)?;
                let Ty::Simd { dtype, width } = &self.reg_types[&dest.0] else {
                    return Err(malformed(ctx, op, "a SIMD construction of another type"));
                };
                let (Some(dtype), Some(width)) = (dtype.known(), width.known()) else {
                    return Err(malformed(ctx, op, "a SIMD construction of an open type"));
                };
                instrs.push(MirInstr::MakeSimd {
                    dest,
                    dtype,
                    width: usize::try_from(width)
                        .map_err(|_| malformed(ctx, op, "a negative SIMD width"))?,
                    elems: operands
                        .iter()
                        .map(|value| self.reg(op, *value))
                        .collect::<Result<Vec<_>, _>>()?,
                });
            }
            CoreOpKind::Use => {
                let mode: UseModeAttr = required(ctx, op, &KEY_USE_MODE)?;
                instrs.push(MirInstr::UseVar {
                    dest: self.dest(op)?,
                    var: self.variable(op, operands[0])?,
                    mode: mode.mode(),
                });
            }
            CoreOpKind::Store => match required::<StoreAttr>(ctx, op, &KEY_STORE)? {
                StoreAttr::DefVar { binding } => instrs.push(MirInstr::DefVar {
                    var: self.variable(op, operands[0])?,
                    src: self.reg(op, operands[1])?,
                    binding_ty: binding.map(|ty| export_type(ctx, ty)).transpose()?,
                }),
                StoreAttr::Place => instrs.push(MirInstr::Store {
                    place: self.place(op, operands[0])?,
                    src: self.reg(op, operands[1])?,
                }),
                StoreAttr::Register | StoreAttr::Caught => {
                    return Err(malformed(
                        ctx,
                        op,
                        "a transport or caught-error store where a source store belongs",
                    ));
                }
            },
            CoreOpKind::Load => instrs.push(MirInstr::LoadPlace {
                dest: self.dest(op)?,
                place: self.place(op, operands[0])?,
            }),
            CoreOpKind::RefMake => instrs.push(MirInstr::MakeRef {
                dest: self.dest(op)?,
                place: self.place(op, operands[0])?,
            }),
            CoreOpKind::RefStore => instrs.push(MirInstr::StoreRef {
                place: self.place(op, operands[0])?,
                reference: self.reg(op, operands[1])?,
            }),
            CoreOpKind::Unary => {
                let prefix: PrefixAttr = required(ctx, op, &KEY_PREFIX)?;
                instrs.push(MirInstr::UnOp {
                    op: prefix.op(),
                    dest: self.dest(op)?,
                    a: self.reg(op, operands[0])?,
                });
            }
            CoreOpKind::SimdConvert => {
                let conversion: SimdConvertAttr = required(ctx, op, &KEY_SIMD_CONVERT)?;
                let dest = self.dest(op)?;
                let Ty::Simd { dtype, width } = &self.reg_types[&dest.0] else {
                    return Err(malformed(ctx, op, "a SIMD conversion to another type"));
                };
                let (Some(dtype), Some(width)) = (dtype.known(), width.known()) else {
                    return Err(malformed(ctx, op, "a SIMD conversion to an open type"));
                };
                let width = usize::try_from(width)
                    .map_err(|_| malformed(ctx, op, "a negative SIMD width"))?;
                let value = self.reg(op, operands[0])?;
                instrs.push(match conversion {
                    SimdConvertAttr::Cast => MirInstr::SimdCast {
                        dest,
                        value,
                        dtype,
                        width,
                    },
                    SimdConvertAttr::Bits => MirInstr::SimdBitcast {
                        dest,
                        value,
                        dtype,
                        width,
                    },
                });
            }
            CoreOpKind::RefRead => instrs.push(MirInstr::ReadRef {
                dest: self.dest(op)?,
                reference: self.reg(op, operands[0])?,
            }),
            CoreOpKind::Copy => instrs.push(MirInstr::CopyValue {
                dest: self.dest(op)?,
                value: self.reg(op, operands[0])?,
            }),
            CoreOpKind::Move => instrs.push(MirInstr::MovePlace {
                dest: self.dest(op)?,
                place: self.place(op, operands[0])?,
            }),
            CoreOpKind::KeepAlive => instrs.push(MirInstr::KeepAlive {
                var: self.variable(op, operands[0])?,
            }),
            CoreOpKind::PointerStorage => {
                let storage: PointerStorageAttr = required(ctx, op, &KEY_POINTER_STORAGE)?;
                let dest = self.dest(op)?;
                let pointer = self.reg(op, operands[0])?;
                let index = self.reg(op, operands[1])?;
                let element = export_type(ctx, storage.element)?;
                instrs.push(match storage.access {
                    CorePointerAccess::Take => MirInstr::PointerStorageTake {
                        dest,
                        pointer,
                        index,
                        element,
                    },
                    CorePointerAccess::Destroy => MirInstr::PointerStorageDestroy {
                        dest,
                        pointer,
                        index,
                        element,
                    },
                });
            }
            CoreOpKind::Index => {
                let index: IndexAttr = required(ctx, op, &KEY_SUBSCRIPT)?;
                let mut rest = operands[2..operands.len() - 1].iter().copied();
                let call = index
                    .call
                    .as_ref()
                    .map(|call| self.subscript_call(op, call, &mut rest))
                    .transpose()?;
                let mut place = |retained: bool| {
                    retained
                        .then(|| rest.next())
                        .flatten()
                        .map(|value| self.place(op, value))
                        .transpose()
                };
                let base_place = place(index.base_place)?;
                let index_place = place(index.index_place)?;
                instrs.push(MirInstr::Index {
                    dest: self.dest(op)?,
                    base: self.reg(op, operands[0])?,
                    index: self.reg(op, operands[1])?,
                    base_place,
                    index_place,
                    call,
                    intrinsic: index.intrinsic.map(CoreIntrinsic::intrinsic),
                });
            }
            CoreOpKind::MultiSet => {
                let store: MultiSetAttr = required(ctx, op, &KEY_MULTI_SET)?;
                let count = store.arg_places.len();
                let args = operands[1..=count]
                    .iter()
                    .map(|value| self.reg(op, *value).map(MirSubscriptArg::Index))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut rest = operands[count + 2..operands.len() - 1].iter().copied();
                let call = self.subscript_call(op, &store.call, &mut rest)?;
                let mut place = |retained: bool| {
                    retained
                        .then(|| rest.next())
                        .flatten()
                        .map(|value| self.place(op, value))
                        .transpose()
                };
                let receiver_place = place(store.receiver_place)?;
                let arg_places = store
                    .arg_places
                    .iter()
                    .map(|retained| place(*retained))
                    .collect::<Result<Vec<_>, A1Error>>()?;
                let value_place = place(store.value_place)?;
                instrs.push(MirInstr::MultiSet {
                    receiver: self.reg(op, operands[0])?,
                    receiver_place,
                    args,
                    arg_places,
                    value: self.reg(op, operands[count + 1])?,
                    value_place,
                    value_keyword: store.value_keyword,
                    call,
                });
            }
            CoreOpKind::IterInit => {
                let iteration: IterInitAttr = required(ctx, op, &KEY_ITER_INIT)?;
                instrs.push(MirInstr::GetIter {
                    source: self.variable(op, operands[0])?,
                    dest: self.variable(op, operands[1])?,
                    mode: match iteration.mode {
                        CoreIterationMode::Borrowed => IterationMode::Borrowed,
                        CoreIterationMode::Owned => IterationMode::Owned,
                    },
                    prepare: iteration
                        .prepare
                        .iter()
                        .map(|step| step.0.clone())
                        .collect(),
                });
            }
            CoreOpKind::IterNext => {
                let next: IterNextAttr = required(ctx, op, &KEY_ITER_NEXT)?;
                let yielded_ty = export_type(ctx, op.deref(ctx).get_type(1))?;
                self.reg_types.insert(next.yielded, yielded_ty);
                if let Some(span) = &next.yielded_record.span {
                    self.spans
                        .0
                        .insert(next.yielded, (span.span()?, next.yielded_record.origin));
                }
                let reference = |ty: TypeHandle| match export_type(ctx, ty)? {
                    Ty::Ref(reference) => Ok(reference),
                    _ => Err(malformed(ctx, op, "a reference result of another type")),
                };
                instrs.push(MirInstr::TryNext {
                    dest: self.dest(op)?,
                    yielded: Reg(next.yielded),
                    iter: self.variable(op, operands[0])?,
                    call: CheckedIteratorCall {
                        target: next.target.0.clone(),
                        result_ty: export_type(ctx, next.result)?,
                        reference_result: next.reference_result.map(reference).transpose()?,
                        raises: next.raises.map(|ty| export_type(ctx, ty)).transpose()?,
                        result_adapter: next.adapter.map(|adapter| match adapter {
                            CoreResultAdapter::CopyIteratorReference => {
                                CheckedResultAdapter::CopyIteratorReference
                            }
                        }),
                    },
                    exhaustion: export_type(ctx, next.exhaustion)?,
                });
            }
            CoreOpKind::Call => instrs.push(self.call(op, &operands)?),
            CoreOpKind::Drop | CoreOpKind::Consume => {
                let lifecycle: LifecycleAttr = required(ctx, op, &KEY_LIFECYCLE)?;
                if lifecycle.kind == CoreLifecycle::DropPlace {
                    instrs.push(MirInstr::DropPlace {
                        place: self.place(op, operands[0])?,
                    });
                    return Ok(None);
                }
                let var = self.variable(op, operands[0])?;
                instrs.push(match lifecycle.kind {
                    CoreLifecycle::DropVar => MirInstr::DropVar { var },
                    CoreLifecycle::ConsumeVar => MirInstr::ConsumeVar { var },
                    CoreLifecycle::DropPlace
                    | CoreLifecycle::DropReg
                    | CoreLifecycle::ConsumePlace => {
                        return Err(malformed(
                            ctx,
                            op,
                            "a lifecycle kind without an export rule",
                        ));
                    }
                });
            }
            CoreOpKind::Loans => {
                let loans: LoansAttr = required(ctx, op, &KEY_LOANS)?;
                let places = &operands[1..operands.len() - 1];
                instrs.push(MirInstr::EstablishLoans {
                    reference: self.variable(op, operands[0])?,
                    loans: loans
                        .loans
                        .iter()
                        .zip(places)
                        .map(|(loan, place)| {
                            Ok(MirLoan {
                                place: self.place(op, *place)?,
                                mutable: loan.mutable,
                                interior: loan
                                    .interior
                                    .as_ref()
                                    .map(super::attrs::CoreInterior::interior),
                                shared: loan.shared,
                            })
                        })
                        .collect::<Result<Vec<_>, A1Error>>()?,
                    marker: self.dest(op)?,
                    dest_interior: loans
                        .dest_interior
                        .as_ref()
                        .map(super::attrs::CoreInterior::interior),
                });
            }
            CoreOpKind::Invalidate => {
                let invalidation: InvalidateAttr = required(ctx, op, &KEY_INVALIDATION)?;
                instrs.push(MirInstr::InvalidateInteriors {
                    base: invalidation.base.interior(),
                    except: invalidation.except,
                    include_base_generation: invalidation.include_base,
                    marker: self.dest(op)?,
                });
            }
            CoreOpKind::TryBridge => {
                let parts: TryAttr = required(ctx, op, &KEY_TRY)?;
                let cleanup = operands[..operands.len() - 1]
                    .iter()
                    .map(|slot| self.variable(op, *slot))
                    .collect::<Result<Vec<_>, _>>()?;
                let regions: Vec<Ptr<Region>> = operation.regions().collect();
                let mut part = |present: bool, index: usize| {
                    present.then(|| self.region(regions[index])).transpose()
                };
                let body = part(true, 0)?.unwrap_or_default();
                let handler = part(parts.handler, 1)?.map(|blocks| (parts.error_var, blocks));
                let orelse = part(parts.orelse, 2)?;
                let finalbody = part(parts.finalbody, 3)?;
                instrs.push(MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    cleanup,
                });
            }
            CoreOpKind::Br => {
                if identity.role == CoreRole::Entry {
                    return Err(malformed(ctx, op, "a region entry branch inside a block"));
                }
                return Ok(Some(MirTerm::Jump(successor(0)?)));
            }
            CoreOpKind::CondBr => {
                return Ok(Some(MirTerm::Branch {
                    cond: self.reg(op, operands[0])?,
                    then_b: successor(0)?,
                    else_b: successor(1)?,
                }));
            }
            CoreOpKind::Return => {
                let Some(cleanup) = attr::<CleanupAttr>(ctx, op, &KEY_CLEANUP) else {
                    let value = match operands.len() {
                        2 => Some(self.reg(op, operands[0])?),
                        _ => None,
                    };
                    return Ok(Some(MirTerm::Return(value)));
                };
                let count = usize::try_from(cleanup.0)
                    .ok()
                    .filter(|count| *count < operands.len())
                    .ok_or_else(|| malformed(ctx, op, "a cleanup count beyond its operands"))?;
                let first = operands.len() - 1 - count;
                let value = match first {
                    0 => None,
                    _ => Some(self.reg(op, operands[0])?),
                };
                let cleanup = operands[first..operands.len() - 1]
                    .iter()
                    .map(|slot| self.variable(op, *slot))
                    .collect::<Result<Vec<_>, _>>()?;
                return Ok(Some(MirTerm::ReturnWithCleanup { value, cleanup }));
            }
            CoreOpKind::Raise => {
                let dead: DeadTermAttr = required(ctx, op, &KEY_DEAD_TERM)?;
                instrs.push(MirInstr::Raise {
                    src: self.reg(op, operands[0])?,
                });
                return Ok(Some(match dead {
                    DeadTermAttr::Jump(target) => MirTerm::Jump(
                        usize::try_from(target)
                            .map_err(|_| malformed(ctx, op, "a block index beyond usize"))?,
                    ),
                    DeadTermAttr::Return => MirTerm::Return(None),
                    DeadTermAttr::FallOff => MirTerm::FallOff,
                }));
            }
            CoreOpKind::RegionExit => return Ok(Some(MirTerm::FallOff)),
            CoreOpKind::Func
            | CoreOpKind::Slot
            | CoreOpKind::Invoke
            | CoreOpKind::Outcome
            | CoreOpKind::Resume => {
                return Err(malformed(
                    ctx,
                    op,
                    "an operation the bridge stage cannot export",
                ));
            }
        }
        Ok(None)
    }

    /// The compile-time arguments `params` records, reading the registers
    /// that reify them from `operands`.
    fn param_args(
        &self,
        op: Ptr<Operation>,
        params: &[CoreParamArg],
        operands: &mut impl Iterator<Item = Value>,
    ) -> Result<Vec<MirParamArg>, A1Error> {
        let ctx = self.ctx;
        let context = ParamContext::detached();
        params
            .iter()
            .map(|param| {
                Ok(MirParamArg {
                    name: param.name.as_ref().map(|name| name.0.clone()),
                    value: param
                        .value
                        .then(|| operands.next())
                        .flatten()
                        .map(|value| self.reg(op, value))
                        .transpose()?,
                    binder: param
                        .binder
                        .as_ref()
                        .map(PayloadBinder::reference)
                        .transpose()?,
                    expr: param
                        .expr
                        .map(|key| export_param(ctx, &context, key))
                        .transpose()?,
                })
            })
            .collect()
    }

    fn subscript_call(
        &self,
        op: Ptr<Operation>,
        call: &CoreSubscriptCall,
        operands: &mut impl Iterator<Item = Value>,
    ) -> Result<MirSubscriptCall, A1Error> {
        let ctx = self.ctx;
        let reference = |ty: TypeHandle| match export_type(ctx, ty)? {
            Ty::Ref(reference) => Ok(reference),
            _ => Err(malformed(ctx, op, "a reference result of another type")),
        };
        Ok(MirSubscriptCall {
            target: call.target.0.clone(),
            raises: call.raises.map(|ty| export_type(ctx, ty)).transpose()?,
            result_ty: export_type(ctx, call.result)?,
            receiver_requires_place: call.receiver_requires_place,
            receiver_convention: call
                .receiver_convention
                .map(super::attrs::CoreConvention::convention),
            arguments: call
                .arguments
                .iter()
                .map(|argument| argument.argument(ctx))
                .collect::<Result<Vec<_>, _>>()?,
            capture_accesses: Vec::new(),
            reference_result: call.reference_result.map(reference).transpose()?,
            param_arg_regs: self.param_args(op, &call.params, operands)?,
            param_decls: Vec::new(),
        })
    }

    fn call(&mut self, op: Ptr<Operation>, operands: &[Value]) -> Result<MirInstr, A1Error> {
        let ctx = self.ctx;
        let call: CallAttr = required(ctx, op, &KEY_CALL)?;
        let segments = call.segments();
        let mut cursor = operands.iter().copied();
        let mut take = |count: usize| cursor.by_ref().take(count).collect::<Vec<_>>();
        let recv = take(segments[0]);
        let args = take(segments[1]);
        let kwargs = take(segments[2]);
        let mut params = take(segments[3]).into_iter();
        let mut arg_places = take(segments[4]).into_iter();
        let mut kwarg_places = take(segments[5]).into_iter();
        let recv_place = take(segments[6]);
        let regs = |values: &[Value]| {
            values
                .iter()
                .map(|value| self.reg(op, *value))
                .collect::<Result<Vec<_>, _>>()
        };
        let args = regs(&args)?;
        let kwargs: Vec<(String, Reg)> = call
            .kwargs
            .iter()
            .map(|name| name.0.clone())
            .zip(regs(&kwargs)?)
            .collect();
        let places = |flags: &[bool], values: &mut std::vec::IntoIter<Value>| {
            flags
                .iter()
                .map(|retained| {
                    retained
                        .then(|| values.next())
                        .flatten()
                        .map(|value| self.place(op, value))
                        .transpose()
                })
                .collect::<Result<Vec<Option<MirPlace>>, A1Error>>()
        };
        let arg_places = places(&call.arg_places, &mut arg_places)?;
        let kwarg_places = places(&call.kwarg_places, &mut kwarg_places)?;
        let raises = call.raises.map(|ty| export_type(ctx, ty)).transpose()?;
        let param_arg_regs = self.param_args(op, &call.params, &mut params)?;
        let dest = self.dest(op)?;
        Ok(match call.kind {
            CoreCallKind::Direct => MirInstr::Call {
                dest,
                func: FuncRef(call.target.0),
                raises,
                args,
                kwargs,
                arg_places,
                kwarg_places,
                capture_accesses: Vec::new(),
                param_arg_regs,
            },
            CoreCallKind::Method => MirInstr::MethodCall {
                dest,
                recv: self.reg(op, recv[0])?,
                method: call.target.0.clone(),
                resolved: call.resolved.as_ref().map(|symbol| symbol.0.clone()),
                raises,
                reference_result: None,
                result_adapter: None,
                args,
                kwargs,
                recv_place: recv_place
                    .first()
                    .map(|value| self.place(op, *value))
                    .transpose()?,
                recv_writes: call.recv_writes,
                arg_places,
                kwarg_places,
                capture_accesses: Vec::new(),
                param_arg_regs,
                param_decls: Vec::new(),
            },
        })
    }

    /// The register `op` defines, with its type and source record entered
    /// in the frame's tables.
    fn dest(&mut self, op: Ptr<Operation>) -> Result<Reg, A1Error> {
        let ctx = self.ctx;
        let reg: RegAttr = required(ctx, op, &KEY_REG)?;
        self.record_type(op, reg.0)?;
        let provenance: ProvenanceAttr = required(ctx, op, &KEY_PROVENANCE)?;
        if let Some(span) = &provenance.span {
            self.spans
                .0
                .insert(reg.0, (span.span()?, provenance.origin));
        }
        Ok(Reg(reg.0))
    }

    fn record_type(&mut self, op: Ptr<Operation>, reg: u32) -> Result<(), A1Error> {
        let ctx = self.ctx;
        let ty = export_type(ctx, op.deref(ctx).get_type(0))?;
        match self.reg_types.get(&reg) {
            Some(recorded) if *recorded != ty => {
                Err(malformed(ctx, op, "a register defined at two types"))
            }
            _ => {
                self.reg_types.insert(reg, ty);
                Ok(())
            }
        }
    }

    /// The register holding `value`: the one its defining operation names.
    fn reg(&self, user: Ptr<Operation>, value: Value) -> Result<Reg, A1Error> {
        let ctx = self.ctx;
        value
            .defining_op()
            .and_then(|op| {
                let operation = op.deref(ctx);
                if operation.get_result(0) == value {
                    return attr::<RegAttr>(ctx, op, &KEY_REG).map(|reg| reg.0);
                }
                let yielded = operation.get_num_results() == 3 && operation.get_result(1) == value;
                yielded
                    .then(|| attr::<IterNextAttr>(ctx, op, &KEY_ITER_NEXT))
                    .flatten()
                    .map(|next| next.yielded)
            })
            .map(Reg)
            .ok_or_else(|| malformed(ctx, user, "an operand no register holds"))
    }

    fn slot(&self, user: Ptr<Operation>, place: Value) -> Result<SlotAttr, A1Error> {
        let ctx = self.ctx;
        place
            .defining_op()
            .and_then(|op| attr::<SlotAttr>(ctx, op, &KEY_SLOT))
            .ok_or_else(|| malformed(ctx, user, "a place that is no slot"))
    }

    fn variable(&self, user: Ptr<Operation>, place: Value) -> Result<u32, A1Error> {
        let slot = self.slot(user, place)?;
        if slot.storage != CoreStorage::Variable {
            return Err(malformed(
                self.ctx,
                user,
                "a register slot used as a variable",
            ));
        }
        Ok(slot.id)
    }

    fn register_slot(&self, user: Ptr<Operation>, place: Value) -> Result<u32, A1Error> {
        let slot = self.slot(user, place)?;
        if slot.storage != CoreStorage::Register {
            return Err(malformed(
                self.ctx,
                user,
                "a variable slot used as register transport",
            ));
        }
        Ok(slot.id)
    }

    /// The MIR place a projection designates.
    fn place(&self, user: Ptr<Operation>, place: Value) -> Result<MirPlace, A1Error> {
        let ctx = self.ctx;
        let project = place
            .defining_op()
            .filter(|op| CoreOpKind::of(ctx, *op) == Some(CoreOpKind::Project))
            .ok_or_else(|| malformed(ctx, user, "a place that is no projection"))?;
        let projection: ProjectionAttr = required(ctx, project, &KEY_PROJECTION)?;
        let operands: Vec<Value> = project.deref(ctx).operands().collect();
        let mut indices = operands[1..].iter();
        let mut proj = Vec::new();
        let mut projection_tys = Vec::new();
        for step in &projection.steps {
            let index = |value: u64| {
                usize::try_from(value).map_err(|_| malformed(ctx, project, "an index beyond usize"))
            };
            proj.push(match &step.kind {
                CoreStepKind::Field(name) => Proj::Field(name.0.clone()),
                CoreStepKind::Index => {
                    let value = indices.next().ok_or_else(|| {
                        malformed(ctx, project, "a dynamic index without an operand")
                    })?;
                    Proj::Index(self.reg(project, *value)?)
                }
                CoreStepKind::ConstIndex(value) => Proj::ConstIndex(index(*value)?),
                CoreStepKind::Variant(value) => Proj::Variant(index(*value)?),
                CoreStepKind::UninitPayload => Proj::UninitPayload,
            });
            projection_tys.push(export_type(ctx, step.ty)?);
        }
        Ok(MirPlace {
            root: self.variable(project, operands[0])?,
            root_ty: projection
                .root_ty
                .map(|ty| export_type(ctx, ty))
                .transpose()?,
            proj,
            projection_tys,
            ty: projection.ty.map(|ty| export_type(ctx, ty)).transpose()?,
            through: projection.through,
        })
    }
}

fn export_declaration(
    ctx: &Context,
    declaration: &DeclarationAttr,
) -> Result<MirFunctionDeclaration, A1Error> {
    let params = &declaration.params;
    let index = |index: Option<u64>| {
        index
            .map(|index| {
                usize::try_from(index).map_err(|_| {
                    A1Error::new(A1ErrorKind::Export, "a parameter index beyond usize")
                })
            })
            .transpose()
    };
    let (variadic, variadic_convention, variadic_index) =
        export_collector(ctx, declaration.variadic.as_ref())?;
    let (kw_variadic, kw_variadic_convention, kw_variadic_index) =
        export_collector(ctx, declaration.kw_variadic.as_ref())?;
    Ok(MirFunctionDeclaration {
        lowered_name: declaration.symbol.0.clone(),
        param_names: params.iter().map(|param| param.name.0.clone()).collect(),
        param_types: params
            .iter()
            .map(|param| export_type(ctx, param.ty))
            .collect::<Result<Vec<_>, _>>()?,
        defaults: params
            .iter()
            .map(|param| {
                param
                    .default
                    .as_ref()
                    .map(super::attrs::CoreDefault::constant)
                    .transpose()
            })
            .collect::<Result<Vec<_>, _>>()?,
        required: params.iter().map(|param| param.required).collect(),
        variadic,
        variadic_convention,
        variadic_index,
        kw_variadic,
        kw_variadic_convention,
        kw_variadic_index,
        positional_only: index(declaration.positional_only)?,
        keyword_only: index(declaration.keyword_only)?,
        param_decls: Vec::new(),
        has_receiver: declaration.has_receiver,
        receiver_convention: declaration
            .receiver
            .map(super::attrs::CoreConvention::convention),
        param_conventions: params
            .iter()
            .map(|param| {
                param
                    .convention
                    .map(super::attrs::CoreConvention::convention)
            })
            .collect(),
        ret_ty: export_type(ctx, declaration.ret)?,
        returns_reference: declaration.returns_reference,
        raises: declaration.raises,
        error_ty: declaration
            .error
            .map(|ty| export_type(ctx, ty))
            .transpose()?,
        ref_params: params.iter().map(|param| param.by_ref).collect(),
        param_writes: params.iter().map(|param| param.writes).collect(),
    })
}

type ExportedCollector = (
    Option<Ty>,
    Option<mojito_ast::ast::ArgConvention>,
    Option<usize>,
);

fn export_collector(
    ctx: &Context,
    collector: Option<&super::attrs::CoreCollector>,
) -> Result<ExportedCollector, A1Error> {
    let Some(collector) = collector else {
        return Ok((None, None, None));
    };
    Ok((
        collector.ty.map(|ty| export_type(ctx, ty)).transpose()?,
        collector
            .convention
            .map(super::attrs::CoreConvention::convention),
        collector
            .index
            .map(|index| {
                usize::try_from(index).map_err(|_| {
                    A1Error::new(A1ErrorKind::Export, "a collector index beyond usize")
                })
            })
            .transpose()?,
    ))
}
