//! Whole-pack spreads in an instance: a call, a method call, or a nested
//! `def`'s call that spreads the caller's collector (`show(*args)`,
//! `Sink().take(*args)`, `count(*args^)`) passes that pack itself once the
//! bindings close it, as upstream's call passes the `VariadicPack`; an LLVM
//! intrinsic call (upstream's loaded `!kgen.pack` operand) takes the bound
//! pack's elements instead. A value pack
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
/// spread into an LLVM intrinsic call is expanded: each element is read from
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
    expand_load_indirect(blocks, &mut tables);
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
        // An LLVM intrinsic call over the loaded elements a tuple holds
        // (`get_loaded_kgen_pack`'s expanded `kgen.struct.load_indirect`)
        // passes them.
        if matches!(
            &blocks[at].instrs[index],
            MirInstr::CallLlvmIntrinsic { .. }
        ) && let MirInstr::MakeTuple { elems, .. } = &blocks[read_block].instrs[read]
        {
            let elems = elems.clone();
            let call = spread_operands(&mut blocks[at].instrs[index])
                .expect("the spread call was matched above");
            call.args.splice(position..=position, elems);
            *call.spread = None;
            index += 1;
            continue;
        }
        // The collector the call builds is the one operand it passes, as
        // is a homogeneous collector's `VariadicList` and a forwarded
        // `VariadicPack`.
        if matches!(blocks[read_block].instrs[read], MirInstr::MakeTuple { .. })
            || tables.reg_types.get(&pack.0).is_some_and(|ty| {
                mojito_types::types::variadic_list_element(ty).is_some()
                    || mojito_types::types::variadic_pack_owned(ty).is_some()
            })
        {
            index += 1;
            continue;
        }
        let elements = match tables.reg_types.get(&pack.0).map(Ty::kind) {
            Some(TyKind::Tuple(elements)) => elements.clone(),
            _ => return Err(tables.error("a spread's pack is still a parameter")),
        };
        // Any callee takes the caller's pack itself; an LLVM intrinsic call
        // passes the loaded pack's elements.
        if !matches!(
            &blocks[at].instrs[index],
            MirInstr::CallLlvmIntrinsic { .. }
        ) {
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

/// Expand each `kgen.struct.load_indirect` of `blocks` over a bound
/// reference pack into a read of each element through its reference
/// (`lit.ref.pack.extract`, then `ref.read`) and the `tuple.make` of the
/// values, as upstream's LLVM lowering loads each pointer.
fn expand_load_indirect(blocks: &mut [MirBlock], tables: &mut SpreadTables<'_>) {
    for block in blocks {
        let mut index = 0;
        while index < block.instrs.len() {
            for region in mojito_mir::mir::instruction_regions_mut(&mut block.instrs[index]) {
                expand_load_indirect(region, tables);
            }
            let MirInstr::LoadIndirect { dest, pack } = block.instrs[index] else {
                index += 1;
                continue;
            };
            let Some(TyKind::LitRefPack { elements, origin }) =
                tables.reg_types.get(&pack.0).map(Ty::kind)
            else {
                index += 1;
                continue;
            };
            if mojito_types::types::pack_spread(elements).is_some() {
                index += 1;
                continue;
            }
            let (elements, origin) = (elements.clone(), origin.clone());
            let span = tables.spans.0.get(&dest.0).cloned();
            let mut fresh = |ty: Ty| {
                let register = Reg(*tables.n_regs);
                *tables.n_regs += 1;
                tables.reg_types.insert(register.0, ty);
                if let Some(span) = &span {
                    tables.spans.0.insert(register.0, span.clone());
                }
                register
            };
            let mut reads = Vec::with_capacity(elements.len() * 3 + 1);
            let mut values = Vec::with_capacity(elements.len());
            for (element_index, element) in elements.iter().enumerate() {
                let at = fresh(Ty::int());
                reads.push(MirInstr::Const {
                    dest: at,
                    k: Const::Int(
                        i64::try_from(element_index).expect("a pack's arity fits an Int"),
                    ),
                });
                let reference = fresh(
                    TyKind::LitRef {
                        element: element.clone(),
                        origin: origin.clone(),
                    }
                    .into(),
                );
                reads.push(MirInstr::Index {
                    dest: reference,
                    base: pack,
                    index: at,
                    base_place: None,
                    index_place: None,
                    call: None,
                    intrinsic: Some(mojito_mir::mir::MirIntrinsicSubscript::TupleStorage),
                });
                let value = fresh(element.clone());
                reads.push(MirInstr::ReadRef {
                    dest: value,
                    reference,
                });
                values.push(value);
            }
            reads.push(MirInstr::MakeTuple {
                dest,
                elems: values,
                element_types: Some(elements),
            });
            let inserted = reads.len();
            block.instrs.splice(index..=index, reads);
            index += inserted;
        }
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
