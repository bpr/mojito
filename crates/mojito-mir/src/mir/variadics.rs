//! The collectors a call builds for its callee's positional `*xs`: the
//! `VariadicList` of a homogeneous one and the `VariadicPack` of a
//! heterogeneous one, after upstream's `std/builtin/variadics.mojo`.
//!
//! For a homogeneous `*xs: T`, upstream's call addresses each element where
//! it lies, gathers the element references in a `!pop.array` in the
//! caller's frame, and passes the `VariadicList` its `@implicit`
//! constructor builds from that array's `!lit.ref`, a `Span[Pointer[
//! element_type, origin], ImmUntrackedOrigin]` over the array. The call
//! here does the same: a `!lit.ref` per element, their `!pop.array`
//! (`array.create`) stored in a slot of the caller's
//! (`__passed_varargs__`), and a `call` of the constructor on the slot's
//! `!lit.ref`.
//!
//! For a `read` or `mut` heterogeneous `*args: *Ts`, upstream's call
//! addresses each element and gathers the references in a
//! `!lit.ref.pack` (`lit.ref.pack.create`), which `VariadicPack.__init__`
//! wraps; the call here builds the pack and the `VariadicPack` over it, a
//! `struct.create` standing for that constructor ([`build_variadic_pack`]).
//!
//! A `read` collector's element is the caller's own place, borrowed, or a
//! value the call binds to a slot of its own; an owned (`var`) list's
//! every element is such a slot, whose value the list then owns: the slot
//! is marked destroyed once the list is built, which tells the lifetime
//! check that neither the caller's normal nor its raise path destroys it
//! again, and the callee's `VariadicList.__deinit__` destroys the element
//! through its pointer. The marks are inputs to that check, which
//! `lower-lit` erases, so a list built below it leaves them out. A
//! `VariadicPack`'s value slot is a temporary the caller destroys after
//! the call, as upstream's caller destroys an rvalue element.

use crate::mir::{FuncRef, MirInstr, MirPlace, Reg};
use mojito_hir::hir::VarId;
use mojito_types::ct::CtValue;
use mojito_types::origin::PointerOrigin;
use mojito_types::types::{Ty, TyArg, TyKind, variadic_list_type};

/// One element of the list a call builds.
pub enum ListElement {
    /// A place of the caller's the element is, already addressed: `pointer`
    /// is its `ref.make`, typed [`element_lit_ref_type`] for a list and
    /// [`element_pointer_type`] for a pack, and `roots` the
    /// slots the place lies in, which outlive the call.
    Borrowed { pointer: Reg, roots: Vec<VarId> },
    /// A value the call holds, bound to a fresh slot of the caller's.
    Value(Reg),
}

/// The register and slot tables a list's construction extends.
pub trait FrameTables {
    /// A fresh register of type `ty`.
    fn fresh_register(&mut self, ty: Ty) -> Reg;
    /// A fresh slot named `name`, of type `ty`.
    fn fresh_slot(&mut self, name: String, ty: Ty) -> VarId;
}

/// A built list, and what runs around the call passing it.
///
/// `before` runs ahead of the call, `list` is the register the call passes,
/// `ended` marks each slot of an owned list's elements destroyed once the
/// list is built (only [`MirInstr::MarkDestroyed`]s), and `after` follows
/// the call, keeping the borrowed elements, last to first, and the array
/// live through it (only [`MirInstr::KeepAlive`]s). Concrete MIR holds
/// neither kind of marker.
pub struct VariadicListConstruction {
    pub before: Vec<MirInstr>,
    pub list: Reg,
    pub ended: Vec<MirInstr>,
    pub after: Vec<MirInstr>,
}

/// The type of one element pointer of a list of `element`s: upstream's
/// `Pointer[element_type, origin]`, its origin untracked, mutable for an
/// owned list.
pub fn element_pointer_type(element: &Ty, owned: bool) -> Ty {
    Ty::pointer(
        element.clone(),
        PointerOrigin::Untracked { mutable: owned },
        mojito_types::types::AddressSpaceSlot::GENERIC,
    )
}

