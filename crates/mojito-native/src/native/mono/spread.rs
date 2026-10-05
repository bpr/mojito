//! Expanding a whole-pack spread: an instance replaces the collector a call
//! spreads (`show(*args)`) with the bound pack's element places.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::Proj;

/// Expand every whole-pack spread of the instance, as the elaborator does for
/// upstream's pack expansion: the spread argument's register, typed the
/// collector's tuple once the pack is bound, becomes one register per
/// element, each read from the collector's element place (or moved out of
/// it, when the caller transferred the pack), and the call's `spread` is
/// cleared. The collector's own read goes with it. A spread whose pack the
/// bindings leave open is the instance's unsupported boundary.
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
    for block in blocks {
        expand_in_block(block, tables)?;
    }
    Ok(())
}

fn expand_in_block(block: &mut MirBlock, tables: &mut SpreadTables<'_>) -> Result<(), MonoError> {
    let mut index = 0;
    while index < block.instrs.len() {
        if let MirInstr::Try {
            body,
            handler,
            orelse,
            finalbody,
            ..
        } = &mut block.instrs[index]
        {
            for region in std::iter::once(body)
                .chain(handler.iter_mut().map(|(_, blocks)| blocks))
                .chain(orelse.iter_mut())
                .chain(finalbody.iter_mut())
            {
                expand_in_blocks(region, tables)?;
            }
            index += 1;
            continue;
        }
        let MirInstr::Call {
            spread: Some(position),
            args,
            ..
        } = &block.instrs[index]
        else {
            index += 1;
            continue;
        };
        let (position, pack) = (*position, args[*position]);
        let collector = collector_read(&block.instrs[..index], pack, tables.var_tys)
            .ok_or_else(|| tables.error("a spread reads no collector"))?;
        let elements = match tables.reg_types.get(&pack.0) {
            Some(Ty::Tuple(elements)) => elements.clone(),
            _ => return Err(tables.error("a spread's pack is still a parameter")),
        };
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
        let MirInstr::Call {
            spread,
            args,
            arg_places,
            ..
        } = &mut block.instrs[index]
        else {
            unreachable!("the spread call was matched above");
        };
        let count = registers.len();
        args.splice(position..=position, registers);
        arg_places.splice(position..=position, std::iter::repeat_n(None, count));
        *spread = None;
        let inserted = loads.len();
        block.instrs.splice(index..index, loads);
        // The collector's whole read is dead once the elements are read.
        let read = block.instrs[..index]
            .iter()
            .rposition(|instruction| defines(instruction, pack))
            .expect("the collector read was found above");
        block.instrs.remove(read);
        tables.reg_types.remove(&pack.0);
        tables.spans.0.remove(&pack.0);
        index += inserted;
    }
    Ok(())
}

/// The collector the register `pack` was read from, among the instructions
/// before the call: a whole place load (a read pack), a whole move (a
/// transferred pack), or a variable move.
fn collector_read(
    instructions: &[MirInstr],
    pack: Reg,
    var_tys: &HashMap<mojito_hir::hir::VarId, Ty>,
) -> Option<Collector> {
    instructions
        .iter()
        .rev()
        .find_map(|instruction| match instruction {
            MirInstr::LoadPlace { dest, place } if *dest == pack => Some(Collector {
                place: place.clone(),
                moved: false,
            }),
            MirInstr::MovePlace { dest, place } if *dest == pack => Some(Collector {
                place: place.clone(),
                moved: true,
            }),
            MirInstr::UseVar { dest, var, mode } if *dest == pack => Some(Collector {
                place: MirPlace::root(*var, var_tys.get(var).cloned()),
                moved: matches!(mode, mojito_mir::mir::UseMode::Move),
            }),
            _ => None,
        })
}

fn defines(instruction: &MirInstr, register: Reg) -> bool {
    matches!(
        instruction,
        MirInstr::LoadPlace { dest, .. } | MirInstr::MovePlace { dest, .. } | MirInstr::UseVar { dest, .. }
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
