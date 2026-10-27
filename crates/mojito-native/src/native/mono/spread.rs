//! Whole-pack spreads in an instance: a call, a method call, or a nested
//! `def`'s call that spreads the caller's collector (`show(*args)`,
//! `Sink().take(*args)`, `count(*args^)`) passes that pack itself once the
//! bindings close it, as upstream's call passes the `VariadicPack`; the
//! builtin `print` and an LLVM intrinsic call (upstream's loaded
//! `!kgen.pack` operand) take the bound pack's elements instead. A value pack
//! spread into a bracket (`f[*vs]()`) retires once the call names its
//! instance. The collector a call builds (`tuple.make`, wrapped in a
//! homogeneous collector's `VariadicList`) stays the one operand it passes.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::Proj;
use mojito_types::types::TyKind;

/// Check every whole-pack spread of the instance: the spread argument's
/// register, the caller's collector read in place or moved, must be typed
/// the bound pack's tuple, and the call passes it unchanged. A spread whose
/// pack the bindings leave open is the instance's unsupported boundary. A
/// spread into the builtin `print` is expanded: each element is read from
/// the collector's element place (or moved out of it, when the caller
/// transferred the pack) and passed as one of its arguments, the call's
/// `spread` cleared and the collector's own read gone.
pub(super) fn expand_pack_spreads(
    template: &str,
    function: &mut MirFunction,
) -> Result<(), MonoError> {
    let MirFunction {
        blocks,
        n_regs,
        var_tys,
        reg_types,
        spans,
        ..
    } = function;
    let mut tables = SpreadTables {
        template,
        n_regs,
        var_tys,
        reg_types,
        spans,
    };
    expand_in_blocks(blocks, &mut tables)
}

/// Retire the bound list of a value pack spread into a bracket
/// (`f[*vs]()`, `PL[*vs]()`): the call it fed names its instance, whose key
/// holds the list, and no register reads it.
pub(super) fn retire_forwarded_packs(function: &mut MirFunction) {
    let forwarded: HashSet<u32> = function
        .reg_types
        .iter()
        .filter(|(_, ty)| matches!(ty.kind(), TyKind::VariadicPack(_)))
        .map(|(register, _)| *register)
        .collect();
    if forwarded.is_empty() {
        return;
    }
    let mut retired = HashSet::new();
    retire_in_blocks(&mut function.blocks, &forwarded, &mut retired);
    function
        .reg_types
        .retain(|register, _| !retired.contains(register));
}

/// Pass the elements of the collector a method call builds as its
/// arguments when the builtin its receiver is answers the call (`write`
/// on the builtin-string writer), which takes its values one by one, as
/// the builtin `print` does. The collector's drop, once the call returns,
/// becomes its elements', first to last, as a `Tuple`'s destroys them.
pub(super) fn unpack_builtin_collectors(blocks: &mut [MirBlock], reg_types: &mut HashMap<u32, Ty>) {
    let mut retired = Vec::new();
    unpack_in_blocks(blocks, reg_types, &mut retired);
    for register in retired {
        reg_types.remove(&register);
    }
}

struct SpreadTables<'a> {
    template: &'a str,
    n_regs: &'a mut u32,
    var_tys: &'a HashMap<mojito_hir::hir::VarId, Ty>,
    reg_types: &'a mut HashMap<u32, Ty>,
    spans: &'a mut mojito_mir::mir::SpanTable,
}

/// The collector a spread register reads: its place and whether the read
/// moved the pack out of its slot.
struct Collector {
    place: MirPlace,
    moved: bool,
}

fn expand_in_blocks(
    blocks: &mut [MirBlock],
    tables: &mut SpreadTables<'_>,
) -> Result<(), MonoError> {
    for at in 0..blocks.len() {
        expand_in_block(blocks, at, tables)?;
    }
    Ok(())
}

