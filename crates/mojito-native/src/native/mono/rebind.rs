//! The per-instance judgement of a generator's `rebind`s.
//!
//! A template reads a `rebind[Dest](x)` operand at `Dest` through a value
//! rebind or a place's [`Proj::Rebind`] step. Once an instance's bindings
//! close both types and its compile-time branches are decided, the
//! elaborator asserts each surviving rebind's two types equal and erases it,
//! as upstream's `processRebindOp` does: an untaken arm's rebind is never
//! judged, and a mismatch fails the instance. A rebind of a constant that
//! other instructions read too becomes that constant at the rebind's type,
//! as `kgen.rebind` folds over a constant attribute: the folder keeps one
//! constant per value, so the constant is not the rebind's alone to retype.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_mir::mir::Proj;
use mojito_mir::mir::verify::{instruction_places_mut, place_base_ty, types_agree};
use mojito_mir::mir::{
    count_register_reads, instruction_regions_mut, instruction_regs_mut, terminator_regs_mut,
};

impl Specializer<'_> {
    /// Judge every rebind of the instance `function` of
    /// `template`: a value rebind's `dest` register becomes its operand's,
    /// and a rebound place loses its step, whose type now names the
    /// storage's own. A chain's root takes the type of its outermost rebind,
    /// the one the program reads the value at, when only the chain reads it:
    /// a `SIMD` built from its storage (`Self(mlir_value=...)`), built from
    /// a `SIMD`, is the `SIMD`. A rebind of a constant others read too
    /// ([`shared_constants`]) becomes the constant at its own type instead.
    pub(super) fn discharge_rebinds(
        &self,
        template: &str,
        function: &mut MirFunction,
    ) -> Result<(), MonoError> {
        let mut aliases = HashMap::new();
        let shared = shared_constants(function);
        judge_blocks(
            &mut function.body,
            &function.reg_types,
            &shared,
            &mut aliases,
        )
        .map_err(|mismatch| self.rebind_mismatch(template, &mismatch.0, &mismatch.1))?;
        if !aliases.is_empty() {
            let mut reads = vec![0; function.n_regs as usize];
            count_register_reads(&mut function.body, &mut reads);
            for value in aliases.values() {
                if let Some(count) = reads.get_mut(value.0 as usize) {
                    *count += 1;
                }
            }
            let sole_reader: HashMap<u32, u32> = aliases
                .iter()
                .filter(|(_, value)| reads.get(value.0 as usize) == Some(&1))
                .map(|(dest, value)| (value.0, *dest))
                .collect();
            let outermost = |mut reg: u32| {
                while let Some(dest) = sole_reader.get(&reg) {
                    reg = *dest;
                }
                reg
            };
            let retyped: Vec<(u32, Ty)> = aliases
                .iter()
                .filter(|(_, root)| !aliases.contains_key(&root.0))
                .filter_map(|(dest, root)| {
                    let read_at = function.reg_types.get(&outermost(*dest))?;
                    (reads.get(root.0 as usize) == Some(&1)).then(|| (root.0, read_at.clone()))
                })
                .collect();
            function.reg_types.extend(retyped);
            rename_blocks(&mut function.body, &aliases);
        }
        Ok(())
    }

    fn rebind_mismatch(&self, template: &str, input: &Ty, result: &Ty) -> MonoError {
        MonoError {
            kind: MonoErrorKind::Instantiation,
            ..self.error(
                Some(template),
                format!("rebind input type '{input}' does not match result type '{result}'"),
            )
        }
    }
}

/// A judged rebind's operand and target types where they disagree.
type Mismatch = Box<(Ty, Ty)>;

