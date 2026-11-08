//! Pack drops refined per element: upstream's `VariadicPack.__deinit__`
//! unrolling a `comptime for` of `destroy_pointee` over the elements.
//!
//! A pack aggregate has no destructor of its own, so a `drop.var` or
//! `drop.place` of one names none. A `Tuple`'s private storage is
//! upstream's `!kgen.struct`, trivially destructible: its drop destroys
//! nothing, since `Tuple.__deinit__` destroys each element itself. The
//! owning heterogeneous collector (`var *args: *Ts`, the `owned_pack`
//! slot) and an elaborated `RuntimePack` destroy their elements, each in
//! its own step in which destructor insertion names that element's
//! `__del__`, the owning collector's last to first. Here such a drop
//! becomes one `drop.place` per element that runs code, which naming then
//! gives its element's destructor, as a field's; a whole slot ends after
//! them with a `consume.var`, which destroys nothing more, and a part pack
//! ends with its last element. A drop whose pack type is still symbolic
//! waits for the elaborator, which refines an instance's once its elements
//! are concrete.

use crate::mir::{MirBlock, MirFunction, MirInstr, MirPlace, Proj, instruction_regions_mut};
use mojito_hir::hir::VarId;
use mojito_types::types::{Ty, TyKind};

/// Refine every drop of a concrete pack aggregate in `function` per element.
///
/// Each `drop.var` and `drop.place`, regions included, of the owning
/// collector `owned_pack` (whose elements die last to first) or of a
/// `RuntimePack` becomes the drops of its elements that run code
/// (`runs_no_code` answering no), a whole slot's end after them. A drop of
/// any other `Tuple` storage destroys no element: only a whole slot ends.
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
        let Some(ty) = place.ty.as_ref() else {
            return;
        };
        let Some(elements) = pack_elements(ty) else {
            return;
        };
        let owned = Some(place.root) == self.owned_pack;
        if !owned && matches!(ty.kind(), TyKind::Tuple(_)) {
            if place.is_whole_root() {
                out.push(MirInstr::ConsumeVar { var: place.root });
            }
            return;
        }
        let mut positions: Vec<usize> = (0..elements.len()).collect();
        if place.is_whole_root() && owned {
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
        ty.is_int()
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
    fn tuple_storage_drop_destroys_no_element() {
        let inner: Ty = TyKind::Tuple(vec![Ty::int(), noisy()]).into();
        let storage: Ty = TyKind::Tuple(vec![noisy(), inner]).into();
        let mut place = MirPlace::root(0, Some(noisy()));
        place.project(Proj::Field("storage".into()), storage.clone());
        let mut f = function(
            noisy(),
            vec![MirInstr::DropPlace {
                place,
                destructor: None,
            }],
        );
        refine_pack_drops(&mut f, None, &runs_no_code);
        assert!(shape(&f).is_empty());
        let mut whole = function(
            storage,
            vec![MirInstr::DropVar {
                var: 0,
                destructor: None,
            }],
        );
        refine_pack_drops(&mut whole, None, &runs_no_code);
        assert_eq!(shape(&whole), ["consume $0"]);
    }

    #[test]
    fn runtime_pack_drops_its_elements_first_to_last() {
        let pack: Ty = TyKind::RuntimePack(vec![noisy(), Ty::int(), noisy()]).into();
        let mut f = function(
            pack,
            vec![MirInstr::DropVar {
                var: 0,
                destructor: None,
            }],
        );
        refine_pack_drops(&mut f, None, &runs_no_code);
        assert_eq!(
            shape(&f),
            ["drop [ConstIndex(0)]", "drop [ConstIndex(2)]", "consume $0"]
        );
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