/// Expand the spreads of the block `at` of `blocks`. A spread's collector
/// is read once, in the block of the call or, once a pass has split that
/// block, in one before it.
fn expand_in_block(
    blocks: &mut [MirBlock],
    at: usize,
    tables: &mut SpreadTables<'_>,
) -> Result<(), MonoError> {
    let mut index = 0;
    while index < blocks[at].instrs.len() {
        let regions = mojito_mir::mir::instruction_regions_mut(&mut blocks[at].instrs[index]);
        if !regions.is_empty() {
            for region in regions {
                expand_in_blocks(region, tables)?;
            }
            index += 1;
            continue;
        }
        let Some((position, pack)) = spread_operands(&mut blocks[at].instrs[index])
            .and_then(|call| call.spread.map(|position| (position, call.args[position])))
        else {
            index += 1;
            continue;
        };
        // A collector the callee no longer takes (an unused argument whose
        // read the removal erased) is dropped with the call's rewrite.
        let Some((read_block, read)) = (0..=at).rev().find_map(|block| {
            let end = if block == at {
                index
            } else {
                blocks[block].instrs.len()
            };
            blocks[block].instrs[..end]
                .iter()
                .rposition(|instruction| defines(instruction, pack))
                .map(|read| (block, read))
        }) else {
            index += 1;
            continue;
        };
        // The collector the call builds is the one operand it passes, as
        // is a homogeneous collector's `VariadicList`.
        if matches!(blocks[read_block].instrs[read], MirInstr::MakeTuple { .. })
            || tables
                .reg_types
                .get(&pack.0)
                .is_some_and(|ty| mojito_types::types::variadic_list_element(ty).is_some())
        {
            index += 1;
            continue;
        }
        let elements = match tables.reg_types.get(&pack.0).map(Ty::kind) {
            Some(TyKind::Tuple(elements)) => elements.clone(),
            _ => return Err(tables.error("a spread's pack is still a parameter")),
        };
        // Any callee but the builtin `print` takes the caller's pack itself;
        // an LLVM intrinsic call passes the loaded pack's elements.
        if !matches!(&blocks[at].instrs[index],
            MirInstr::Call { func, .. } if func.0 == "print")
            && !matches!(
                &blocks[at].instrs[index],
                MirInstr::CallLlvmIntrinsic { .. }
            )
        {
            index += 1;
            continue;
        }
        let collector = collector_read(&blocks[read_block].instrs[read], pack, tables.var_tys)
            .ok_or_else(|| tables.error("a spread reads no collector"))?;
        let span = tables.spans.0.get(&pack.0).cloned();
        let mut loads = Vec::with_capacity(elements.len());
        let mut registers = Vec::with_capacity(elements.len());
        for (element_index, element) in elements.into_iter().enumerate() {
            let dest = Reg(*tables.n_regs);
            *tables.n_regs += 1;
            tables.reg_types.insert(dest.0, element.clone());
            if let Some(span) = &span {
                tables.spans.0.insert(dest.0, span.clone());
            }
            let mut place = collector.place.clone();
            place.project(Proj::ConstIndex(element_index), element);
            loads.push(if collector.moved {
                MirInstr::MovePlace { dest, place }
            } else {
                MirInstr::LoadPlace { dest, place }
            });
            registers.push(dest);
        }
        let block = &mut blocks[at];
        let call =
            spread_operands(&mut block.instrs[index]).expect("the spread call was matched above");
        let count = registers.len();
        call.args
            .splice(position..=position, registers.iter().copied());
        if let Some(arg_places) = call.arg_places {
            arg_places.splice(position..=position, std::iter::repeat_n(None, count));
        }
        *call.spread = None;
        tables.reg_types.remove(&pack.0);
        tables.spans.0.remove(&pack.0);
        // A moved pack's drop once the call returns becomes its elements',
        // first to last, as a `Tuple`'s destroys them.
        if collector.moved
            && let Some(drop) = block.instrs[index..].iter().position(
                |instruction| matches!(instruction, MirInstr::Drop { reg, .. } if *reg == pack),
            )
        {
            block.instrs.splice(
                index + drop..=index + drop,
                registers.into_iter().map(|reg| MirInstr::Drop {
                    reg,
                    destructor: None,
                }),
            );
        }
        let inserted = loads.len();
        block.instrs.splice(index..index, loads);
        // The collector's whole read is dead once the elements are read.
        blocks[read_block].instrs.remove(read);
        index += inserted;
        if read_block == at {
            index -= 1;
        }
    }
    Ok(())
}

/// The operands a whole-pack spread rewrites on a direct call, a method
/// call, an indirect call of a nested `def`, or an LLVM intrinsic call: its
/// position, the positional arguments, and their places (an intrinsic's
/// operands have none).
struct SpreadOperands<'a> {
    spread: &'a mut Option<usize>,
    args: &'a mut Vec<Reg>,
    arg_places: Option<&'a mut Vec<Option<MirPlace>>>,
}

