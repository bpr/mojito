//! The operand places MIR text older than 1.202 writes on each call-shaped
//! instruction, held by the reader for the load-time upgrades.
//!
//! Upstream's `lit.call` names no storage beside its operands, and the IR
//! carries no table just so older artifacts still load: as an MLIR
//! dialect's bytecode upgrade hook reads an old encoding inside the reader,
//! the text decoder reads each older call's place fields into a
//! [`LegacyCallPlaces`], and the upgrades pair those with the call-shaped
//! instructions ([`MirInstr::is_call_shaped`]) of each function in decode
//! order: a block's instructions in order, a region holder's regions in
//! [`super::instruction_regions`] order. A call-shaped instruction holds no
//! region and no upgrade adds, removes, or reorders one, so a walk in that
//! order meets them as the decoder pushed them, whether it visits a region
//! holder before or after its regions.

use crate::mir::{MirFunction, MirInstr, MirPlace, instruction_regions};

/// Each function's legacy call places, parallel to the functions the
/// decoder read.
#[derive(Debug, Default)]
pub struct LegacyOperandTables {
    functions: Vec<Vec<LegacyCallPlaces>>,
}

impl LegacyOperandTables {
    /// Record the places of the next function's call-shaped instructions,
    /// in decode order.
    pub fn push_function(&mut self, calls: Vec<LegacyCallPlaces>) {
        self.functions.push(calls);
    }

    /// A cursor over the places of the function at `index`; one over none
    /// when the decoder recorded no such function.
    pub fn cursor(&self, index: usize) -> LegacyCursor<'_> {
        LegacyCursor {
            calls: self.functions.get(index).map_or(&[], Vec::as_slice),
            next: 0,
        }
    }
}

/// One call-shaped instruction's places as older MIR text wrote them.
#[derive(Debug, Default, Clone)]
pub struct LegacyCallPlaces {
    /// A method call's `recv_place`, or a subscript's `base_place`,
    /// `object_place`, or `receiver_place`.
    pub receiver: Option<MirPlace>,
    /// `arg_places`.
    pub positional: Vec<Option<MirPlace>>,
    /// `kwarg_places`.
    pub keyword: Vec<Option<MirPlace>>,
    /// An `index.get`'s `index_place`, or an `index.multi_set`'s
    /// `value_place`.
    pub trailing: Option<MirPlace>,
}

impl LegacyCallPlaces {
    /// Each regular operand's place, in the order the call lists its
    /// operands: positional then keyword, an indexed `index.get`'s index,
    /// or an `index.multi_set`'s indices then its value.
    pub fn operands(&self, instruction: &MirInstr) -> Vec<Option<&MirPlace>> {
        match instruction {
            MirInstr::Index { call, .. } => call
                .as_ref()
                .filter(|call| !call.arguments.is_empty())
                .map(|_| self.trailing.as_ref())
                .into_iter()
                .collect(),
            MirInstr::MultiSet { .. } => self
                .positional
                .iter()
                .map(Option::as_ref)
                .chain([self.trailing.as_ref()])
                .collect(),
            _ => self
                .positional
                .iter()
                .chain(&self.keyword)
                .map(Option::as_ref)
                .collect(),
        }
    }
}

/// A walk's position among one function's legacy call places.
pub struct LegacyCursor<'t> {
    calls: &'t [LegacyCallPlaces],
    next: usize,
}

impl<'t> LegacyCursor<'t> {
    /// Advance past `instruction`, returning its places when it is
    /// call-shaped.
    pub fn visit(&mut self, instruction: &MirInstr) -> Option<&'t LegacyCallPlaces> {
        if !instruction.is_call_shaped() {
            return None;
        }
        let places = self.calls.get(self.next);
        self.next += 1;
        places
    }
}

/// The number of call-shaped instructions in `function`, which the
/// decoder's tables for it must match.
pub fn call_shaped_count(function: &MirFunction) -> usize {
    fn count(block: &crate::mir::MirBlock) -> usize {
        block
            .instrs
            .iter()
            .map(|instruction| {
                usize::from(instruction.is_call_shaped())
                    + instruction_regions(instruction)
                        .into_iter()
                        .map(count)
                        .sum::<usize>()
            })
            .sum()
    }
    count(&function.body)
}