/// The type of one element reference a list of `element`s gathers in its
/// `!pop.array`: upstream's `Pointer[element_type, origin]._mlir_lit_ref`,
/// its origin untracked, mutable for an owned list.
pub fn element_lit_ref_type(element: &Ty, owned: bool) -> Ty {
    Ty::lit_ref(
        element.clone(),
        PointerOrigin::Untracked { mutable: owned },
        mojito_types::types::AddressSpaceSlot::GENERIC,
    )
}

/// The `VariadicList[element, owned]` over `elements`, in order.
///
/// It is built as the call builds it ([`VariadicListConstruction`]): the
/// element references' `!pop.array<size, …>`, empty for an empty call, in
/// the `__passed_varargs__` slot, and the list's `@implicit` constructor
/// called on the slot's `!lit.ref`.
///
/// # Panics
///
/// When the elements outnumber an `Int`.
pub fn build_variadic_list(
    tables: &mut impl FrameTables,
    element: &Ty,
    owned: bool,
    elements: Vec<ListElement>,
) -> VariadicListConstruction {
    let reference_ty = element_lit_ref_type(element, owned);
    let mut before = Vec::new();
    let mut after = Vec::new();
    let mut references = Vec::with_capacity(elements.len());
    let mut ended = Vec::new();
    for value in elements {
        match value {
            ListElement::Borrowed { pointer, roots } => {
                references.push(pointer);
                after.extend(roots.into_iter().map(|var| MirInstr::KeepAlive { var }));
            }
            ListElement::Value(value) => {
                let slot =
                    tables.fresh_slot(format!("$list_element_r{}", value.0), element.clone());
                before.push(MirInstr::DefVar {
                    var: slot,
                    src: value,
                    binding_ty: Some(element.clone()),
                    in_place: false,
                });
                let reference = tables.fresh_register(reference_ty.clone());
                before.push(MirInstr::MakeRef {
                    dest: reference,
                    place: MirPlace::root(slot, Some(element.clone())),
                });
                references.push(reference);
                ended.push(slot);
            }
        }
    }
    let length = references.len();
    let array_ty: Ty = TyKind::PopArray {
        size: mojito_types::types::ArraySize::Known(
            u64::try_from(length).expect("a call's argument count fits a u64"),
        ),
        element: reference_ty,
    }
    .into();
    let array = tables.fresh_register(array_ty.clone());
    before.push(MirInstr::MakeArray {
        dest: array,
        elems: references,
    });
    let array_slot = tables.fresh_slot("__passed_varargs__".to_string(), array_ty.clone());
    before.push(MirInstr::DefVar {
        var: array_slot,
        src: array,
        binding_ty: Some(array_ty.clone()),
        in_place: false,
    });
    let array_reference = tables.fresh_register(Ty::lit_ref(
        array_ty.clone(),
        PointerOrigin::Untracked { mutable: false },
        mojito_types::types::AddressSpaceSlot::GENERIC,
    ));
    before.push(MirInstr::MakeRef {
        dest: array_reference,
        place: MirPlace::root(array_slot, Some(array_ty)),
    });
    let list = tables.fresh_register(variadic_list_type(
        element.clone(),
        owned,
        owned,
        mojito_types::origin::Origin::Unbound,
    ));
    before.push(MirInstr::Call {
        conventions: crate::mir::MirArgConventions::default(),
        dest: list,
        func: FuncRef::named(&mojito_symbol::symbol::variadic_list_constructor_symbol()),
        raises: None,
        args: vec![array_reference],
        kwargs: Vec::new(),
        capture_accesses: Vec::new(),
        param_arg_regs: Vec::new(),
        receiver: None,
        // The constructor's whole declared list: the list's
        // `elt_is_mutable`, `origin`, `element_type`, and `is_owned`, then
        // its own `size` and the array's `container_origin`.
        instantiated_args: vec![
            TyArg::Val(CtValue::Bool(owned)),
            TyArg::Origin(mojito_types::origin::Origin::Unbound),
            TyArg::Ty(element.clone()),
            TyArg::Val(CtValue::Bool(owned)),
            TyArg::Val(CtValue::Index(
                i64::try_from(length).expect("a call's argument count fits an Int"),
            )),
            TyArg::Origin(mojito_types::origin::Origin::Untracked { mutable: false }),
        ],
        spread: None,
    });
    let ended = ended
        .into_iter()
        .map(|slot| MirInstr::MarkDestroyed {
            place: MirPlace::root(slot, Some(element.clone())),
        })
        .collect();
    // The borrowed elements die last to first, as the pin destroys a call's
    // argument temporaries.
    after.reverse();
    after.push(MirInstr::KeepAlive { var: array_slot });
    VariadicListConstruction {
        before,
        list,
        ended,
        after,
    }
}