const fn spread_operands(instruction: &mut MirInstr) -> Option<SpreadOperands<'_>> {
    match instruction {
        MirInstr::Call {
            spread,
            args,
            arg_places,
            ..
        }
        | MirInstr::MethodCall {
            spread,
            args,
            arg_places,
            ..
        }
        | MirInstr::CallIndirect {
            spread,
            args,
            arg_places,
            ..
        } => Some(SpreadOperands {
            spread,
            args,
            arg_places: Some(arg_places),
        }),
        MirInstr::CallLlvmIntrinsic { spread, args, .. } => Some(SpreadOperands {
            spread,
            args,
            arg_places: None,
        }),
        _ => None,
    }
}

/// The collector the register `pack` was read from, among the instructions
/// before the call: a whole place load (a read pack), a whole move (a
/// transferred pack), or a variable move.
fn collector_read(
    instruction: &MirInstr,
    pack: Reg,
    var_tys: &HashMap<mojito_hir::hir::VarId, Ty>,
) -> Option<Collector> {
    match instruction {
        MirInstr::LoadPlace { dest, place } if *dest == pack => Some(Collector {
            place: place.clone(),
            moved: false,
        }),
        MirInstr::MovePlace { dest, place } if *dest == pack => Some(Collector {
            place: place.clone(),
            moved: true,
        }),
        MirInstr::UseVar {
            dest, var, mode, ..
        } if *dest == pack => Some(Collector {
            place: MirPlace::root(*var, var_tys.get(var).cloned()),
            moved: mode.moves(),
        }),
        _ => None,
    }
}

fn defines(instruction: &MirInstr, register: Reg) -> bool {
    matches!(
        instruction,
        MirInstr::LoadPlace { dest, .. }
            | MirInstr::MovePlace { dest, .. }
            | MirInstr::UseVar { dest, .. }
            | MirInstr::MakeTuple { dest, .. }
            if *dest == register
    )
}

impl SpreadTables<'_> {
    fn error(&self, construct: &str) -> MonoError {
        MonoError {
            kind: MonoErrorKind::Unsupported,
            function: Some(self.template.to_string()),
            construct: construct.to_string(),
        }
    }
}

fn retire_in_blocks(blocks: &mut [MirBlock], forwarded: &HashSet<u32>, retired: &mut HashSet<u32>) {
    for block in blocks {
        block.instrs.retain_mut(|instruction| match instruction {
            MirInstr::Const {
                dest,
                k: Const::Value(CtValue::Tuple(_)),
            } if forwarded.contains(&dest.0) => {
                retired.insert(dest.0);
                false
            }
            instruction => {
                for region in mojito_mir::mir::instruction_regions_mut(instruction) {
                    retire_in_blocks(region, forwarded, retired);
                }
                true
            }
        });
    }
}

fn unpack_in_blocks(blocks: &mut [MirBlock], reg_types: &HashMap<u32, Ty>, retired: &mut Vec<u32>) {
    for block in blocks {
        let mut index = 0;
        while index < block.instrs.len() {
            for region in mojito_mir::mir::instruction_regions_mut(&mut block.instrs[index]) {
                unpack_in_blocks(region, reg_types, retired);
            }
            let collector = match &block.instrs[index] {
                MirInstr::MethodCall {
                    recv,
                    spread: Some(position),
                    args,
                    ..
                } if !matches!(
                    reg_types.get(&recv.0).map(|ty| peel_refs(ty).kind()),
                    Some(TyKind::Struct(..))
                ) =>
                {
                    args.get(*position).map(|register| (*position, *register))
                }
                _ => None,
            };
            let built = collector.and_then(|(position, register)| {
                block.instrs[..index]
                    .iter()
                    .rposition(|instruction| {
                        matches!(instruction, MirInstr::MakeTuple { dest, .. } if *dest == register)
                    })
                    .map(|at| (position, register, at))
            });
            let Some((position, register, at)) = built else {
                index += 1;
                continue;
            };
            let MirInstr::MakeTuple { elems, .. } = block.instrs.remove(at) else {
                unreachable!("the collector's tuple.make was matched above");
            };
            if let Some(drop) = block.instrs[index..].iter().position(
                |instruction| matches!(instruction, MirInstr::Drop { reg, .. } if *reg == register),
            ) {
                block.instrs.splice(
                    index + drop..=index + drop,
                    elems.iter().map(|element| MirInstr::Drop {
                        reg: *element,
                        destructor: None,
                    }),
                );
            }
            if let Some(call) = spread_operands(&mut block.instrs[index - 1]) {
                let count = elems.len();
                call.args.splice(position..=position, elems);
                if let Some(arg_places) = call.arg_places {
                    arg_places.splice(position..=position, std::iter::repeat_n(None, count));
                }
                *call.spread = None;
            }
            retired.push(register.0);
        }
    }
}
