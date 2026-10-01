//! The closed inventory of `mojito`: the registry every verifier,
//! importer, exporter, and legality check decides from, and the census of
//! MIR forms that fixed it.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use pliron::context::{Context, Ptr};
use pliron::op::{Op, OpObj};
use pliron::operation::Operation;

use mojito_mir::mir::text::{instruction_mnemonic, terminator_mnemonic, type_spelling};
use mojito_mir::mir::verify::instruction_result_regs;
use mojito_mir::mir::{MirBlock, MirFunction, MirInstr, MirProgram};

use super::ops;

/// A conversion boundary, and what may legally remain at it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    /// Freshly imported: structured `try` is permitted.
    Bridge,
    /// Normalized: no bridge operation remains.
    ExecutableCore,
}

/// How an operation relates to the effect chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffectClass {
    /// No effect token; removable when unused and its opcode is total.
    Pure,
    /// Consumes the current token and produces the next.
    Effectful,
    /// Ends a block, consuming the current token.
    Terminator,
    /// A container or a declaration outside any chain.
    Structural,
}

/// Every operation `mojito` registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CoreOpKind {
    Func,
    Slot,
    Const,
    Materialize,
    Binary,
    SimdMake,
    Unary,
    SimdConvert,
    Use,
    Store,
    Project,
    Load,
    RefMake,
    RefStore,
    RefRead,
    Copy,
    Move,
    KeepAlive,
    Index,
    MultiSet,
    PointerStorage,
    IterInit,
    IterNext,
    Call,
    ClosureMake,
    FieldGet,
    SizeOf,
    RefWrite,
    MultiIndex,
    Slice,
    SimdShuffle,
    PackHasNext,
    PackNext,
    UninitStorage,
    VariantMake,
    VariantTest,
    VariantGet,
    VariantSet,
    VariantDeinitWith,
    Invoke,
    Drop,
    Consume,
    Loans,
    Invalidate,
    Br,
    CondBr,
    Return,
    Raise,
    Outcome,
    Resume,
    TryBridge,
    RegionExit,
    Escape,
}

impl CoreOpKind {
    pub const ALL: [Self; 53] = [
        Self::Func,
        Self::Slot,
        Self::Const,
        Self::Materialize,
        Self::Binary,
        Self::SimdMake,
        Self::Unary,
        Self::SimdConvert,
        Self::Use,
        Self::Store,
        Self::Project,
        Self::Load,
        Self::RefMake,
        Self::RefStore,
        Self::RefRead,
        Self::Copy,
        Self::Move,
        Self::KeepAlive,
        Self::Index,
        Self::MultiSet,
        Self::PointerStorage,
        Self::IterInit,
        Self::IterNext,
        Self::Call,
        Self::ClosureMake,
        Self::FieldGet,
        Self::SizeOf,
        Self::RefWrite,
        Self::MultiIndex,
        Self::Slice,
        Self::SimdShuffle,
        Self::PackHasNext,
        Self::PackNext,
        Self::UninitStorage,
        Self::VariantMake,
        Self::VariantTest,
        Self::VariantGet,
        Self::VariantSet,
        Self::VariantDeinitWith,
        Self::Invoke,
        Self::Drop,
        Self::Consume,
        Self::Loans,
        Self::Invalidate,
        Self::Br,
        Self::CondBr,
        Self::Return,
        Self::Raise,
        Self::Outcome,
        Self::Resume,
        Self::TryBridge,
        Self::RegionExit,
        Self::Escape,
    ];