/// The `VariadicPack` of type `pack_ty` over `elements`, whose types are
/// `element_types`, in order, built as the call builds it.
///
/// The construction ([`VariadicListConstruction`]) is a reference to each
/// element, the `!lit.ref.pack` over them (`lit.ref.pack.create`), mutable
/// for a `mut` collector, and the pack over that.
///
/// What the elements refer to lives through the call: each of the `kept`
/// places (where a borrowed element lies, or the owner a value element was
/// read out of) is referenced again after it, and a value element's slot is
/// kept alive, latest-declared root first, as at any one instruction. The
/// slot of a value among the `shallow` ones, read out of storage its owner
/// keeps, holds no value of its own and is `ended` instead.
pub fn build_variadic_pack(
    tables: &mut impl FrameTables,
    pack_ty: Ty,
    element_types: &[Ty],
    mutable: bool,
    elements: Vec<ListElement>,
    (kept, shallow): (Vec<MirPlace>, Vec<Reg>),
) -> VariadicListConstruction {
    let mut before = Vec::new();
    let mut ended = Vec::new();
    let mut slots = Vec::new();
    let mut references = Vec::with_capacity(elements.len());
    for (value, element) in elements.into_iter().zip(element_types) {
        match value {
            ListElement::Borrowed { pointer, .. } => references.push(pointer),
            ListElement::Value(value) => {
                let slot =
                    tables.fresh_slot(format!("$pack_element_r{}", value.0), element.clone());
                before.push(MirInstr::DefVar {
                    var: slot,
                    src: value,
                    binding_ty: Some(element.clone()),
                    in_place: false,
                });
                let pointer = tables.fresh_register(element_pointer_type(element, mutable));
                before.push(MirInstr::MakeRef {
                    dest: pointer,
                    place: MirPlace::root(slot, Some(element.clone())),
                });
                references.push(pointer);
                if shallow.contains(&value) {
                    ended.push(MirInstr::MarkDestroyed {
                        place: MirPlace::root(slot, Some(element.clone())),
                    });
                } else {
                    slots.push(slot);
                }
            }
        }
    }
    let refs = tables.fresh_register(
        TyKind::LitRefPack {
            elements: element_types.to_vec(),
            origin: PointerOrigin::Untracked { mutable },
        }
        .into(),
    );
    before.push(MirInstr::MakeRefPack {
        dest: refs,
        elems: references,
    });
    let pack = tables.fresh_register(pack_ty);
    before.push(MirInstr::MakeStruct {
        dest: pack,
        fields: vec![refs],
    });
    // A whole slot is kept alive; a part of one is referenced again, so
    // its other parts die where they are last used.
    let mut lived: Vec<(VarId, Option<MirPlace>)> = kept
        .into_iter()
        .map(|place| {
            let part = !place.proj.is_empty();
            (place.root, part.then_some(place))
        })
        .chain(slots.into_iter().map(|slot| (slot, None)))
        .collect();
    lived.sort_by_key(|(var, _)| std::cmp::Reverse(*var));
    lived.dedup_by(|left, right| left.0 == right.0 && left.1.is_none() && right.1.is_none());
    let after = lived
        .into_iter()
        .map(|(var, place)| match place {
            Some(place) => {
                let referent = place.ty.clone().unwrap_or_else(Ty::none);
                MirInstr::MakeRef {
                    dest: tables.fresh_register(element_pointer_type(&referent, false)),
                    place,
                }
            }
            None => MirInstr::KeepAlive { var },
        })
        .collect();
    VariadicListConstruction {
        before,
        list: pack,
        ended,
        after,
    }
}
