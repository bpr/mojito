//! Pack drops refined per element: upstream's `Tuple.__del__` and
//! `VariadicPack.__deinit__` unrolling a `comptime for` of
//! `destroy_pointee` over the elements.
//!
//! A pack aggregate (a `Tuple`'s private storage, a heterogeneous
//! collector) has no destructor of its own, so a `drop.var` or `drop.place`
//! of one names none. Upstream's library destructor destroys each element
//! in its own step, in which destructor insertion names that element's
//! `__del__`: a tuple's first to last, an owning collector's last to first.
//! Here such a drop becomes one `drop.place` per element that runs code,
//! which naming then gives its element's destructor, as a field's; a
//! whole slot ends after them with a `consume.var`, which destroys nothing
//! more, and a part pack ends with its last element. A drop whose
//! pack type is still symbolic waits for the elaborator, which refines an
//! instance's once its elements are concrete.

use crate::mir::{MirBlock, MirFunction, MirInstr, MirPlace, Proj, instruction_regions_mut};
use mojito_hir::hir::VarId;
use mojito_types::types::{Ty, TyKind};

/// Refine every drop of a concrete pack aggregate in `function` per element.
///
/// Each `drop.var` and `drop.place`, regions included, becomes the drops of
/// its elements that run code (`runs_no_code` answering no), a whole
/// slot's end after them; `owned_pack` is the slot of the function's owning
/// collector, whose elements die last to first.
pub fn refine_pack_drops(
    function: &mut MirFunction,
    owned_pack: Option<VarId>,
    runs_no_code: &dyn Fn(&Ty) -> bool,
) {
    let mut blocks = std::mem::take(&mut function.blocks);
    Refine {
        function: &*function,
        owned_pack,
        runs_no_code,
    }
    .blocks(&mut blocks);
    function.blocks = blocks;
}

struct Refine<'a> {
    function: &'a MirFunction,
    owned_pack: Option<VarId>,
    runs_no_code: &'a dyn Fn(&Ty) -> bool,
}

impl Refine<'_> {
    fn blocks(&self, blocks: &mut [MirBlock]) {
        for block in blocks {
            let instrs = std::mem::take(&mut block.instrs);
            let mut refined = Vec::with_capacity(instrs.len());
            for mut instruction in instrs {
                for region in instruction_regions_mut(&mut instruction) {
                    self.blocks(region);
                }
                self.instruction(instruction, &mut refined);
            }
            block.instrs = refined;
        }
    }

    /// Push `instruction`, or the element drops refining it.
    fn instruction(&self, instruction: MirInstr, out: &mut Vec<MirInstr>) {
        let place = match &instruction {
            MirInstr::DropVar {
                var,
                destructor: None,
            } => self
                .function
                .var_tys
                .get(var)
                .filter(|ty| pack_elements(ty).is_some())
                .map(|ty| MirPlace::root(*var, Some(ty.clone()))),
            MirInstr::DropPlace {
                place,
                destructor: None,
            } if place.ty.as_ref().and_then(pack_elements).is_some() => Some(place.clone()),
            _ => None,
        };
        match place {
            Some(place) => self.pack_drops(&place, out),
            None => out.push(instruction),
        }
    }

    /// Push the drops of the pack `place`'s elements that run code, each
    /// nested pack refined in turn, in the order its library destructor
    /// destroys them, then the end of the pack's slot when it is one.
    fn pack_drops(&self, place: &MirPlace, out: &mut Vec<MirInstr>) {
        let Some(elements) = place.ty.as_ref().and_then(pack_elements) else {
            return;
        };
        let mut positions: Vec<usize> = (0..elements.len()).collect();
        if place.is_whole_root() && Some(place.root) == self.owned_pack {
            positions.reverse();
        }
        for position in positions {
            let element = &elements[position];
            if (self.runs_no_code)(element) {
                continue;
            }
            let mut element_place = place.clone();
            element_place.project(Proj::ConstIndex(position), element.clone());
            if pack_elements(element).is_some() {
                self.pack_drops(&element_place, out);
            } else {
                out.push(MirInstr::DropPlace {
                    place: element_place,
                    destructor: None,
                });
            }
        }
        if place.is_whole_root() {
            out.push(MirInstr::ConsumeVar { var: place.root });
        }
    }
}