    /// The operation's name within the `mojito` dialect.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Func => "func",
            Self::Slot => "slot",
            Self::Const => "const",
            Self::Materialize => "materialize",
            Self::Binary => "binary",
            Self::SimdMake => "simd_make",
            Self::Unary => "unary",
            Self::SimdConvert => "simd_convert",
            Self::Use => "use",
            Self::Store => "store",
            Self::Project => "project",
            Self::Load => "load",
            Self::RefMake => "ref_make",
            Self::RefStore => "ref_store",
            Self::RefRead => "ref_read",
            Self::Copy => "copy",
            Self::Move => "move",
            Self::KeepAlive => "keep_alive",
            Self::Index => "index",
            Self::MultiSet => "multi_set",
            Self::PointerStorage => "pointer_storage",
            Self::IterInit => "iter_init",
            Self::IterNext => "iter_next",
            Self::Call => "call",
            Self::ClosureMake => "closure_make",
            Self::FieldGet => "field_get",
            Self::SizeOf => "size_of",
            Self::RefWrite => "ref_write",
            Self::MultiIndex => "multi_index",
            Self::Slice => "slice",
            Self::SimdShuffle => "simd_shuffle",
            Self::PackHasNext => "pack_has_next",
            Self::PackNext => "pack_next",
            Self::UninitStorage => "uninit_storage",
            Self::VariantMake => "variant_make",
            Self::VariantTest => "variant_test",
            Self::VariantGet => "variant_get",
            Self::VariantSet => "variant_set",
            Self::VariantDeinitWith => "variant_deinit_with",
            Self::Invoke => "invoke",
            Self::Drop => "drop",
            Self::Consume => "consume",
            Self::Loans => "loans",
            Self::Invalidate => "invalidate",
            Self::Br => "br",
            Self::CondBr => "cond_br",
            Self::Return => "return",
            Self::Raise => "raise",
            Self::Outcome => "outcome",
            Self::Resume => "resume",
            Self::TryBridge => "try_bridge",
            Self::RegionExit => "region_exit",
            Self::Escape => "escape",
        }
    }

    pub const fn effect(self) -> EffectClass {
        match self {
            Self::Func | Self::Slot | Self::Project => EffectClass::Structural,
            Self::Const
            | Self::Materialize
            | Self::Binary
            | Self::SimdMake
            | Self::Unary
            | Self::SimdConvert
            | Self::RefMake
            | Self::FieldGet
            | Self::SizeOf
            | Self::SimdShuffle
            | Self::VariantTest => EffectClass::Pure,
            Self::Use
            | Self::RefRead
            | Self::Copy
            | Self::Move
            | Self::KeepAlive
            | Self::Index
            | Self::MultiSet
            | Self::PointerStorage
            | Self::IterInit
            | Self::IterNext
            | Self::Store
            | Self::Load
            | Self::RefStore
            | Self::Call
            | Self::ClosureMake
            | Self::RefWrite
            | Self::MultiIndex
            | Self::Slice
            | Self::PackHasNext
            | Self::PackNext
            | Self::UninitStorage
            | Self::VariantMake
            | Self::VariantGet
            | Self::VariantSet
            | Self::VariantDeinitWith
            | Self::Drop
            | Self::Consume
            | Self::Loans
            | Self::Invalidate
            | Self::Outcome
            | Self::TryBridge => EffectClass::Effectful,
            Self::Invoke
            | Self::Br
            | Self::CondBr
            | Self::Return
            | Self::Raise
            | Self::Resume
            | Self::RegionExit
            | Self::Escape => EffectClass::Terminator,
        }
    }

    /// Whether the operation may remain at `stage`.
    pub const fn legal_at(self, stage: Stage) -> bool {
        match self {
            Self::TryBridge | Self::RegionExit | Self::Escape => matches!(stage, Stage::Bridge),
            Self::Invoke | Self::Outcome | Self::Resume => matches!(stage, Stage::ExecutableCore),
            _ => true,
        }
    }

    /// The MIR forms this operation imports, by their v1 mnemonics. An
    /// operation with none is synthesized by the bridge or by normalization.
    pub const fn mir_forms(self) -> &'static [&'static str] {
        match self {
            Self::Const => &["const"],
            Self::Materialize => &["literal.materialize"],
            Self::Binary => &["binary"],
            Self::SimdMake => &["simd.make"],
            Self::Unary => &["unary"],
            Self::SimdConvert => &["simd.cast", "simd.bits"],
            Self::RefRead => &["ref.read"],
            Self::Copy => &["value.copy"],
            Self::Move => &["place.move"],
            Self::KeepAlive => &["lifetime.keep_alive"],
            Self::Index => &["index.get"],
            Self::MultiSet => &["index.multi_set"],
            Self::PointerStorage => &["pointer.take", "pointer.destroy"],
            Self::IterInit => &["iter.init"],
            Self::IterNext => &["iter.try_next"],
            Self::Use => &["var.use"],
            Self::Store => &["var.store", "place.store"],
            Self::Load => &["place.load"],
            Self::RefMake => &["ref.make"],
            Self::RefStore => &["place.store_ref"],
            Self::Call => &["call", "call.method", "call.indirect"],
            Self::ClosureMake => &["closure.make"],
            Self::FieldGet => &["field.get"],
            Self::SizeOf => &["layout.size_of"],
            Self::RefWrite => &["ref.write"],
            Self::MultiIndex => &["index.multi"],
            Self::Slice => &["slice.get"],
            Self::SimdShuffle => &["simd.shuffle"],
            Self::PackHasNext => &["iter.has_next"],
            Self::PackNext => &["iter.next"],
            Self::UninitStorage => &["uninit.make", "uninit.take", "uninit.destroy"],
            Self::VariantMake => &["variant.make"],
            Self::VariantTest => &["variant.is"],
            Self::VariantGet => &["variant.get", "variant.take"],
            Self::VariantSet => &["variant.set", "variant.replace", "variant.set_init_with"],
            Self::VariantDeinitWith => &["variant.deinit_with"],
            Self::Drop => &["drop.var", "drop.place"],
            Self::Consume => &["consume.var", "consume.place"],
            Self::Loans => &["loans.establish"],
            Self::Invalidate => &["interiors.invalidate"],
            Self::Br => &["jump"],
            Self::CondBr => &["branch"],
            Self::Return => &["return", "return.cleanup"],
            Self::Raise => &["raise"],
            Self::TryBridge => &["try"],
            Self::RegionExit => &["falloff"],
            Self::Escape => &["escape"],
            Self::Func
            | Self::Slot
            | Self::Project
            | Self::Invoke
            | Self::Outcome
            | Self::Resume => &[],
        }
    }

    /// What the exporter does with the operation at the bridge stage.
    pub const fn export_rule(self) -> ExportRule {
        match self {
            Self::Func | Self::Slot => ExportRule::Frame,
            Self::Project => ExportRule::Place,
            Self::Const
            | Self::Materialize
            | Self::Binary
            | Self::SimdMake
            | Self::Unary
            | Self::SimdConvert
            | Self::Use
            | Self::Store
            | Self::Load
            | Self::RefRead
            | Self::Copy
            | Self::Move
            | Self::KeepAlive
            | Self::Index
            | Self::MultiSet
            | Self::PointerStorage
            | Self::IterInit
            | Self::IterNext
            | Self::RefMake
            | Self::RefStore
            | Self::Call
            | Self::ClosureMake
            | Self::FieldGet
            | Self::SizeOf
            | Self::RefWrite
            | Self::MultiIndex
            | Self::Slice
            | Self::SimdShuffle
            | Self::PackHasNext
            | Self::PackNext
            | Self::UninitStorage
            | Self::VariantMake
            | Self::VariantTest
            | Self::VariantGet
            | Self::VariantSet
            | Self::VariantDeinitWith
            | Self::Drop
            | Self::Consume
            | Self::Loans
            | Self::Invalidate
            | Self::TryBridge => ExportRule::Instruction,
            Self::Br
            | Self::CondBr
            | Self::Return
            | Self::Raise
            | Self::RegionExit
            | Self::Escape => ExportRule::Terminator,
            Self::Invoke | Self::Outcome | Self::Resume => ExportRule::Denormalized,
        }
    }

    /// How the operation is spelled in core text.
    pub const fn text_rule(self) -> TextRule {
        TextRule::CanonicalSyntax
    }

    /// The constructor and type identity Pliron builds the operation from.
    pub fn concrete_op_info(self) -> (fn(Ptr<Operation>) -> OpObj, std::any::TypeId) {
        match self {
            Self::Func => ops::FuncOp::get_concrete_op_info(),
            Self::Slot => ops::SlotOp::get_concrete_op_info(),
            Self::Const => ops::ConstOp::get_concrete_op_info(),
            Self::Materialize => ops::MaterializeOp::get_concrete_op_info(),
            Self::Binary => ops::BinaryOp::get_concrete_op_info(),
            Self::SimdMake => ops::SimdMakeOp::get_concrete_op_info(),
            Self::Unary => ops::UnaryOp::get_concrete_op_info(),
            Self::SimdConvert => ops::SimdConvertOp::get_concrete_op_info(),
            Self::Use => ops::UseOp::get_concrete_op_info(),
            Self::Store => ops::StoreOp::get_concrete_op_info(),
            Self::Project => ops::ProjectOp::get_concrete_op_info(),
            Self::Load => ops::LoadOp::get_concrete_op_info(),
            Self::RefMake => ops::RefMakeOp::get_concrete_op_info(),
            Self::RefStore => ops::RefStoreOp::get_concrete_op_info(),
            Self::RefRead => ops::RefReadOp::get_concrete_op_info(),
            Self::Copy => ops::CopyOp::get_concrete_op_info(),
            Self::Move => ops::MoveOp::get_concrete_op_info(),
            Self::KeepAlive => ops::KeepAliveOp::get_concrete_op_info(),
            Self::Index => ops::IndexOp::get_concrete_op_info(),
            Self::MultiSet => ops::MultiSetOp::get_concrete_op_info(),
            Self::PointerStorage => ops::PointerStorageOp::get_concrete_op_info(),
            Self::IterInit => ops::IterInitOp::get_concrete_op_info(),
            Self::IterNext => ops::IterNextOp::get_concrete_op_info(),
            Self::Call => ops::CallOp::get_concrete_op_info(),
            Self::ClosureMake => ops::ClosureMakeOp::get_concrete_op_info(),
            Self::FieldGet => ops::FieldGetOp::get_concrete_op_info(),
            Self::SizeOf => ops::SizeOfOp::get_concrete_op_info(),
            Self::RefWrite => ops::RefWriteOp::get_concrete_op_info(),
            Self::MultiIndex => ops::MultiIndexOp::get_concrete_op_info(),
            Self::Slice => ops::SliceOp::get_concrete_op_info(),
            Self::SimdShuffle => ops::SimdShuffleOp::get_concrete_op_info(),
            Self::PackHasNext => ops::PackHasNextOp::get_concrete_op_info(),
            Self::PackNext => ops::PackNextOp::get_concrete_op_info(),
            Self::UninitStorage => ops::UninitStorageOp::get_concrete_op_info(),
            Self::VariantMake => ops::VariantMakeOp::get_concrete_op_info(),
            Self::VariantTest => ops::VariantTestOp::get_concrete_op_info(),
            Self::VariantGet => ops::VariantGetOp::get_concrete_op_info(),
            Self::VariantSet => ops::VariantSetOp::get_concrete_op_info(),
            Self::VariantDeinitWith => ops::VariantDeinitWithOp::get_concrete_op_info(),
            Self::Invoke => ops::InvokeOp::get_concrete_op_info(),
            Self::Drop => ops::DropOp::get_concrete_op_info(),
            Self::Consume => ops::ConsumeOp::get_concrete_op_info(),
            Self::Loans => ops::LoansOp::get_concrete_op_info(),
            Self::Invalidate => ops::InvalidateOp::get_concrete_op_info(),
            Self::Br => ops::BrOp::get_concrete_op_info(),
            Self::CondBr => ops::CondBrOp::get_concrete_op_info(),
            Self::Return => ops::ReturnOp::get_concrete_op_info(),
            Self::Raise => ops::RaiseOp::get_concrete_op_info(),
            Self::Outcome => ops::OutcomeOp::get_concrete_op_info(),
            Self::Resume => ops::ResumeOp::get_concrete_op_info(),
            Self::TryBridge => ops::TryBridgeOp::get_concrete_op_info(),
            Self::RegionExit => ops::RegionExitOp::get_concrete_op_info(),
            Self::Escape => ops::EscapeOp::get_concrete_op_info(),
        }
    }

    /// The registry entry of `op`, or `None` for an operation of another
    /// dialect or an unregistered name.
    pub fn of(ctx: &Context, op: Ptr<Operation>) -> Option<Self> {
        let opid = Operation::get_opid(op, ctx);
        if &*opid.dialect.to_string() != DIALECT {
            return None;
        }
        let name = opid.name.to_string();
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

/// What the exporter does with an operation at the bridge stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExportRule {
    /// Read into the function's frame tables.
    Frame,
    /// Read where an instruction names the place.
    Place,
    /// One MIR instruction, or register transport around one.
    Instruction,
    /// The block's MIR terminator.
    Terminator,
    /// Removed by denormalization before export.
    Denormalized,
}

