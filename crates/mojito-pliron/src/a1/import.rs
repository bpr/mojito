//! Import: verified, drop-elaborated, specialized MIR to the bridge stage
//! of `mojito`. Every MIR form has a rule or an explicit rejection.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use pliron::basic_block::BasicBlock;
use pliron::builtin::op_interfaces::{OneRegionInterface, SymbolOpInterface};
use pliron::builtin::ops::ModuleOp;
use pliron::context::{Context, Ptr};
use pliron::op::Op;
use pliron::operation::Operation;
use pliron::region::Region;
use pliron::r#type::TypeHandle;
use pliron::value::Value;

use mojito_checked::checked::{CheckedCallArgumentSource, CheckedResultAdapter, IterationMode};
use mojito_mir::mir::text::{instruction_mnemonic, terminator_mnemonic};
use mojito_mir::mir::verify::{instruction_operand_regs, instruction_result_regs};
use mojito_mir::mir::{
    MirBlock, MirCaptureAccess, MirFunction, MirFunctionDeclaration, MirInstr, MirParamArg,
    MirPlace, MirProgram, MirStructDeclaration, MirSubscriptArg, MirSubscriptCall, MirTerm, Proj,
    Reg,
};
use mojito_types::types::{Ty, TyArg};

use super::attrs::{
    CallAttr, ClosureAttr, ConstAttr, CoreCallKind, CoreCaptureAccess, CoreCaptureMode,
    CoreCollector, CoreConvention, CoreDefault, CoreDestructor, CoreEntry, CoreField, CoreInterior,
    CoreLifecycle, CoreLoan, CoreOrphan, CoreOrphanSpan, CoreParam, CoreRole, CoreSpan, CoreStep,
    CoreStepKind, CoreStorage, CoreStruct, DeadTermAttr, DeclarationAttr, ExitAttr, IdentityAttr,
    InfixAttr, InvalidateAttr, LifecycleAttr, LoansAttr, ModuleAttr, OrphansAttr, ProjectionAttr,
    ProvenanceAttr, RegAttr, ResolvedAttr, SCHEMA, SignatureAttr, SlotAttr, StoreAttr, Text,
    TryAttr, UseModeAttr, var_name,
};
use super::attrs::{
    CleanupAttr, CoreArgSource, CoreCallArgument, CoreIntrinsic, CoreIterationMode, CoreKeywordArg,
    CoreParamArg, CorePointerAccess, CoreResultAdapter, CoreSimdConversion, CoreSliceBounds,
    CoreSliceKind, CoreSubscriptArg, CoreSubscriptCall, CoreUninitAccess, CoreVariantAccess,
    EscapeAttr, FieldAttr, IndexAttr, IterInitAttr, IterNextAttr, MultiIndexAttr, MultiSetAttr,
    PointerStorageAttr, PrefixAttr, ShuffleAttr, SimdConvertAttr, SimdMakeAttr, SizeOfAttr,
    SliceAttr, UninitAttr, VariantAttr,
};
use super::inventory::{CoreOpKind, importing_op, try_path};
use super::ops::{
    self, FuncOp, KEY_CALL, KEY_CONSTANT, KEY_DEAD_TERM, KEY_DECLARATION, KEY_EXIT, KEY_IDENTITY,
    KEY_INFIX, KEY_INVALIDATION, KEY_LIFECYCLE, KEY_LOANS, KEY_ORPHANS, KEY_PROJECTION,
    KEY_PROVENANCE, KEY_REG, KEY_RESOLVED, KEY_SIGNATURE, KEY_SLOT, KEY_STORE, KEY_TABLES, KEY_TRY,
    KEY_USE_MODE,
};
use super::ops::{
    KEY_CLEANUP, KEY_CLOSURE, KEY_ESCAPE, KEY_FIELD, KEY_ITER_INIT, KEY_ITER_NEXT, KEY_MULTI_INDEX,
    KEY_MULTI_SET, KEY_POINTER_STORAGE, KEY_PREFIX, KEY_SHUFFLE, KEY_SIMD_CONVERT, KEY_SIMD_MAKE,
    KEY_SIZE_OF, KEY_SLICE, KEY_SUBSCRIPT, KEY_UNINIT, KEY_VARIANT,
};
use super::params::{PayloadBinder, import_param};
use super::types::{
    CoreDtype, EffectType, NoneType, PlaceType, import_arg, import_type, scalar_alias,
};
use super::{A1Error, A1ErrorKind};

/// What the importer may convert.
#[derive(Debug, Clone, Default)]
pub struct ImportConfig {
    /// Operations whose conversion rules are switched off; a form that
    /// needs one is rejected before anything is emitted for it.
    pub disabled: Vec<CoreOpKind>,
}

/// Import `program` as a bridge-stage module. `entries` maps each requested
/// entry to the concrete function specialization chose for it.
pub fn import_program(
    ctx: &mut Context,
    program: &MirProgram,
    entries: &[(String, String)],
) -> Result<Ptr<Operation>, A1Error> {
    import_program_with(ctx, program, entries, &ImportConfig::default())
}

/// [`import_program`] under `config`.
pub fn import_program_with(
    ctx: &mut Context,
    program: &MirProgram,
    entries: &[(String, String)],
    config: &ImportConfig,
) -> Result<Ptr<Operation>, A1Error> {
    import_program_in(ctx, program, entries, config, &Sink::default())
}

/// Every refusal the importer has for `program`, counted by what is refused.
///
/// The classes are a form, a type, a constant, a call facet, a
/// declaration, a struct, a positional shape, a per-operation verifier
/// rule, and a legality rule. The importer runs to the end of the input
/// instead of stopping at the first; the module it builds is discarded.
pub fn refusals(program: &MirProgram) -> BTreeMap<String, usize> {
    let mut ctx = super::ir_framework::new_context();
    let sink = Sink {
        collect: true,
        found: RefCell::default(),
    };
    let _ = import_program_in(&mut ctx, program, &[], &ImportConfig::default(), &sink);
    let mut found = BTreeMap::new();
    for message in sink.found.into_inner() {
        // A refused instruction defines nothing, so the reads of its
        // registers are its consequence, not a refusal of their own.
        if message.starts_with("a read of undefined register") {
            continue;
        }
        for key in refusal_keys(&message) {
            *found.entry(key).or_default() += 1;
        }
    }
    found
}

/// Where a refusal goes: back to the caller at once, or into the census
/// of a run that continues past it.
#[derive(Default)]
struct Sink {
    collect: bool,
    found: RefCell<Vec<String>>,
}

impl Sink {
    /// Pass `result` through, or in a census record its refusal and
    /// continue with `placeholder`.
    fn admit<T>(
        &self,
        result: Result<T, A1Error>,
        placeholder: impl FnOnce() -> T,
    ) -> Result<T, A1Error> {
        match result {
            Ok(value) => Ok(value),
            Err(error) if self.collect => {
                self.record(error.message);
                Ok(placeholder())
            }
            Err(error) => Err(error),
        }
    }

    fn record(&self, message: String) {
        self.found.borrow_mut().push(message);
    }
}

fn import_program_in(
    ctx: &mut Context,
    program: &MirProgram,
    entries: &[(String, String)],
    config: &ImportConfig,
    sink: &Sink,
) -> Result<Ptr<Operation>, A1Error> {
    // A census counts each occurrence of a rejected form where it stands
    // instead of stopping at the first form the scan finds.
    for (name, function) in program.functions.iter().filter(|_| !sink.collect) {
        let mut forms = Vec::new();
        collect_forms(&function.blocks, &mut forms);
        for form in forms {
            match importing_op(form) {
                Some(kind) if config.disabled.contains(&kind) => {
                    return Err(A1Error::new(
                        A1ErrorKind::UnsupportedForm,
                        format!(
                            "the conversion rule of `mojito.{}` is disabled, and `{form}` needs it",
                            kind.name()
                        ),
                    )
                    .in_function(name));
                }
                Some(_) => {}
                None => {
                    return Err(A1Error::new(
                        A1ErrorKind::UnsupportedForm,
                        format!("`{form}` has no core form"),
                    )
                    .in_function(name));
                }
            }
        }
    }
    if !program.invariant_errors.is_empty() {
        let violated = Err(A1Error::new(
            A1ErrorKind::UnsupportedForm,
            format!(
                "the input violates the checked-program contract: {}",
                program.invariant_errors.join("; ")
            ),
        ));
        sink.admit(violated, || ())?;
    }
    let name = "a1"
        .try_into()
        .map_err(|_| A1Error::new(A1ErrorKind::UnsupportedForm, "module name"))?;
    let module = ModuleOp::new(ctx, name);
    let mut structs = Vec::new();
    for decl in &program.declarations.structs {
        let imported = import_struct(ctx, decl).map(Some);
        let imported = sink.admit(imported.map_err(|error| error.classified("struct")), || {
            None
        })?;
        structs.extend(imported);
    }
    let tables = ModuleAttr {
        schema: SCHEMA.into(),
        sources: sources(program).into_iter().map(Text::from).collect(),
        entries: entries
            .iter()
            .map(|(requested, concrete)| CoreEntry {
                requested: requested.into(),
                concrete: concrete.into(),
            })
            .collect(),
        structs,
        declaration_order: program
            .declarations
            .functions
            .iter()
            .map(|decl| Text::from(&decl.lowered_name))
            .collect(),
    };
    set(ctx, module.get_operation(), &KEY_TABLES, tables.clone());
    annotate(
        ctx,
        module.get_operation(),
        IdentityAttr {
            function: Text::default(),
            region: Text::default(),
            block: 0,
            ordinal: 0,
            role: CoreRole::Module,
        },
        ProvenanceAttr {
            span: None,
            origin: None,
            derived_from: None,
            reason: "module".into(),
        },
    );
    let body = module
        .get_region(ctx)
        .deref(ctx)
        .get_entry_block()
        .ok_or_else(|| A1Error::new(A1ErrorKind::UnsupportedForm, "module without a block"))?;
    let declarations: HashMap<&str, &MirFunctionDeclaration> = program
        .declarations
        .functions
        .iter()
        .map(|decl| (decl.lowered_name.as_str(), decl))
        .collect();
    for decl in &program.declarations.functions {
        if !program
            .functions
            .iter()
            .any(|(name, _)| *name == decl.lowered_name)
        {
            let bodiless = Err(A1Error::new(
                A1ErrorKind::UnsupportedForm,
                "declaration: a declaration without a body has no core form",
            )
            .in_function(&decl.lowered_name));
            sink.admit(bodiless, || ())?;
        }
    }
    let symbols: Vec<Text> = program
        .functions
        .iter()
        .map(|(name, _)| Text::from(name))
        .collect();
    for (index, (name, function)) in program.functions.iter().enumerate() {
        let func = FnImport::run(
            ctx,
            index,
            name,
            function,
            declarations.get(name.as_str()),
            &tables.sources,
            sink,
        )
        .map_err(|error| error.in_function(name));
        let Some(func) = sink.admit(func.map(Some), || None)? else {
            continue;
        };
        if sink.collect {
            collect_legality(ctx, func, &tables, &symbols, sink);
        }
        func.insert_at_back(body, ctx);
    }
    Ok(module.get_operation())
}