/// The element types of a pack aggregate whose element list is known: not
/// the spread of a variadic parameter (`__RuntimeTuple[*Ts]` in its
/// template), whose elements only the elaborator lists.
fn pack_elements(ty: &Ty) -> Option<&[Ty]> {
    match ty.kind() {
        TyKind::Tuple(elements) | TyKind::RuntimePack(elements)
            if !elements.iter().any(|element| {
                matches!(element.kind(), TyKind::Param { binder, .. } if binder.name.starts_with('*'))
            }) =>
        {
            Some(elements)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::MirTerm;
    use mojito_types::param_expr::ParamRef;
    use std::collections::HashMap;

    fn noisy() -> Ty {
        TyKind::Struct("Noisy".into(), Vec::new()).into()
    }

    fn function(var_ty: Ty, instrs: Vec<MirInstr>) -> MirFunction {
        MirFunction {
            blocks: vec![MirBlock {
                instrs,
                term: MirTerm::Return(None),
            }],
            n_vars: 1,
            n_params: 1,
            var_names: vec!["args".into()],
            var_tys: HashMap::from([(0, var_ty)]),
            ..MirFunction::default()
        }
    }

    /// The elements each `drop.place` of `function` drops, and how a
    /// whole slot's pack ends.
    fn shape(function: &MirFunction) -> Vec<String> {
        function.blocks[0]
            .instrs
            .iter()
            .map(|instruction| match instruction {
                MirInstr::DropPlace { place, .. } => format!("drop {:?}", place.proj),
                MirInstr::ConsumeVar { var } => format!("consume ${var}"),
                MirInstr::DropVar { var, .. } => format!("drop ${var}"),
                other => format!("{other:?}"),
            })
            .collect()
    }

    fn runs_no_code(ty: &Ty) -> bool {
        matches!(ty.kind(), TyKind::Int)
    }

    #[test]
    fn owned_collector_drops_its_elements_last_to_first() {
        let pack: Ty = TyKind::Tuple(vec![noisy(), Ty::int(), noisy()]).into();
        let mut f = function(
            pack,
            vec![MirInstr::DropVar {
                var: 0,
                destructor: None,
            }],
        );
        refine_pack_drops(&mut f, Some(0), &runs_no_code);
        assert_eq!(
            shape(&f),
            ["drop [ConstIndex(2)]", "drop [ConstIndex(0)]", "consume $0"]
        );
    }

    #[test]
    fn tuple_storage_drops_first_to_last_and_nested_packs_in_turn() {
        let inner: Ty = TyKind::Tuple(vec![Ty::int(), noisy()]).into();
        let storage: Ty = TyKind::Tuple(vec![noisy(), inner]).into();
        let mut place = MirPlace::root(0, Some(noisy()));
        place.project(Proj::Field("storage".into()), storage);
        let mut f = function(
            noisy(),
            vec![MirInstr::DropPlace {
                place,
                destructor: None,
            }],
        );
        refine_pack_drops(&mut f, None, &runs_no_code);
        assert_eq!(
            shape(&f),
            [
                "drop [Field(\"storage\"), ConstIndex(0)]",
                "drop [Field(\"storage\"), ConstIndex(1), ConstIndex(1)]",
            ]
        );
        assert_eq!(f.n_regs, 0);
    }

    #[test]
    fn spread_of_a_variadic_parameter_waits_for_the_elaborator() {
        let spread: Ty = TyKind::Tuple(vec![
            TyKind::Param {
                binder: ParamRef::unbound("*Ts"),
                bounds: Vec::new(),
                callable_bound: None,
            }
            .into(),
        ])
        .into();
        let mut f = function(
            spread,
            vec![MirInstr::DropVar {
                var: 0,
                destructor: None,
            }],
        );
        refine_pack_drops(&mut f, Some(0), &runs_no_code);
        assert_eq!(shape(&f), ["drop $0"]);
    }
}