/// How an operation is spelled in core text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextRule {
    /// Pliron's canonical operation syntax, attributes in one dictionary.
    CanonicalSyntax,
}

/// The MIR forms the importer rejects by name, by their v1 mnemonics.
pub fn rejected_forms() -> Vec<&'static str> {
    mojito_mir::mir::text::INSTRUCTION_MNEMONICS
        .iter()
        .chain(mojito_mir::mir::text::TERMINATOR_MNEMONICS)
        .copied()
        .filter(|mnemonic| importing_op(mnemonic).is_none())
        .collect()
}

/// The wire namespace: Pliron identifiers admit no dot, so the dialect
/// is a single word.
pub const DIALECT: &str = "mojito";

/// One MIR form at one position of the closure.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CensusRow {
    pub function: String,
    /// The nested region path from the function body: empty for the body,
    /// `try0.body` for the first `Try` instruction's body, and so on.
    pub region: String,
    pub form: &'static str,
    pub result_types: Vec<&'static str>,
    pub target: Option<String>,
}

/// Every instruction and terminator of `program`, nested regions included,
/// in function, region, block, and instruction order.
pub fn census(program: &MirProgram) -> Vec<CensusRow> {
    let mut rows = Vec::new();
    for (name, function) in &program.functions {
        walk_region(name, function, "", &function.blocks, &mut rows);
    }
    rows
}