/// Record the legality violations of `func`'s operations: the projection
/// and call-target rules `verify::legality_violations` applies to a whole
/// module, applied here to a function of a census.
fn collect_legality(
    ctx: &Context,
    func: Ptr<Operation>,
    tables: &ModuleAttr,
    symbols: &[Text],
    sink: &Sink,
) {
    for op in super::verify::walk(ctx, func) {
        if let Err(error) = super::verify::check_projection(ctx, op, tables) {
            sink.record(format!("legality: {}", error.message));
        }
        let call = op.deref(ctx).attributes.get::<CallAttr>(&KEY_CALL).cloned();
        if let Some(call) = call
            && !super::verify::call_target_known(&call, symbols, &tables.structs)
        {
            sink.record(format!(
                "legality: call target `{}` resolves to no function, struct, or builtin",
                super::verify::call_target(&call)
            ));
        }
    }
}

/// The v1 mnemonic of every instruction and terminator of `blocks`,
/// nested regions included.
fn collect_forms(blocks: &[MirBlock], forms: &mut Vec<&'static str>) {
    for block in blocks {
        for instruction in &block.instrs {
            forms.push(instruction_mnemonic(instruction));
            if let MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } = instruction
            {
                collect_forms(body, forms);
                for part in [
                    handler.as_ref().map(|(_, blocks)| blocks),
                    orelse.as_ref(),
                    finalbody.as_ref(),
                ]
                .into_iter()
                .flatten()
                {
                    collect_forms(part, forms);
                }
            }
        }
        forms.push(terminator_mnemonic(&block.term));
    }
}

/// Set attribute `value` under `key` on `op`.
pub fn set<T: pliron::attribute::Attribute>(
    ctx: &Context,
    op: Ptr<Operation>,
    key: &pliron::identifier::Identifier,
    value: T,
) {
    op.deref_mut(ctx).attributes.set(key.clone(), value);
}

/// Give `op` its identity and provenance.
pub fn annotate(
    ctx: &Context,
    op: Ptr<Operation>,
    identity: IdentityAttr,
    provenance: ProvenanceAttr,
) {
    set(ctx, op, &KEY_IDENTITY, identity);
    set(ctx, op, &KEY_PROVENANCE, provenance);
}

/// The registers of `function` that cannot be a direct SSA value: defined
/// more than once, or read outside the block that defines them.
pub fn slot_registers(function: &MirFunction) -> Result<BTreeSet<u32>, A1Error> {
    let mut sites = RegisterSites::default();
    sites.walk("", &function.blocks);
    let mut slots = BTreeSet::new();
    for (reg, uses) in &sites.uses {
        let Some(defs) = sites.defs.get(reg) else {
            return Err(A1Error::new(
                A1ErrorKind::UnsupportedForm,
                format!("register %r{reg} is read but never defined"),
            ));
        };
        let direct = defs.len() == 1
            && uses.iter().all(|site| {
                site.region == defs[0].region
                    && site.block == defs[0].block
                    && site.ordinal > defs[0].ordinal
            });
        if !direct {
            slots.insert(*reg);
        }
    }
    slots.extend(
        sites
            .defs
            .iter()
            .filter(|(_, defs)| defs.len() > 1)
            .map(|(reg, _)| *reg),
    );
    Ok(slots)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Site {
    region: String,
    block: usize,
    ordinal: usize,
}

#[derive(Default)]
struct RegisterSites {
    defs: BTreeMap<u32, Vec<Site>>,
    uses: BTreeMap<u32, Vec<Site>>,
}

impl RegisterSites {
    fn walk(&mut self, region: &str, blocks: &[MirBlock]) {
        let mut tries = 0usize;
        for (block, contents) in blocks.iter().enumerate() {
            for (ordinal, instruction) in contents.instrs.iter().enumerate() {
                let site = Site {
                    region: region.to_string(),
                    block,
                    ordinal,
                };
                let mut regs = Vec::new();
                instruction_operand_regs(instruction, &mut regs);
                for reg in std::mem::take(&mut regs) {
                    self.uses.entry(reg.0).or_default().push(site.clone());
                }
                instruction_result_regs(instruction, &mut regs);
                for reg in regs {
                    self.defs.entry(reg.0).or_default().push(site.clone());
                }
                if let MirInstr::Try {
                    body,
                    handler,
                    orelse,
                    finalbody,
                    ..
                } = instruction
                {
                    let prefix = try_path(region, tries);
                    tries += 1;
                    self.walk(&format!("{prefix}.body"), body);
                    if let Some((_, blocks)) = handler {
                        self.walk(&format!("{prefix}.handler"), blocks);
                    }
                    if let Some(blocks) = orelse {
                        self.walk(&format!("{prefix}.else"), blocks);
                    }
                    if let Some(blocks) = finalbody {
                        self.walk(&format!("{prefix}.finally"), blocks);
                    }
                }
            }
            let site = Site {
                region: region.to_string(),
                block,
                ordinal: contents.instrs.len(),
            };
            let read = match &contents.term {
                MirTerm::Branch { cond, .. } => Some(*cond),
                MirTerm::Return(value) | MirTerm::ReturnWithCleanup { value, .. } => *value,
                MirTerm::Jump(_) | MirTerm::FallOff | MirTerm::EscapeJump { .. } => None,
            };
            if let Some(reg) = read {
                self.uses.entry(reg.0).or_default().push(site);
            }
        }
    }
}

/// The import of one function.
struct FnImport<'a> {
    name: &'a str,
    function: &'a MirFunction,
    slot_regs: BTreeSet<u32>,
    var_slots: Vec<Value>,
    reg_slots: HashMap<u32, Value>,
    values: HashMap<u32, Value>,
    effect: TypeHandle,
    /// The block being filled and the effect token current in it.
    block: Ptr<BasicBlock>,
    token: Value,
    region: String,
    mir_block: usize,
    ordinal: usize,
    /// Register loads emitted for the instruction being imported.
    loads: u64,
    places: u64,
    /// The function's source records, their sources resolved to indices
    /// into the module's `sources`.
    records: HashMap<u32, (CoreSpan, Option<u32>)>,
    sink: &'a Sink,
}

impl<'a> FnImport<'a> {
    fn run(
        ctx: &mut Context,
        index: usize,
        name: &'a str,
        function: &'a MirFunction,
        declaration: Option<&&MirFunctionDeclaration>,
        sources: &'a [Text],
        sink: &'a Sink,
    ) -> Result<Ptr<Operation>, A1Error> {
        let effect: TypeHandle = EffectType::get(ctx).into();
        let mut params = Vec::with_capacity(function.param_types.len());
        for ty in &function.param_types {
            let imported = import_type(ctx, ty);
            params.push(sink.admit(imported, || NoneType::get(ctx).into())?);
        }
        if params.len() != function.n_params {
            return Err(A1Error::new(
                A1ErrorKind::UnsupportedForm,
                "a parameter without a checked type",
            ));
        }
        let records = function
            .spans
            .0
            .iter()
            .map(|(reg, (span, origin))| Ok((*reg, (CoreSpan::from_span(span, sources)?, *origin))))
            .collect::<Result<HashMap<_, _>, A1Error>>()?;
        let func = ops::build(ctx, CoreOpKind::Func, vec![], vec![], vec![], 1);
        let symbol = format!("f{index}")
            .as_str()
            .try_into()
            .map_err(|_| A1Error::new(A1ErrorKind::UnsupportedForm, "function symbol"))?;
        FuncOp::from_operation(func).set_symbol_name(ctx, symbol);
        let ret = function
            .ret_ty
            .as_ref()
            .map(|ty| import_type(ctx, ty))
            .transpose();
        let ret = sink.admit(ret, || None)?;
        let error = function
            .error_ty
            .as_ref()
            .map(|ty| import_type(ctx, ty))
            .transpose();
        let error = sink.admit(error, || None)?;
        set(
            ctx,
            func,
            &KEY_SIGNATURE,
            SignatureAttr {
                symbol: name.into(),
                registers: function.n_regs,
                ret,
                returns_reference: function.returns_reference,
                raises: function.raises,
                error,
                params: params.clone(),
            },
        );
        if let Some(declaration) = declaration {
            let imported = import_declaration(ctx, declaration)
                .map(Some)
                .map_err(|error| error.classified("declaration"));
            if let Some(declaration) = sink.admit(imported, || None)? {
                set(ctx, func, &KEY_DECLARATION, declaration);
            }
        }
        annotate(
            ctx,
            func,
            IdentityAttr {
                function: name.into(),
                region: Text::default(),
                block: 0,
                ordinal: 0,
                role: CoreRole::Function,
            },
            ProvenanceAttr {
                span: None,
                origin: None,
                derived_from: None,
                reason: "function".into(),
            },
        );
        let region = func.deref(ctx).get_region(0);
        let mut arguments = params;
        arguments.push(effect);
        let entry = BasicBlock::new(ctx, None, arguments);
        entry.insert_at_back(region, ctx);
        let token = entry.deref(ctx).get_argument(function.n_params);
        let slot_regs = sink.admit(slot_registers(function), BTreeSet::new)?;
        let mut import = Self {
            name,
            function,
            slot_regs,
            var_slots: Vec::new(),
            reg_slots: HashMap::new(),
            values: HashMap::new(),
            effect,
            block: entry,
            token,
            region: String::new(),
            mir_block: 0,
            ordinal: 0,
            loads: 0,
            places: 0,
            records,
            sink,
        };
        import.declare_slots(ctx, entry)?;
        let filled = import.region_blocks(ctx, region, "", &function.blocks, entry);
        sink.admit(filled, || ())?;
        let orphans = import.orphans(ctx);
        let orphans = sink.admit(orphans, Vec::new)?;
        set(ctx, func, &KEY_ORPHANS, OrphansAttr(orphans));
        Ok(func)
    }