/// Judge the rebinds of one block list and the regions below it, recording
/// each value rebind's `dest → value` and removing the instruction or place
/// step; the first mismatch is returned as its `(input, result)` types.
fn judge_blocks(
    block: &mut MirBlock,
    reg_types: &HashMap<u32, Ty>,
    shared: &HashMap<u32, Const>,
    aliases: &mut HashMap<u32, Reg>,
) -> Result<(), Mismatch> {
    let mut judged = Vec::with_capacity(block.instrs.len());
    for mut instruction in std::mem::take(&mut block.instrs) {
        for region in instruction_regions_mut(&mut instruction) {
            judge_blocks(region, reg_types, shared, aliases)?;
        }
        for place in instruction_places_mut(&mut instruction) {
            discharge_place(place)?;
        }
        if let MirInstr::Rebind { dest, value } = instruction {
            if let (Some(input), Some(result)) = (reg_types.get(&value.0), reg_types.get(&dest.0))
                && !types_agree(input, result)
            {
                return Err(Box::new((input.clone(), result.clone())));
            }
            match shared.get(&value.0) {
                Some(k) if reg_types.get(&value.0) != reg_types.get(&dest.0) => {
                    judged.push(MirInstr::Const { dest, k: k.clone() });
                }
                _ => {
                    aliases.insert(dest.0, value);
                }
            }
            continue;
        }
        judged.push(instruction);
    }
    block.instrs = judged;
    Ok(())
}

/// The constant each register of `function` defined once by a `const`
/// holds, when more than one instruction or terminator reads it.
fn shared_constants(function: &mut MirFunction) -> HashMap<u32, Const> {
    let mut reads = vec![0; function.n_regs as usize];
    count_register_reads(&mut function.body, &mut reads);
    let mut definitions = HashMap::<u32, u32>::new();
    let mut constants = HashMap::new();
    collect_constants(&function.body, &mut definitions, &mut constants);
    constants.retain(|reg, _| {
        definitions.get(reg) == Some(&1) && reads.get(*reg as usize).is_some_and(|n| *n > 1)
    });
    constants
}

/// Count every register `block` and its regions define, and record each
/// `const`'s value.
fn collect_constants(
    block: &MirBlock,
    definitions: &mut HashMap<u32, u32>,
    constants: &mut HashMap<u32, Const>,
) {
    for instruction in &block.instrs {
        let mut results = Vec::new();
        mojito_mir::mir::verify::instruction_result_regs(instruction, &mut results);
        for reg in results {
            *definitions.entry(reg.0).or_default() += 1;
        }
        if let MirInstr::Const { dest, k } = instruction {
            constants.insert(dest.0, k.clone());
        }
        for region in mojito_mir::mir::instruction_regions(instruction) {
            collect_constants(region, definitions, constants);
        }
    }
}

/// Erase a place's rebind steps, judging each against the type it applies
/// to: the storage's own type must agree with the one the rebind names.
fn discharge_place(place: &mut MirPlace) -> Result<(), Mismatch> {
    if !place.proj.iter().any(|step| matches!(step, Proj::Rebind)) {
        return Ok(());
    }
    let mut applied = place_base_ty(place);
    for (step, ty) in place.proj.iter().zip(&place.projection_tys) {
        if let (Proj::Rebind, Some(input)) = (step, applied)
            && !types_agree(input, ty)
        {
            return Err(Box::new((input.clone(), ty.clone())));
        }
        applied = Some(ty);
    }
    *place = place.storage().into_owned();
    Ok(())
}

/// Read every use of a judged value rebind's `dest` as its operand.
fn rename_blocks(block: &mut MirBlock, aliases: &HashMap<u32, Reg>) {
    let resolve = |reg: &mut Reg| {
        while let Some(value) = aliases.get(&reg.0) {
            *reg = *value;
        }
    };
    for instruction in &mut block.instrs {
        for region in instruction_regions_mut(instruction) {
            rename_blocks(region, aliases);
        }
        instruction_regs_mut(instruction)
            .into_iter()
            .for_each(resolve);
    }
    terminator_regs_mut(&mut block.term)
        .into_iter()
        .for_each(resolve);
}