/// The census as tab-separated text with a count per distinct row and the
/// operation that imports the form.
pub fn census_tsv(rows: &[CensusRow]) -> String {
    let mut counts: BTreeMap<&CensusRow, usize> = BTreeMap::new();
    for row in rows {
        *counts.entry(row).or_default() += 1;
    }
    let mut out = String::from("function\tregion\tform\tresult_types\ttarget\tcount\top\n");
    for (row, count) in counts {
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}\t{count}\t{}",
            row.function,
            row.region,
            row.form,
            row.result_types.join(","),
            row.target.as_deref().unwrap_or("-"),
            importing_op(row.form).map_or("-", CoreOpKind::name),
        );
    }
    out
}

/// The forms of `program` no operation imports, with their counts.
pub fn uncovered_forms(program: &MirProgram) -> BTreeMap<&'static str, usize> {
    let mut uncovered = BTreeMap::new();
    for row in census(program) {
        if importing_op(row.form).is_none() {
            *uncovered.entry(row.form).or_default() += 1;
        }
    }
    uncovered
}

/// The operation that imports the MIR form spelled `mnemonic`.
pub fn importing_op(mnemonic: &str) -> Option<CoreOpKind> {
    CoreOpKind::ALL
        .into_iter()
        .find(|kind| kind.mir_forms().contains(&mnemonic))
}