    fn declare_slots(&mut self, ctx: &mut Context, entry: Ptr<BasicBlock>) -> Result<(), A1Error> {
        let function = self.function;
        let mut ordinal = 0u64;
        for var in 0..function.n_vars {
            let id = u32::try_from(var)
                .map_err(|_| A1Error::new(A1ErrorKind::UnsupportedForm, "variable count"))?;
            let ty = function.var_tys.get(&id).ok_or_else(|| {
                A1Error::new(
                    A1ErrorKind::UnsupportedForm,
                    format!(
                        "variable `{}` has no checked type",
                        var_name(&function.var_names, id)
                    ),
                )
            })?;
            let imported = import_type(ctx, ty);
            let target = self.sink.admit(imported, || NoneType::get(ctx).into())?;
            let param = (var < function.n_params).then_some(id);
            let flag = |flags: &[bool]| flags.get(var).copied().unwrap_or(false);
            let operands = param
                .map(|_| vec![entry.deref(ctx).get_argument(var)])
                .unwrap_or_default();
            let slot = self.slot(
                ctx,
                target,
                operands,
                SlotAttr {
                    storage: CoreStorage::Variable,
                    id,
                    name: var_name(&function.var_names, id),
                    param,
                    owned: flag(&function.owned_params),
                    deinit: flag(&function.deinit_params),
                    by_ref: flag(&function.ref_params),
                },
                ordinal,
            );
            self.var_slots.push(slot);
            ordinal += 1;
        }
        for reg in self.slot_regs.clone() {
            let imported = self.reg_type(Reg(reg)).and_then(|ty| import_type(ctx, ty));
            let target = self.sink.admit(imported, || NoneType::get(ctx).into())?;
            let slot = self.slot(
                ctx,
                target,
                Vec::new(),
                SlotAttr {
                    storage: CoreStorage::Register,
                    id: reg,
                    name: Text::default(),
                    param: None,
                    owned: false,
                    deinit: false,
                    by_ref: false,
                },
                ordinal,
            );
            self.reg_slots.insert(reg, slot);
            ordinal += 1;
        }
        Ok(())
    }

    fn slot(
        &self,
        ctx: &mut Context,
        target: TypeHandle,
        operands: Vec<Value>,
        slot: SlotAttr,
        ordinal: u64,
    ) -> Value {
        let place = PlaceType::get(ctx, target).into();
        let op = ops::build(ctx, CoreOpKind::Slot, vec![place], operands, vec![], 0);
        set(ctx, op, &KEY_SLOT, slot);
        annotate(
            ctx,
            op,
            IdentityAttr {
                function: self.name.into(),
                region: Text::default(),
                block: 0,
                ordinal: 0,
                role: CoreRole::Slot(ordinal),
            },
            ProvenanceAttr {
                span: None,
                origin: None,
                derived_from: None,
                reason: "slot".into(),
            },
        );
        op.insert_at_back(self.block, ctx);
        op.deref(ctx).get_result(0)
    }

    /// Create the blocks of one region behind a synthesized entry, fill
    /// them, and branch from `entry` to block 0.
    fn region_blocks(
        &mut self,
        ctx: &mut Context,
        region: Ptr<Region>,
        path: &str,
        blocks: &[MirBlock],
        entry: Ptr<BasicBlock>,
    ) -> Result<(), A1Error> {
        let targets: Vec<Ptr<BasicBlock>> = blocks
            .iter()
            .map(|_| {
                let block = BasicBlock::new(ctx, None, vec![self.effect]);
                block.insert_at_back(region, ctx);
                block
            })
            .collect();
        let Some(first) = targets.first() else {
            return Err(A1Error::new(
                A1ErrorKind::UnsupportedForm,
                "a region without a block",
            ));
        };
        let saved = (
            self.block,
            self.token,
            self.region.clone(),
            self.mir_block,
            self.ordinal,
        );
        self.region = path.to_string();
        self.mir_block = 0;
        self.ordinal = 0;
        self.block = entry;
        if !path.is_empty() {
            self.token = entry.deref(ctx).get_argument(0);
        }
        let branch = ops::build(
            ctx,
            CoreOpKind::Br,
            vec![],
            vec![self.token],
            vec![*first],
            0,
        );
        self.finish(ctx, branch, CoreRole::Entry, None, "region-entry");
        let mut tries = 0usize;
        for (index, contents) in blocks.iter().enumerate() {
            self.block = targets[index];
            self.token = targets[index].deref(ctx).get_argument(0);
            self.mir_block = index;
            self.values.clear();
            let filled = self.block_contents(ctx, contents, &targets, &mut tries);
            self.sink.admit(filled, || ())?;
        }
        (
            self.block,
            self.token,
            self.region,
            self.mir_block,
            self.ordinal,
        ) = saved;
        Ok(())
    }

    fn block_contents(
        &mut self,
        ctx: &mut Context,
        contents: &MirBlock,
        targets: &[Ptr<BasicBlock>],
        tries: &mut usize,
    ) -> Result<(), A1Error> {
        for (ordinal, instruction) in contents.instrs.iter().enumerate() {
            self.ordinal = ordinal;
            self.loads = 0;
            self.places = 0;
            if let MirInstr::Raise { src } = instruction {
                let raised = self.raise(ctx, *src, ordinal, contents);
                return self.sink.admit(raised, || ());
            }
            let imported = self.instruction(ctx, instruction, tries);
            self.sink.admit(imported, || ())?;
        }
        self.ordinal = contents.instrs.len();
        self.loads = 0;
        self.places = 0;
        let ended = self.terminator(ctx, &contents.term, targets);
        self.sink.admit(ended, || ())
    }

    /// A `raise` ends its block: the terminator after it is dead and is
    /// recorded on the operation.
    fn raise(
        &mut self,
        ctx: &mut Context,
        src: Reg,
        ordinal: usize,
        contents: &MirBlock,
    ) -> Result<(), A1Error> {
        if ordinal + 1 != contents.instrs.len() {
            return Err(self.unsupported("instructions after a `raise` in its block"));
        }
        let dead = match &contents.term {
            MirTerm::Jump(target) => DeadTermAttr::Jump(*target as u64),
            MirTerm::Return(None) => DeadTermAttr::Return,
            MirTerm::FallOff => DeadTermAttr::FallOff,
            other => {
                return Err(self.unsupported(format!(
                    "a `raise` cutting off `{}`",
                    terminator_mnemonic(other)
                )));
            }
        };
        let error = self.value(ctx, src)?;
        let op = ops::build(
            ctx,
            CoreOpKind::Raise,
            vec![],
            vec![error, self.token],
            vec![],
            0,
        );
        set(ctx, op, &KEY_DEAD_TERM, dead);
        self.finish(ctx, op, CoreRole::Primary, None, "no-source-record");
        Ok(())
    }