/// The path of the `index`th `try` of `region`, and of one of its parts.
pub fn try_path(region: &str, index: usize) -> String {
    if region.is_empty() {
        format!("try{index}")
    } else {
        format!("{region}.try{index}")
    }
}

fn walk_region(
    name: &str,
    function: &MirFunction,
    region: &str,
    blocks: &[MirBlock],
    rows: &mut Vec<CensusRow>,
) {
    let mut tries = 0usize;
    for block in blocks {
        for instruction in &block.instrs {
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            rows.push(CensusRow {
                function: name.to_string(),
                region: region.to_string(),
                form: instruction_mnemonic(instruction),
                result_types: results
                    .iter()
                    .map(|reg| {
                        function
                            .reg_types
                            .get(&reg.0)
                            .map_or("untyped", type_spelling)
                    })
                    .collect(),
                target: target(instruction),
            });
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
                let parts = [
                    ("body", Some(body)),
                    ("handler", handler.as_ref().map(|(_, blocks)| blocks)),
                    ("else", orelse.as_ref()),
                    ("finally", finalbody.as_ref()),
                ];
                for (part, blocks) in parts {
                    if let Some(blocks) = blocks {
                        walk_region(name, function, &format!("{prefix}.{part}"), blocks, rows);
                    }
                }
            }
        }
        rows.push(CensusRow {
            function: name.to_string(),
            region: region.to_string(),
            form: terminator_mnemonic(&block.term),
            result_types: Vec::new(),
            target: None,
        });
    }
}

fn target(instruction: &MirInstr) -> Option<String> {
    match instruction {
        MirInstr::Call { func, .. } => Some(func.0.clone()),
        MirInstr::MethodCall {
            method, resolved, ..
        } => Some(resolved.clone().unwrap_or_else(|| method.clone())),
        _ => None,
    }
}