    fn terminator(
        &mut self,
        ctx: &mut Context,
        term: &MirTerm,
        targets: &[Ptr<BasicBlock>],
    ) -> Result<(), A1Error> {
        let target = |index: usize| {
            targets.get(index).copied().ok_or_else(|| {
                A1Error::new(
                    A1ErrorKind::UnsupportedForm,
                    format!("a branch to missing block bb{index}"),
                )
            })
        };
        let op = match term {
            MirTerm::Jump(index) => {
                let successor = target(*index)?;
                ops::build(
                    ctx,
                    CoreOpKind::Br,
                    vec![],
                    vec![self.token],
                    vec![successor],
                    0,
                )
            }
            MirTerm::Branch {
                cond,
                then_b,
                else_b,
            } => {
                let successors = vec![target(*then_b)?, target(*else_b)?];
                let condition = self.value(ctx, *cond)?;
                ops::build(
                    ctx,
                    CoreOpKind::CondBr,
                    vec![],
                    vec![condition, self.token],
                    successors,
                    0,
                )
            }
            MirTerm::Return(value) => {
                let mut operands = Vec::new();
                if let Some(value) = value {
                    operands.push(self.value(ctx, *value)?);
                }
                operands.push(self.token);
                ops::build(ctx, CoreOpKind::Return, vec![], operands, vec![], 0)
            }
            MirTerm::FallOff => {
                if self.region.is_empty() {
                    return Err(self.unsupported("a `falloff` outside a try region"));
                }
                let op = ops::build(
                    ctx,
                    CoreOpKind::RegionExit,
                    vec![],
                    vec![self.token],
                    vec![],
                    0,
                );
                set(ctx, op, &KEY_EXIT, ExitAttr::FallOff);
                op
            }
            MirTerm::ReturnWithCleanup { value, cleanup } => {
                let mut operands = Vec::new();
                if let Some(value) = value {
                    operands.push(self.value(ctx, *value)?);
                }
                for var in cleanup {
                    operands.push(self.var_slot(*var)?);
                }
                operands.push(self.token);
                let op = ops::build(ctx, CoreOpKind::Return, vec![], operands, vec![], 0);
                set(ctx, op, &KEY_CLEANUP, CleanupAttr(cleanup.len() as u64));
                op
            }
            MirTerm::EscapeJump { target, cleanup } => {
                if self.region.is_empty() {
                    return Err(self.unsupported("an `escape` outside a try region"));
                }
                let mut operands = cleanup
                    .iter()
                    .map(|var| self.var_slot(*var))
                    .collect::<Result<Vec<_>, _>>()?;
                operands.push(self.token);
                let op = ops::build(ctx, CoreOpKind::Escape, vec![], operands, vec![], 0);
                set(
                    ctx,
                    op,
                    &KEY_ESCAPE,
                    EscapeAttr {
                        target: *target as u64,
                    },
                );
                set(ctx, op, &KEY_CLEANUP, CleanupAttr(cleanup.len() as u64));
                op
            }
        };
        self.finish(ctx, op, CoreRole::Primary, None, "no-source-record");
        Ok(())
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one exhaustive match is the conversion-totality contract"
    )]
    fn instruction(
        &mut self,
        ctx: &mut Context,
        instruction: &MirInstr,
        tries: &mut usize,
    ) -> Result<(), A1Error> {
        match instruction {
            MirInstr::Const { dest, k } => {
                let ty = self.result_type(ctx, *dest)?;
                let op = ops::build(ctx, CoreOpKind::Const, vec![ty], vec![], vec![], 0);
                set(ctx, op, &KEY_CONSTANT, ConstAttr::from_const(k)?);
                self.define(ctx, op, *dest, false);
            }
            MirInstr::MaterializeLiteral {
                dest,
                value,
                target,
            } => {
                let target = import_type(ctx, target)?;
                if target != self.result_type(ctx, *dest)? {
                    return Err(self.unsupported(
                        "a literal materialized at a type other than its register's",
                    ));
                }
                let literal = self.value(ctx, *value)?;
                let op = ops::build(
                    ctx,
                    CoreOpKind::Materialize,
                    vec![target],
                    vec![literal],
                    vec![],
                    0,
                );
                self.define(ctx, op, *dest, false);
            }
            MirInstr::BinOp {
                op: infix,
                dest,
                a,
                b,
                resolved,
            } => {
                let ty = self.result_type(ctx, *dest)?;
                let operands = vec![self.value(ctx, *a)?, self.value(ctx, *b)?];
                let op = ops::build(ctx, CoreOpKind::Binary, vec![ty], operands, vec![], 0);
                set(ctx, op, &KEY_INFIX, InfixAttr::from_op(*infix));
                set(
                    ctx,
                    op,
                    &KEY_RESOLVED,
                    ResolvedAttr(resolved.clone().map(Text::from)),
                );
                self.define(ctx, op, *dest, false);
            }
            MirInstr::MakeSimd {
                dest,
                dtype,
                width,
                elems,
            } => {
                let ty = self.result_type(ctx, *dest)?;
                let lane = CoreDtype::from_dtype(*dtype);
                let declared = import_type(
                    ctx,
                    &mojito_types::types::Ty::Simd {
                        dtype: mojito_types::types::SimdDtype::Known(*dtype),
                        width: mojito_types::types::SimdWidth::Known(*width as i64),
                    },
                )?;
                let aliased = *width == 1 && scalar_alias(ctx, lane) == Some(ty);
                if declared != ty && !aliased {
                    return Err(
                        self.unsupported("a SIMD construction at a type other than its register's")
                    );
                }
                let operands = elems
                    .iter()
                    .map(|reg| self.value(ctx, *reg))
                    .collect::<Result<Vec<_>, _>>()?;
                let op = ops::build(ctx, CoreOpKind::SimdMake, vec![ty], operands, vec![], 0);
                set(
                    ctx,
                    op,
                    &KEY_SIMD_MAKE,
                    SimdMakeAttr {
                        dtype: lane,
                        width: *width as u64,
                    },
                );
                self.define(ctx, op, *dest, false);
            }
            MirInstr::UseVar { dest, var, mode } => {
                let ty = self.result_type(ctx, *dest)?;
                let place = self.var_slot(*var)?;
                let op = ops::build(
                    ctx,
                    CoreOpKind::Use,
                    vec![ty, self.effect],
                    vec![place, self.token],
                    vec![],
                    0,
                );
                set(ctx, op, &KEY_USE_MODE, UseModeAttr::from_mode(*mode));
                self.define(ctx, op, *dest, true);
            }
            MirInstr::DefVar {
                var,
                src,
                binding_ty,
            } => {
                let binding = binding_ty
                    .as_ref()
                    .map(|ty| import_type(ctx, ty))
                    .transpose()?;
                let value = self.value(ctx, *src)?;
                let place = self.var_slot(*var)?;
                self.effectful(ctx, CoreOpKind::Store, vec![place, value], |ctx, op| {
                    set(ctx, op, &KEY_STORE, StoreAttr::DefVar { binding });
                });
            }
            MirInstr::Store { place, src } => {
                let value = self.value(ctx, *src)?;
                let place = self.place(ctx, place)?;
                self.effectful(ctx, CoreOpKind::Store, vec![place, value], |ctx, op| {
                    set(ctx, op, &KEY_STORE, StoreAttr::Place);
                });
            }
            MirInstr::StoreRef { place, reference } => {
                let reference = self.value(ctx, *reference)?;
                let place = self.place(ctx, place)?;
                self.effectful(ctx, CoreOpKind::RefStore, vec![place, reference], |_, _| {});
            }
            MirInstr::LoadPlace { dest, place } => {
                let ty = self.result_type(ctx, *dest)?;
                let place = self.place(ctx, place)?;
                let op = ops::build(
                    ctx,
                    CoreOpKind::Load,
                    vec![ty, self.effect],
                    vec![place, self.token],
                    vec![],
                    0,
                );
                self.define(ctx, op, *dest, true);
            }
            MirInstr::MakeRef { dest, place } => {
                let ty = self.result_type(ctx, *dest)?;
                let place = self.place(ctx, place)?;
                let op = ops::build(ctx, CoreOpKind::RefMake, vec![ty], vec![place], vec![], 0);
                self.define(ctx, op, *dest, false);
            }
            MirInstr::Call {
                dest,
                func,
                raises,
                args,
                kwargs,
                arg_places,
                kwarg_places,
                capture_accesses,
                param_arg_regs,
            } => {
                self.admit_call(instruction)?;
                let facts = CallFacts {
                    kind: CoreCallKind::Direct,
                    target: func.0.clone(),
                    resolved: None,
                    raises: raises.as_ref(),
                    recv: None,
                    recv_place: None,
                    recv_writes: false,
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                    params: param_arg_regs,
                    captures: capture_accesses,
                    contract: None,
                    instantiated: &[],
                    reference_result: None,
                    adapter: None,
                };
                self.call(ctx, *dest, &facts)?;
            }
            MirInstr::CallIndirect {
                dest,
                callee,
                resolved,
                raises,
                args,
                kwargs,
                callee_place,
                arg_places,
                kwarg_places,
                capture_accesses,
                param_arg_regs,
                param_decls: _,
                instantiated_contract,
                instantiated_args,
            } => {
                self.admit_call(instruction)?;
                let facts = CallFacts {
                    kind: CoreCallKind::Indirect,
                    target: String::new(),
                    resolved: resolved.clone(),
                    raises: raises.as_ref(),
                    recv: Some(*callee),
                    recv_place: callee_place.as_ref(),
                    recv_writes: false,
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                    params: param_arg_regs,
                    captures: capture_accesses,
                    contract: instantiated_contract.as_ref(),
                    instantiated: instantiated_args,
                    reference_result: None,
                    adapter: None,
                };
                self.call(ctx, *dest, &facts)?;
            }
            MirInstr::MakeClosure {
                dest,
                function,
                captures,
            } => {
                let mut operands = Vec::with_capacity(captures.len());
                for capture in captures {
                    operands.push(self.place(ctx, &capture.place)?);
                }
                let facts = ClosureAttr {
                    function: Text::from(function),
                    modes: captures
                        .iter()
                        .map(|capture| CoreCaptureMode::from_mode(capture.mode))
                        .collect(),
                };
                self.effectful_value(ctx, CoreOpKind::ClosureMake, operands, *dest, |ctx, op| {
                    set(ctx, op, &KEY_CLOSURE, facts);
                })?;
            }
            MirInstr::MethodCall {
                dest,
                recv,
                method,
                resolved,
                raises,
                reference_result,
                result_adapter,
                args,
                kwargs,
                recv_place,
                recv_writes,
                arg_places,
                kwarg_places,
                capture_accesses,
                param_arg_regs,
                param_decls,
            } => {
                let _ = param_decls;
                self.admit_call(instruction)?;
                let facts = CallFacts {
                    kind: CoreCallKind::Method,
                    target: method.clone(),
                    resolved: resolved.clone(),
                    raises: raises.as_ref(),
                    recv: Some(*recv),
                    recv_place: recv_place.as_ref(),
                    recv_writes: *recv_writes,
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                    params: param_arg_regs,
                    captures: capture_accesses,
                    contract: None,
                    instantiated: &[],
                    reference_result: reference_result.as_ref(),
                    adapter: *result_adapter,
                };
                self.call(ctx, *dest, &facts)?;
            }
            MirInstr::UnOp {
                op: prefix,
                dest,
                a,
            } => {
                let ty = self.result_type(ctx, *dest)?;
                let operand = self.value(ctx, *a)?;
                let op = ops::build(ctx, CoreOpKind::Unary, vec![ty], vec![operand], vec![], 0);
                set(ctx, op, &KEY_PREFIX, PrefixAttr::from_op(*prefix));
                self.define(ctx, op, *dest, false);
            }
            MirInstr::SimdCast {
                dest,
                value,
                dtype,
                width,
            }
            | MirInstr::SimdBitcast {
                dest,
                value,
                dtype,
                width,
            } => {
                let ty = self.result_type(ctx, *dest)?;
                let lane = CoreDtype::from_dtype(*dtype);
                let declared = import_type(
                    ctx,
                    &mojito_types::types::Ty::Simd {
                        dtype: mojito_types::types::SimdDtype::Known(*dtype),
                        width: mojito_types::types::SimdWidth::Known(*width as i64),
                    },
                )?;
                let aliased = *width == 1 && scalar_alias(ctx, lane) == Some(ty);
                if declared != ty && !aliased {
                    return Err(
                        self.unsupported("a SIMD conversion to a type other than its register's")
                    );
                }
                let conversion = match instruction {
                    MirInstr::SimdCast { .. } => CoreSimdConversion::Cast,
                    _ => CoreSimdConversion::Bits,
                };
                let operand = self.value(ctx, *value)?;
                let op = ops::build(
                    ctx,
                    CoreOpKind::SimdConvert,
                    vec![ty],
                    vec![operand],
                    vec![],
                    0,
                );
                set(
                    ctx,
                    op,
                    &KEY_SIMD_CONVERT,
                    SimdConvertAttr {
                        conversion,
                        dtype: lane,
                        width: *width as u64,
                    },
                );
                self.define(ctx, op, *dest, false);
            }
            MirInstr::ReadRef { dest, reference } => {
                let operand = self.value(ctx, *reference)?;
                self.effectful_value(ctx, CoreOpKind::RefRead, vec![operand], *dest, |_, _| {})?;
            }
            MirInstr::CopyValue { dest, value } => {
                let operand = self.value(ctx, *value)?;
                self.effectful_value(ctx, CoreOpKind::Copy, vec![operand], *dest, |_, _| {})?;
            }
            MirInstr::MovePlace { dest, place } => {
                let place = self.place(ctx, place)?;
                self.effectful_value(ctx, CoreOpKind::Move, vec![place], *dest, |_, _| {})?;
            }
            MirInstr::KeepAlive { var } => {
                let place = self.var_slot(*var)?;
                self.effectful(ctx, CoreOpKind::KeepAlive, vec![place], |_, _| {});
            }
            MirInstr::PointerStorageTake {
                dest,
                pointer,
                index,
                element,
            }
            | MirInstr::PointerStorageDestroy {
                dest,
                pointer,
                index,
                element,
            } => {
                let access = match instruction {
                    MirInstr::PointerStorageTake { .. } => CorePointerAccess::Take,
                    _ => CorePointerAccess::Destroy,
                };
                let element = import_type(ctx, element)?;
                let operands = vec![self.value(ctx, *pointer)?, self.value(ctx, *index)?];
                self.effectful_value(
                    ctx,
                    CoreOpKind::PointerStorage,
                    operands,
                    *dest,
                    |ctx, op| {
                        set(
                            ctx,
                            op,
                            &KEY_POINTER_STORAGE,
                            PointerStorageAttr { access, element },
                        );
                    },
                )?;
            }
            MirInstr::Index {
                dest,
                base,
                index,
                base_place,
                index_place,
                call,
                intrinsic,
            } => {
                let mut operands = vec![self.value(ctx, *base)?, self.value(ctx, *index)?];
                let call = call
                    .as_ref()
                    .map(|call| self.subscript_call(ctx, call, &mut operands))
                    .transpose()?;
                for place in base_place.iter().chain(index_place) {
                    operands.push(self.place(ctx, place)?);
                }
                let facts = IndexAttr {
                    base_place: base_place.is_some(),
                    index_place: index_place.is_some(),
                    call,
                    intrinsic: intrinsic.map(CoreIntrinsic::from_intrinsic),
                };
                self.effectful_value(ctx, CoreOpKind::Index, operands, *dest, |ctx, op| {
                    set(ctx, op, &KEY_SUBSCRIPT, facts);
                })?;
            }
            MirInstr::MultiSet {
                receiver,
                receiver_place,
                args,
                arg_places,
                value,
                value_place,
                value_keyword,
                call,
            } => {
                if arg_places.len() != args.len() {
                    return Err(
                        self.unsupported("a subscript store whose places misalign its arguments")
                    );
                }
                let mut operands = vec![self.value(ctx, *receiver)?];
                for argument in args {
                    let MirSubscriptArg::Index(reg) = argument else {
                        return Err(self.unsupported("a subscript store through a slice"));
                    };
                    operands.push(self.value(ctx, *reg)?);
                }
                operands.push(self.value(ctx, *value)?);
                let call = self.subscript_call(ctx, call, &mut operands)?;
                let places = receiver_place
                    .iter()
                    .chain(arg_places.iter().flatten())
                    .chain(value_place);
                for place in places {
                    operands.push(self.place(ctx, place)?);
                }
                let facts = MultiSetAttr {
                    receiver_place: receiver_place.is_some(),
                    arg_places: arg_places.iter().map(Option::is_some).collect(),
                    value_place: value_place.is_some(),
                    value_keyword: *value_keyword,
                    call,
                };
                self.effectful(ctx, CoreOpKind::MultiSet, operands, |ctx, op| {
                    set(ctx, op, &KEY_MULTI_SET, facts);
                });
            }
            MirInstr::GetIter {
                source,
                dest,
                mode,
                prepare,
            } => {
                let operands = vec![self.var_slot(*source)?, self.var_slot(*dest)?];
                let facts = IterInitAttr {
                    mode: match mode {
                        IterationMode::Borrowed => CoreIterationMode::Borrowed,
                        IterationMode::Owned => CoreIterationMode::Owned,
                    },
                    prepare: prepare.iter().map(Text::from).collect(),
                };
                self.effectful(ctx, CoreOpKind::IterInit, operands, |ctx, op| {
                    set(ctx, op, &KEY_ITER_INIT, facts);
                });
            }
            MirInstr::TryNext {
                dest,
                yielded,
                iter,
                call,
                exhaustion,
            } => {
                let results = vec![
                    self.result_type(ctx, *dest)?,
                    self.result_type(ctx, *yielded)?,
                    self.effect,
                ];
                let record = self.records.get(&yielded.0);
                let facts = IterNextAttr {
                    target: Text::from(&call.target),
                    result: import_type(ctx, &call.result_ty)?,
                    reference_result: call
                        .reference_result
                        .as_ref()
                        .map(|reference| {
                            import_type(ctx, &mojito_types::types::Ty::Ref(reference.clone()))
                        })
                        .transpose()?,
                    raises: call
                        .raises
                        .as_ref()
                        .map(|ty| import_type(ctx, ty))
                        .transpose()?,
                    adapter: call.result_adapter.map(|adapter| match adapter {
                        CheckedResultAdapter::CopyIteratorReference => {
                            CoreResultAdapter::CopyIteratorReference
                        }
                    }),
                    exhaustion: import_type(ctx, exhaustion)?,
                    yielded: yielded.0,
                    yielded_record: CoreOrphanSpan {
                        span: record.map(|(span, _)| span.clone()),
                        origin: record.and_then(|(_, origin)| *origin),
                    },
                };
                let operands = vec![self.var_slot(*iter)?, self.token];
                let op = ops::build(ctx, CoreOpKind::IterNext, results, operands, vec![], 0);
                set(ctx, op, &KEY_ITER_NEXT, facts);
                set(ctx, op, &KEY_REG, RegAttr(dest.0));
                let element = op.deref(ctx).get_result(0);
                let has_element = op.deref(ctx).get_result(1);
                let token = op.deref(ctx).get_result(2);
                self.finish(ctx, op, CoreRole::Primary, Some(*dest), "source");
                self.token = token;
                self.bind_as(ctx, *dest, element, CoreRole::RegisterStore);
                self.bind_as(ctx, *yielded, has_element, CoreRole::YieldStore);
            }
            MirInstr::GetField { dest, base, field } => {
                let ty = self.result_type(ctx, *dest)?;
                let base = self.value(ctx, *base)?;
                let op = ops::build(ctx, CoreOpKind::FieldGet, vec![ty], vec![base], vec![], 0);
                set(ctx, op, &KEY_FIELD, FieldAttr { name: field.into() });
                self.define(ctx, op, *dest, false);
            }
            MirInstr::SizeOf { dest, ty: measured } => {
                let ty = self.result_type(ctx, *dest)?;
                let measured = import_type(ctx, measured)?;
                let op = ops::build(ctx, CoreOpKind::SizeOf, vec![ty], vec![], vec![], 0);
                set(ctx, op, &KEY_SIZE_OF, SizeOfAttr { ty: measured });
                self.define(ctx, op, *dest, false);
            }
            MirInstr::WriteRef { reference, value } => {
                let operands = vec![self.value(ctx, *reference)?, self.value(ctx, *value)?];
                self.effectful(ctx, CoreOpKind::RefWrite, operands, |_, _| {});
            }
            MirInstr::MultiIndex {
                dest,
                object,
                args,
                object_place,
                arg_places,
                kwargs,
                kwarg_places,
                call,
            } => {
                let aligned = |places: &[Option<MirPlace>], count: usize| {
                    places.is_empty() || places.len() == count
                };
                if !aligned(arg_places, args.len()) || !aligned(kwarg_places, kwargs.len()) {
                    return Err(self.unsupported("a subscript whose places misalign its arguments"));
                }
                let mut operands = vec![self.value(ctx, *object)?];
                let mut shapes = Vec::with_capacity(args.len());
                for argument in args {
                    shapes.push(self.subscript_arg(ctx, argument, &mut operands)?);
                }
                let mut keywords = Vec::with_capacity(kwargs.len());
                for (name, argument) in kwargs {
                    keywords.push(CoreKeywordArg {
                        name: Text::from(name),
                        arg: self.subscript_arg(ctx, argument, &mut operands)?,
                    });
                }
                let call = call
                    .as_ref()
                    .map(|call| self.subscript_call(ctx, call, &mut operands))
                    .transpose()?;
                let places = object_place
                    .iter()
                    .chain(arg_places.iter().flatten())
                    .chain(kwarg_places.iter().flatten());
                for place in places {
                    operands.push(self.place(ctx, place)?);
                }
                let facts = MultiIndexAttr {
                    args: shapes,
                    kwargs: keywords,
                    object_place: object_place.is_some(),
                    arg_places: arg_places.iter().map(Option::is_some).collect(),
                    kwarg_places: kwarg_places.iter().map(Option::is_some).collect(),
                    call,
                };
                self.effectful_value(ctx, CoreOpKind::MultiIndex, operands, *dest, |ctx, op| {
                    set(ctx, op, &KEY_MULTI_INDEX, facts);
                })?;
            }
            MirInstr::Slice {
                dest,
                object,
                kind,
                lower,
                upper,
                step,
                object_place,
                arg_places,
                call,
                intrinsic,
            } => {
                let mut operands = vec![self.value(ctx, *object)?];
                for bound in [lower, upper, step].into_iter().flatten() {
                    operands.push(self.value(ctx, *bound)?);
                }
                let call = call
                    .as_ref()
                    .map(|call| self.subscript_call(ctx, call, &mut operands))
                    .transpose()?;
                for place in object_place.iter().chain(arg_places.iter().flatten()) {
                    operands.push(self.place(ctx, place)?);
                }
                let facts = SliceAttr {
                    bounds: CoreSliceBounds {
                        kind: CoreSliceKind::from_kind(*kind),
                        lower: lower.is_some(),
                        upper: upper.is_some(),
                        step: step.is_some(),
                    },
                    object_place: object_place.is_some(),
                    arg_places: arg_places.iter().map(Option::is_some).collect(),
                    call,
                    intrinsic: intrinsic.map(CoreIntrinsic::from_intrinsic),
                };
                self.effectful_value(ctx, CoreOpKind::Slice, operands, *dest, |ctx, op| {
                    set(ctx, op, &KEY_SLICE, facts);
                })?;
            }
            MirInstr::SimdShuffle {
                dest,
                value,
                other,
                mask,
            } => {
                let ty = self.result_type(ctx, *dest)?;
                let mut operands = vec![self.value(ctx, *value)?];
                if let Some(other) = other {
                    operands.push(self.value(ctx, *other)?);
                }
                let op = ops::build(ctx, CoreOpKind::SimdShuffle, vec![ty], operands, vec![], 0);
                set(
                    ctx,
                    op,
                    &KEY_SHUFFLE,
                    ShuffleAttr {
                        other: other.is_some(),
                        mask: mask.iter().map(|lane| *lane as u64).collect(),
                    },
                );
                self.define(ctx, op, *dest, false);
            }
            MirInstr::HasNext { dest, iter } => {
                let pack = self.var_slot(*iter)?;
                self.effectful_value(ctx, CoreOpKind::PackHasNext, vec![pack], *dest, |_, _| {})?;
            }
            MirInstr::Next { dest, iter } => {
                let pack = self.var_slot(*iter)?;
                self.effectful_value(ctx, CoreOpKind::PackNext, vec![pack], *dest, |_, _| {})?;
            }
            MirInstr::ConsumePlace { place, marker } => {
                let owner = place.root;
                let operands = vec![self.place(ctx, place)?, self.value(ctx, *marker)?];
                self.effectful(ctx, CoreOpKind::Consume, operands, |ctx, op| {
                    set(
                        ctx,
                        op,
                        &KEY_LIFECYCLE,
                        LifecycleAttr {
                            kind: CoreLifecycle::ConsumePlace,
                            owner,
                            path: Vec::new(),
                        },
                    );
                });
            }
            MirInstr::UninitStorage { dest, init } => {
                let mut operands = Vec::new();
                if let Some(init) = init {
                    operands.push(self.value(ctx, *init)?);
                }
                let facts = UninitAttr {
                    access: CoreUninitAccess::Make,
                    init: init.is_some(),
                    element: None,
                };
                self.effectful_value(
                    ctx,
                    CoreOpKind::UninitStorage,
                    operands,
                    *dest,
                    |ctx, op| {
                        set(ctx, op, &KEY_UNINIT, facts);
                    },
                )?;
            }
            MirInstr::UninitStorageTake {
                dest,
                storage,
                element,
            }
            | MirInstr::UninitStorageDestroy {
                dest,
                storage,
                element,
            } => {
                let access = match instruction {
                    MirInstr::UninitStorageTake { .. } => CoreUninitAccess::Take,
                    _ => CoreUninitAccess::Destroy,
                };
                let facts = UninitAttr {
                    access,
                    init: false,
                    element: Some(import_type(ctx, element)?),
                };
                let operands = vec![self.value(ctx, *storage)?];
                self.effectful_value(
                    ctx,
                    CoreOpKind::UninitStorage,
                    operands,
                    *dest,
                    |ctx, op| {
                        set(ctx, op, &KEY_UNINIT, facts);
                    },
                )?;
            }
            MirInstr::MakeVariant {
                dest,
                alternatives,
                index,
                value,
            } => {
                let declared = import_type(ctx, &Ty::Variant(alternatives.clone()))?;
                if declared != self.result_type(ctx, *dest)? {
                    return Err(self.unsupported(
                        "a variant construction at a type other than its register's",
                    ));
                }
                let operands = vec![self.value(ctx, *value)?];
                let access = CoreVariantAccess::Make(*index as u64);
                self.variant(ctx, CoreOpKind::VariantMake, operands, *dest, access)?;
            }
            MirInstr::VariantIs {
                dest,
                variant,
                index,
            } => {
                let ty = self.result_type(ctx, *dest)?;
                let operand = self.value(ctx, *variant)?;
                let op = ops::build(
                    ctx,
                    CoreOpKind::VariantTest,
                    vec![ty],
                    vec![operand],
                    vec![],
                    0,
                );
                set(
                    ctx,
                    op,
                    &KEY_VARIANT,
                    VariantAttr {
                        access: CoreVariantAccess::Test(*index as u64),
                    },
                );
                self.define(ctx, op, *dest, false);
            }
            MirInstr::VariantGet {
                dest,
                variant,
                index,
            } => {
                let operands = vec![self.value(ctx, *variant)?];
                let access = CoreVariantAccess::Get(*index as u64);
                self.variant(ctx, CoreOpKind::VariantGet, operands, *dest, access)?;
            }
            MirInstr::VariantTake {
                dest,
                variant,
                index,
                checked,
            } => {
                let operands = vec![self.value(ctx, *variant)?];
                let access = CoreVariantAccess::Take(*index as u64, *checked);
                self.variant(ctx, CoreOpKind::VariantGet, operands, *dest, access)?;
            }
            MirInstr::VariantSet {
                dest,
                place,
                index,
                value,
            } => {
                let operands = vec![self.place(ctx, place)?, self.value(ctx, *value)?];
                let access = CoreVariantAccess::Set(*index as u64);
                self.variant(ctx, CoreOpKind::VariantSet, operands, *dest, access)?;
            }
            MirInstr::VariantReplace {
                dest,
                place,
                input_index,
                output_index,
                value,
                checked,
            } => {
                let operands = vec![self.place(ctx, place)?, self.value(ctx, *value)?];
                let access =
                    CoreVariantAccess::Replace(*input_index as u64, *output_index as u64, *checked);
                self.variant(ctx, CoreOpKind::VariantSet, operands, *dest, access)?;
            }
            MirInstr::VariantSetInitWith {
                dest,
                place,
                index,
                factory,
            } => {
                let operands = vec![self.place(ctx, place)?, self.value(ctx, *factory)?];
                let access = CoreVariantAccess::SetInitWith(*index as u64);
                self.variant(ctx, CoreOpKind::VariantSet, operands, *dest, access)?;
            }
            MirInstr::VariantDeinitWith {
                dest,
                variant,
                handler,
                index,
            } => {
                let operands = vec![self.value(ctx, *variant)?, self.value(ctx, *handler)?];
                let access = CoreVariantAccess::DeinitWith(*index as u64);
                self.variant(ctx, CoreOpKind::VariantDeinitWith, operands, *dest, access)?;
            }
            MirInstr::DropPlace { place } => {
                let owner = place.root;
                let place = self.place(ctx, place)?;
                self.effectful(ctx, CoreOpKind::Drop, vec![place], |ctx, op| {
                    set(
                        ctx,
                        op,
                        &KEY_LIFECYCLE,
                        LifecycleAttr {
                            kind: CoreLifecycle::DropPlace,
                            owner,
                            path: Vec::new(),
                        },
                    );
                });
            }
            MirInstr::DropVar { var } => {
                self.lifecycle(ctx, CoreOpKind::Drop, CoreLifecycle::DropVar, *var)?;
            }
            MirInstr::ConsumeVar { var } => {
                self.lifecycle(ctx, CoreOpKind::Consume, CoreLifecycle::ConsumeVar, *var)?;
            }
            MirInstr::EstablishLoans {
                reference,
                loans,
                marker,
                dest_interior,
            } => {
                let mut operands = vec![self.var_slot(*reference)?];
                for loan in loans {
                    operands.push(self.place(ctx, &loan.place)?);
                }
                operands.push(self.token);
                let none = NoneType::get(ctx).into();
                let op = ops::build(
                    ctx,
                    CoreOpKind::Loans,
                    vec![none, self.effect],
                    operands,
                    vec![],
                    0,
                );
                set(
                    ctx,
                    op,
                    &KEY_LOANS,
                    LoansAttr {
                        loans: loans
                            .iter()
                            .map(|loan| CoreLoan {
                                mutable: loan.mutable,
                                shared: loan.shared,
                                interior: loan.interior.as_ref().map(CoreInterior::from_interior),
                            })
                            .collect(),
                        dest_interior: dest_interior.as_ref().map(CoreInterior::from_interior),
                    },
                );
                self.define_marker(ctx, op, *marker)?;
            }
            MirInstr::InvalidateInteriors {
                base,
                except,
                include_base_generation,
                marker,
            } => {
                let none = NoneType::get(ctx).into();
                let op = ops::build(
                    ctx,
                    CoreOpKind::Invalidate,
                    vec![none, self.effect],
                    vec![self.token],
                    vec![],
                    0,
                );
                set(
                    ctx,
                    op,
                    &KEY_INVALIDATION,
                    InvalidateAttr {
                        base: CoreInterior::from_interior(base),
                        except: *except,
                        include_base: *include_base_generation,
                    },
                );
                self.define_marker(ctx, op, *marker)?;
            }
            MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                cleanup,
            } => {
                let path = try_path(&self.region, *tries);
                *tries += 1;
                let mut operands = cleanup
                    .iter()
                    .map(|var| self.var_slot(*var))
                    .collect::<Result<Vec<_>, _>>()?;
                operands.push(self.token);
                let op = ops::build(
                    ctx,
                    CoreOpKind::TryBridge,
                    vec![self.effect],
                    operands,
                    vec![],
                    4,
                );
                set(
                    ctx,
                    op,
                    &KEY_TRY,
                    TryAttr {
                        handler: handler.is_some(),
                        error_var: handler.as_ref().and_then(|(var, _)| *var),
                        orelse: orelse.is_some(),
                        finalbody: finalbody.is_some(),
                    },
                );
                let parts = [
                    ("body", Some(body)),
                    ("handler", handler.as_ref().map(|(_, blocks)| blocks)),
                    ("else", orelse.as_ref()),
                    ("finally", finalbody.as_ref()),
                ];
                for (index, (part, blocks)) in parts.into_iter().enumerate() {
                    let Some(blocks) = blocks else { continue };
                    let region = op.deref(ctx).get_region(index);
                    let entry = BasicBlock::new(ctx, None, vec![self.effect]);
                    entry.insert_at_back(region, ctx);
                    self.region_blocks(ctx, region, &format!("{path}.{part}"), blocks, entry)?;
                }
                self.values.clear();
                let token = op.deref(ctx).get_result(0);
                self.finish(ctx, op, CoreRole::Primary, None, "no-source-record");
                self.token = token;
            }
            MirInstr::Raise { .. } => {
                return Err(self.unsupported("a `raise` outside the end of its block"));
            }
            MirInstr::ConstructTypeParam { .. }
            | MirInstr::MakeTuple { .. }
            | MirInstr::Drop { .. }
            | MirInstr::Unsupported(_) => {
                return Err(self.unsupported(format!(
                    "instruction `{}`",
                    instruction_mnemonic(instruction)
                )));
            }
        }
        Ok(())
    }

    fn admit_call(&self, instruction: &MirInstr) -> Result<(), A1Error> {
        let facets = refused_call_facets(instruction);
        if facets.is_empty() {
            return Ok(());
        }
        Err(self.unsupported(format!("a call with {}", facets.join(", "))))
    }

    fn call(&mut self, ctx: &mut Context, dest: Reg, facts: &CallFacts<'_>) -> Result<(), A1Error> {
        let aligned =
            |places: &[Option<MirPlace>], count: usize| places.is_empty() || places.len() == count;
        if !aligned(facts.arg_places, facts.args.len())
            || !aligned(facts.kwarg_places, facts.kwargs.len())
        {
            return Err(self.unsupported("a call whose retained places misalign its arguments"));
        }
        let ty = self.result_type(ctx, dest)?;
        let raises = facts.raises.map(|ty| import_type(ctx, ty)).transpose()?;
        let mut operands = Vec::new();
        if let Some(recv) = facts.recv {
            operands.push(self.value(ctx, recv)?);
        }
        for reg in facts.args {
            operands.push(self.value(ctx, *reg)?);
        }
        for (_, reg) in facts.kwargs {
            operands.push(self.value(ctx, *reg)?);
        }
        let params = self.param_args(ctx, facts.params, &mut operands)?;
        for place in facts.arg_places.iter().chain(facts.kwarg_places).flatten() {
            operands.push(self.place(ctx, place)?);
        }
        if let Some(place) = facts.recv_place {
            operands.push(self.place(ctx, place)?);
        }
        let contract = facts.contract.map(|ty| import_type(ctx, ty)).transpose()?;
        let instantiated = facts
            .instantiated
            .iter()
            .map(|argument| import_arg(ctx, argument))
            .collect::<Result<Vec<_>, _>>()?;
        let reference_result = facts
            .reference_result
            .map(|reference| import_type(ctx, &Ty::Ref(reference.clone())))
            .transpose()?;
        operands.push(self.token);
        let op = ops::build(
            ctx,
            CoreOpKind::Call,
            vec![ty, self.effect],
            operands,
            vec![],
            0,
        );
        set(
            ctx,
            op,
            &KEY_CALL,
            CallAttr {
                kind: facts.kind,
                target: facts.target.clone().into(),
                resolved: facts.resolved.clone().map(Text::from),
                raises,
                args: facts.args.len() as u64,
                kwargs: facts
                    .kwargs
                    .iter()
                    .map(|(name, _)| Text::from(name))
                    .collect(),
                arg_places: facts.arg_places.iter().map(Option::is_some).collect(),
                kwarg_places: facts.kwarg_places.iter().map(Option::is_some).collect(),
                recv_place: facts.recv_place.is_some(),
                recv_writes: facts.recv_writes,
                params,
                captures: facts
                    .captures
                    .iter()
                    .map(CoreCaptureAccess::from_access)
                    .collect(),
                contract,
                instantiated,
                reference_result,
                adapter: facts.adapter.map(|adapter| match adapter {
                    CheckedResultAdapter::CopyIteratorReference => {
                        CoreResultAdapter::CopyIteratorReference
                    }
                }),
            },
        );
        self.define(ctx, op, dest, true);
        Ok(())
    }

    /// The shape of one subscript argument, with its value operands
    /// appended to `operands`.
    fn subscript_arg(
        &mut self,
        ctx: &mut Context,
        argument: &MirSubscriptArg,
        operands: &mut Vec<Value>,
    ) -> Result<CoreSubscriptArg, A1Error> {
        Ok(match argument {
            MirSubscriptArg::Index(reg) => {
                operands.push(self.value(ctx, *reg)?);
                CoreSubscriptArg::Index
            }
            MirSubscriptArg::Slice {
                kind,
                lower,
                upper,
                step,
            } => {
                for bound in [lower, upper, step].into_iter().flatten() {
                    operands.push(self.value(ctx, *bound)?);
                }
                CoreSubscriptArg::Slice(CoreSliceBounds {
                    kind: CoreSliceKind::from_kind(*kind),
                    lower: lower.is_some(),
                    upper: upper.is_some(),
                    step: step.is_some(),
                })
            }
        })
    }

    /// The facts of a subscript's nominal call, with its reified
    /// compile-time arguments appended to `operands`.
    fn subscript_call(
        &mut self,
        ctx: &mut Context,
        call: &MirSubscriptCall,
        operands: &mut Vec<Value>,
    ) -> Result<CoreSubscriptCall, A1Error> {
        let facets = refused_subscript_facets(call);
        if !facets.is_empty() {
            return Err(self.unsupported(format!("a subscript call with {}", facets.join(", "))));
        }
        let params = self.param_args(ctx, &call.param_arg_regs, operands)?;
        let arguments = call
            .arguments
            .iter()
            .map(|argument| {
                Ok(CoreCallArgument {
                    source: match argument.source {
                        CheckedCallArgumentSource::Positional(index) => {
                            CoreArgSource::Positional(index as u64)
                        }
                        CheckedCallArgumentSource::Keyword(index) => {
                            CoreArgSource::Keyword(index as u64)
                        }
                        CheckedCallArgumentSource::Default => CoreArgSource::Default,
                    },
                    parameter: import_type(ctx, &argument.parameter_ty)?,
                    requires_place: argument.requires_place,
                    convention: argument.convention.map(CoreConvention::from_convention),
                })
            })
            .collect::<Result<Vec<_>, A1Error>>()?;
        Ok(CoreSubscriptCall {
            target: Text::from(&call.target),
            raises: call
                .raises
                .as_ref()
                .map(|ty| import_type(ctx, ty))
                .transpose()?,
            result: import_type(ctx, &call.result_ty)?,
            receiver_requires_place: call.receiver_requires_place,
            receiver_convention: call
                .receiver_convention
                .map(CoreConvention::from_convention),
            arguments,
            reference_result: call
                .reference_result
                .as_ref()
                .map(|reference| import_type(ctx, &mojito_types::types::Ty::Ref(reference.clone())))
                .transpose()?,
            params,
            captures: call
                .capture_accesses
                .iter()
                .map(CoreCaptureAccess::from_access)
                .collect(),
        })
    }

    /// The compile-time arguments of a call, with the registers reifying
    /// them appended to `operands`.
    fn param_args(
        &mut self,
        ctx: &mut Context,
        arguments: &[MirParamArg],
        operands: &mut Vec<Value>,
    ) -> Result<Vec<CoreParamArg>, A1Error> {
        let mut params = Vec::new();
        for param in arguments {
            if let Some(reg) = param.value {
                operands.push(self.value(ctx, reg)?);
            }
            params.push(CoreParamArg {
                name: param.name.as_ref().map(Text::from),
                value: param.value.is_some(),
                binder: param.binder.as_ref().map(PayloadBinder::from_ref),
                expr: param
                    .expr
                    .as_ref()
                    .map(|expr| import_param(ctx, expr))
                    .transpose()?,
            });
        }
        Ok(params)
    }

    /// Append an effectful variant operation under `access`.
    fn variant(
        &mut self,
        ctx: &mut Context,
        kind: CoreOpKind,
        operands: Vec<Value>,
        dest: Reg,
        access: CoreVariantAccess,
    ) -> Result<(), A1Error> {
        self.effectful_value(ctx, kind, operands, dest, |ctx, op| {
            set(ctx, op, &KEY_VARIANT, VariantAttr { access });
        })
    }

    /// Append an effectful operation whose value result defines `dest`.
    fn effectful_value(
        &mut self,
        ctx: &mut Context,
        kind: CoreOpKind,
        mut operands: Vec<Value>,
        dest: Reg,
        attributes: impl FnOnce(&Context, Ptr<Operation>),
    ) -> Result<(), A1Error> {
        let ty = self.result_type(ctx, dest)?;
        operands.push(self.token);
        let op = ops::build(ctx, kind, vec![ty, self.effect], operands, vec![], 0);
        attributes(ctx, op);
        self.define(ctx, op, dest, true);
        Ok(())
    }

    fn lifecycle(
        &mut self,
        ctx: &mut Context,
        kind: CoreOpKind,
        event: CoreLifecycle,
        var: u32,
    ) -> Result<(), A1Error> {
        let place = self.var_slot(var)?;
        self.effectful(ctx, kind, vec![place], |ctx, op| {
            set(
                ctx,
                op,
                &KEY_LIFECYCLE,
                LifecycleAttr {
                    kind: event,
                    owner: var,
                    path: Vec::new(),
                },
            );
        });
        Ok(())
    }

    /// Append an effectful operation without a value result.
    fn effectful(
        &mut self,
        ctx: &mut Context,
        kind: CoreOpKind,
        mut operands: Vec<Value>,
        attributes: impl FnOnce(&Context, Ptr<Operation>),
    ) {
        operands.push(self.token);
        let op = ops::build(ctx, kind, vec![self.effect], operands, vec![], 0);
        attributes(ctx, op);
        let token = op.deref(ctx).get_result(0);
        self.finish(ctx, op, CoreRole::Primary, None, "no-source-record");
        self.token = token;
    }

    /// Append a value-producing operation and bind its result to `dest`.
    fn define(&mut self, ctx: &mut Context, op: Ptr<Operation>, dest: Reg, effectful: bool) {
        set(ctx, op, &KEY_REG, RegAttr(dest.0));
        let value = op.deref(ctx).get_result(0);
        if effectful {
            let token = op.deref(ctx).get_result(1);
            self.finish(ctx, op, CoreRole::Primary, Some(dest), "source");
            self.token = token;
        } else {
            self.finish(ctx, op, CoreRole::Primary, Some(dest), "source");
        }
        self.bind(ctx, dest, value);
    }

    fn define_marker(
        &mut self,
        ctx: &mut Context,
        op: Ptr<Operation>,
        marker: Reg,
    ) -> Result<(), A1Error> {
        if !matches!(self.reg_type(marker)?, mojito_types::types::Ty::None) {
            return Err(self.unsupported("a marker register that is not `None`"));
        }
        self.define(ctx, op, marker, true);
        Ok(())
    }

    /// Record `value` as `reg`: directly, or through its register slot.
    fn bind(&mut self, ctx: &mut Context, reg: Reg, value: Value) {
        self.bind_as(ctx, reg, value, CoreRole::RegisterStore);
    }

    /// [`Self::bind`], with the transport store carrying `role`.
    fn bind_as(&mut self, ctx: &mut Context, reg: Reg, value: Value, role: CoreRole) {
        let Some(slot) = self.reg_slots.get(&reg.0).copied() else {
            self.values.insert(reg.0, value);
            return;
        };
        let op = ops::build(
            ctx,
            CoreOpKind::Store,
            vec![self.effect],
            vec![slot, value, self.token],
            vec![],
            0,
        );
        set(ctx, op, &KEY_STORE, StoreAttr::Register);
        let token = op.deref(ctx).get_result(0);
        self.finish(ctx, op, role, None, "register-transport");
        self.token = token;
    }

    /// The value of `reg` at this point: its SSA value, or a load of its
    /// register slot.
    fn value(&mut self, ctx: &mut Context, reg: Reg) -> Result<Value, A1Error> {
        if let Some(slot) = self.reg_slots.get(&reg.0).copied() {
            let ty = self.result_type(ctx, reg)?;
            let op = ops::build(
                ctx,
                CoreOpKind::Load,
                vec![ty, self.effect],
                vec![slot, self.token],
                vec![],
                0,
            );
            set(ctx, op, &KEY_REG, RegAttr(reg.0));
            let value = op.deref(ctx).get_result(0);
            let token = op.deref(ctx).get_result(1);
            let role = CoreRole::RegisterLoad(self.loads);
            self.loads += 1;
            self.finish(ctx, op, role, None, "register-transport");
            self.token = token;
            return Ok(value);
        }
        self.values
            .get(&reg.0)
            .copied()
            .ok_or_else(|| self.unsupported(format!("a read of undefined register %r{}", reg.0)))
    }

    fn place(&mut self, ctx: &mut Context, place: &MirPlace) -> Result<Value, A1Error> {
        if place.proj.len() != place.projection_tys.len() {
            return Err(self.unsupported("a place whose projections are not all typed"));
        }
        let root = self.var_slot(place.root)?;
        let mut operands = vec![root];
        let mut steps = Vec::new();
        for (projection, ty) in place.proj.iter().zip(&place.projection_tys) {
            let kind = match projection {
                Proj::Field(name) => CoreStepKind::Field(name.into()),
                Proj::Index(reg) => {
                    operands.push(self.value(ctx, *reg)?);
                    CoreStepKind::Index
                }
                Proj::ConstIndex(index) => CoreStepKind::ConstIndex(*index as u64),
                Proj::Variant(index) => CoreStepKind::Variant(*index as u64),
                Proj::UninitPayload => CoreStepKind::UninitPayload,
            };
            steps.push(CoreStep {
                kind,
                ty: import_type(ctx, ty)?,
            });
        }
        let root_ty = place
            .root_ty
            .as_ref()
            .map(|ty| import_type(ctx, ty))
            .transpose()?;
        let Some(target) = place
            .ty
            .as_ref()
            .map(|ty| import_type(ctx, ty))
            .transpose()?
        else {
            return Err(self.unsupported("an untyped place"));
        };
        let result = PlaceType::get(ctx, target).into();
        let op = ops::build(ctx, CoreOpKind::Project, vec![result], operands, vec![], 0);
        set(
            ctx,
            op,
            &KEY_PROJECTION,
            ProjectionAttr {
                root_ty,
                steps,
                ty: Some(target),
                through: place.through,
            },
        );
        let value = op.deref(ctx).get_result(0);
        let role = CoreRole::Place(self.places);
        self.places += 1;
        self.finish(ctx, op, role, None, "place");
        Ok(value)
    }

    /// Annotate `op` with this position's identity and link it at the end
    /// of the current block.
    fn finish(
        &self,
        ctx: &Context,
        op: Ptr<Operation>,
        role: CoreRole,
        source: Option<Reg>,
        reason: &str,
    ) {
        let identity = IdentityAttr {
            function: self.name.into(),
            region: self.region.as_str().into(),
            block: self.mir_block as u64,
            ordinal: self.ordinal as u64,
            role,
        };
        let record = source.and_then(|reg| self.records.get(&reg.0));
        let derived_from = if role == CoreRole::Primary {
            None
        } else {
            let primary = IdentityAttr {
                role: CoreRole::Primary,
                ..identity.clone()
            };
            Some(primary.local_key().into())
        };
        let provenance = ProvenanceAttr {
            span: record.map(|(span, _)| span.clone()),
            origin: record.and_then(|(_, origin)| *origin),
            derived_from,
            reason: reason.into(),
        };
        annotate(ctx, op, identity, provenance);
        op.insert_at_back(self.block, ctx);
        if self.sink.collect
            && let Some(kind) = CoreOpKind::of(ctx, op)
            && let Err(error) = super::verify::verify_core_op(ctx, op, kind)
        {
            let text = pliron::printable::Printable::disp(&error, ctx).to_string();
            self.sink.record(format!("verify: {}", verify_rule(&text)));
        }
    }

    /// The registers the tables name and no operation defines.
    fn orphans(&self, ctx: &mut Context) -> Result<Vec<CoreOrphan>, A1Error> {
        let mut sites = RegisterSites::default();
        sites.walk("", &self.function.blocks);
        let named: BTreeSet<u32> = self
            .function
            .reg_types
            .keys()
            .chain(self.function.spans.0.keys())
            .copied()
            .collect();
        named
            .into_iter()
            .filter(|reg| !sites.defs.contains_key(reg))
            .map(|reg| {
                let record = self.records.get(&reg);
                Ok(CoreOrphan {
                    reg,
                    ty: self
                        .function
                        .reg_types
                        .get(&reg)
                        .map(|ty| import_type(ctx, ty))
                        .transpose()?,
                    provenance: CoreOrphanSpan {
                        span: record.map(|(span, _)| span.clone()),
                        origin: record.and_then(|(_, origin)| *origin),
                    },
                })
            })
            .collect()
    }

    fn var_slot(&self, var: u32) -> Result<Value, A1Error> {
        self.var_slots
            .get(var as usize)
            .copied()
            .ok_or_else(|| self.unsupported(format!("a use of undeclared variable slot $v{var}")))
    }

    fn reg_type(&self, reg: Reg) -> Result<&'a mojito_types::types::Ty, A1Error> {
        self.function
            .reg_types
            .get(&reg.0)
            .ok_or_else(|| self.unsupported(format!("untyped register %r{}", reg.0)))
    }

    fn result_type(&self, ctx: &mut Context, reg: Reg) -> Result<TypeHandle, A1Error> {
        import_type(ctx, self.reg_type(reg)?)
    }

    fn unsupported(&self, what: impl std::fmt::Display) -> A1Error {
        A1Error::new(
            A1ErrorKind::UnsupportedForm,
            format!(
                "{what} has no core form (region `{}`, bb{}, instruction {})",
                self.region, self.mir_block, self.ordinal
            ),
        )
        .in_function(self.name)
    }
}

/// The checked facts of a direct or method call, by reference.
struct CallFacts<'a> {
    kind: CoreCallKind,
    target: String,
    resolved: Option<String>,
    raises: Option<&'a mojito_types::types::Ty>,
    recv: Option<Reg>,
    recv_place: Option<&'a MirPlace>,
    recv_writes: bool,
    args: &'a [Reg],
    kwargs: &'a [(String, Reg)],
    arg_places: &'a [Option<MirPlace>],
    kwarg_places: &'a [Option<MirPlace>],
    params: &'a [MirParamArg],
    captures: &'a [MirCaptureAccess],
    contract: Option<&'a Ty>,
    instantiated: &'a [TyArg],
    reference_result: Option<&'a mojito_types::origin::RefTy>,
    adapter: Option<CheckedResultAdapter>,
}

/// The facets of a call instruction the core call attribute cannot carry.
fn refused_call_facets(instruction: &MirInstr) -> Vec<&'static str> {
    let parameters = match instruction {
        MirInstr::MethodCall { param_decls, .. } | MirInstr::CallIndirect { param_decls, .. } => {
            !param_decls.is_empty()
        }
        _ => false,
    };
    parameters
        .then_some("unresolved parameters")
        .into_iter()
        .collect()
}

/// The facets of a subscript's nominal call the core attribute cannot
/// carry.
fn refused_subscript_facets(call: &MirSubscriptCall) -> Vec<&'static str> {
    (!call.param_decls.is_empty())
        .then_some("unresolved parameters")
        .into_iter()
        .collect()
}

/// The census keys of one refusal: its class and what it names, without
/// the position and the payload that would keep equal refusals apart. A
/// call refused for several facets counts once per facet.
fn refusal_keys(message: &str) -> Vec<String> {
    for class in ["legality", "verify", "struct", "declaration"] {
        if let Some(body) = message
            .strip_prefix(class)
            .and_then(|rest| rest.strip_prefix(": "))
        {
            let body = body.split(" (`").next().unwrap_or(body);
            return vec![format!("{class}: {}", normalized(body))];
        }
    }
    let message = message.split(" (region `").next().unwrap_or(message);
    let message = message.strip_suffix(" has no core form").unwrap_or(message);
    if let Some(facets) = message.strip_prefix("a call with ") {
        return facets
            .split(", ")
            .map(|facet| format!("call: {facet}"))
            .collect();
    }
    if let Some(facets) = message.strip_prefix("a subscript call with ") {
        return facets
            .split(", ")
            .map(|facet| format!("subscript: {facet}"))
            .collect();
    }
    if let Some(form) = message
        .strip_prefix("instruction `")
        .or_else(|| message.strip_prefix("terminator `"))
    {
        return vec![format!("form: {}", form.trim_end_matches('`'))];
    }
    if let Some(what) = message.strip_suffix(" is outside the core type vocabulary") {
        return vec![format!("type: {}", normalized(what).trim_matches('`'))];
    }
    if message.starts_with("constant `") {
        return vec![format!("constant: {}", normalized(message))];
    }
    if message.contains("is outside the parameter payload") {
        return vec![format!("param: {}", normalized(message))];
    }
    vec![format!("shape: {}", normalized(message))]
}

/// `message` with every backticked payload cut to its head: the type or
/// constant constructor, or the whole symbol, so that equal refusals count
/// together.
fn normalized(message: &str) -> String {
    let mut out = String::new();
    let mut rest = message;
    while let Some((before, after)) = rest.split_once('`') {
        out.push_str(before);
        out.push('`');
        let Some((payload, tail)) = after.split_once('`') else {
            out.push_str(after);
            return out;
        };
        let head = payload.find([' ', '(', '{']).unwrap_or(payload.len());
        out.push_str(&payload[..head]);
        out.push('`');
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// The rule a verifier diagnostic states, without the framing and the
/// operation it names.
fn verify_rule(text: &str) -> &str {
    let rule = text.rsplit("verification failed. ").next().unwrap_or(text);
    rule.split(" (`").next().unwrap_or(rule)
}

/// Every source label the program's records name, sorted.
fn sources(program: &MirProgram) -> Vec<String> {
    program
        .functions
        .iter()
        .flat_map(|(_, function)| function.spans.0.values())
        .filter_map(|(span, _)| span.source.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn import_struct(ctx: &mut Context, decl: &MirStructDeclaration) -> Result<CoreStruct, A1Error> {
    if !decl.param_decls.is_empty() {
        return Err(A1Error::new(
            A1ErrorKind::UnsupportedForm,
            format!("struct `{}` keeps unresolved parameters", decl.name),
        ));
    }
    let fields = decl
        .fields
        .iter()
        .map(|(name, ty)| {
            Ok(CoreField {
                name: name.into(),
                ty: import_type(ctx, ty)?,
            })
        })
        .collect::<Result<Vec<_>, A1Error>>()?;
    let mut mut_self_methods: Vec<String> = decl.mut_self_methods.iter().cloned().collect();
    mut_self_methods.sort();
    let mut explicit_destructors: Vec<CoreDestructor> = decl
        .explicit_destructors
        .iter()
        .map(|(name, raises)| CoreDestructor {
            name: name.into(),
            raises: *raises,
        })
        .collect();
    explicit_destructors.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(CoreStruct {
        name: Text::from(&decl.name),
        fields,
        mut_self_methods: mut_self_methods.into_iter().map(Text::from).collect(),
        fieldwise_init: decl.fieldwise_init,
        explicit_destroy_message: decl.explicit_destroy_message.clone().map(Text::from),
        explicit_destructors,
    })
}

fn import_declaration(
    ctx: &mut Context,
    decl: &MirFunctionDeclaration,
) -> Result<DeclarationAttr, A1Error> {
    let count = decl.param_types.len();
    let aligned = [
        decl.param_names.len(),
        decl.defaults.len(),
        decl.required.len(),
        decl.param_conventions.len(),
        decl.ref_params.len(),
        decl.param_writes.len(),
    ]
    .iter()
    .all(|length| *length == count);
    if !aligned {
        return Err(A1Error::new(
            A1ErrorKind::UnsupportedForm,
            "a declaration whose parameter tables misalign has no core form",
        ));
    }
    if !decl.param_decls.is_empty() {
        return Err(A1Error::new(
            A1ErrorKind::UnsupportedForm,
            "a declaration with unresolved parameters has no core form",
        ));
    }
    let mut collector = |ty: &Option<mojito_types::types::Ty>,
                         convention: Option<mojito_ast::ast::ArgConvention>,
                         index: Option<usize>| {
        let collector = CoreCollector {
            ty: ty.as_ref().map(|ty| import_type(ctx, ty)).transpose()?,
            convention: convention.map(CoreConvention::from_convention),
            index: index.map(|index| index as u64),
        };
        Ok::<_, A1Error>((!collector.is_absent()).then_some(collector))
    };
    let variadic = collector(
        &decl.variadic,
        decl.variadic_convention,
        decl.variadic_index,
    )?;
    let kw_variadic = collector(
        &decl.kw_variadic,
        decl.kw_variadic_convention,
        decl.kw_variadic_index,
    )?;
    let params = (0..count)
        .map(|index| {
            Ok(CoreParam {
                name: Text::from(&decl.param_names[index]),
                ty: import_type(ctx, &decl.param_types[index])?,
                required: decl.required[index],
                convention: decl.param_conventions[index].map(CoreConvention::from_convention),
                by_ref: decl.ref_params[index],
                writes: decl.param_writes[index],
                default: decl.defaults[index]
                    .as_ref()
                    .map(CoreDefault::from_const)
                    .transpose()?,
            })
        })
        .collect::<Result<Vec<_>, A1Error>>()?;
    Ok(DeclarationAttr {
        symbol: Text::from(&decl.lowered_name),
        params,
        has_receiver: decl.has_receiver,
        receiver: decl
            .receiver_convention
            .map(CoreConvention::from_convention),
        ret: import_type(ctx, &decl.ret_ty)?,
        returns_reference: decl.returns_reference,
        raises: decl.raises,
        error: decl
            .error_ty
            .as_ref()
            .map(|ty| import_type(ctx, ty))
            .transpose()?,
        variadic,
        kw_variadic,
        positional_only: decl.positional_only.map(|count| count as u64),
        keyword_only: decl.keyword_only.map(|index| index as u64),
    })
}
