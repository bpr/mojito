//! The `VariadicList` a call passes its callee's homogeneous `*xs`
//! collector.
//!
//! Upstream's call addresses each element where it lies, gathers the
//! element pointers in a `!pop.array` in the caller's frame, and passes the
//! `VariadicList` whose `@implicit` constructor wraps that array in a
//! `Span[Pointer[element_type, origin], ImmUntrackedOrigin]`. The call here
//! builds the same values: a pointer per element, their `!pop.array`
//! (`array.create`) stored in a slot of the caller's
//! (`__passed_varargs__`), and the `Span` over its first element and the
//! list over the span, each a `struct.create`, which stands for the inlined
//! constructor.
//!
//! A `read` collector's element is the caller's own place, borrowed, or a
//! value the call binds to a slot of its own; an owned (`var`) collector's
//! every element is such a slot, whose value the list then owns: the slot
//! is marked destroyed once the list is built, which tells the lifetime
//! check that neither the caller's normal nor its raise path destroys it
//! again, and the callee's `VariadicList.__deinit__` destroys the element
//! through its pointer. The marks are inputs to that check, which
//! `lower-lit` erases, so a list built below it leaves them out.

use crate::mir::{Const, MirInstr, MirPlace, Proj, Reg};
use mojito_hir::hir::VarId;
use mojito_types::origin::{Origin, PointerOrigin};
use mojito_types::types::{Ty, TyArg, TyKind, variadic_list_type};

/// One element of the list a call builds.
pub enum ListElement {
    /// A place of the caller's the element is, already addressed: `pointer`
    /// is its `ref.make`, typed [`element_pointer_type`], and `roots` the
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
    TyKind::Pointer {
        element: element.clone(),
        origin: PointerOrigin::Untracked { mutable: owned },
    }
    .into()
}

/// The `VariadicList[element, owned]` over `elements`, in order.
///
/// It is built as the call builds it ([`VariadicListConstruction`]). An
/// empty list's array holds one uninitialized pointer, so its span still
/// addresses the array.
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
    let pointer_ty = element_pointer_type(element, owned);
    let mut before = Vec::new();
    let mut after = Vec::new();
    let mut pointers = Vec::with_capacity(elements.len().max(1));
    let mut ended = Vec::new();
    for value in elements {
        match value {
            ListElement::Borrowed { pointer, roots } => {
                pointers.push(pointer);
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
                let pointer = tables.fresh_register(pointer_ty.clone());
                before.push(MirInstr::MakeRef {
                    dest: pointer,
                    place: MirPlace::root(slot, Some(element.clone())),
                });
                pointers.push(pointer);
                ended.push(slot);
            }
        }
    }
    let length = pointers.len();
    if pointers.is_empty() {
        let placeholder = tables.fresh_register(pointer_ty.clone());
        before.push(MirInstr::Const {
            dest: placeholder,
            k: Const::UninitMem,
        });
        pointers.push(placeholder);
    }
    let array_ty: Ty = TyKind::PopArray {
        size: u64::try_from(pointers.len()).expect("a call's argument count fits a u64"),
        element: pointer_ty.clone(),
    }
    .into();
    let array = tables.fresh_register(array_ty.clone());
    before.push(MirInstr::MakeArray {
        dest: array,
        elems: pointers,
    });
    let array_slot = tables.fresh_slot("__passed_varargs__".to_string(), array_ty.clone());
    before.push(MirInstr::DefVar {
        var: array_slot,
        src: array,
        binding_ty: Some(array_ty.clone()),
        in_place: false,
    });
    let first = tables.fresh_register(
        TyKind::Pointer {
            element: pointer_ty.clone(),
            origin: PointerOrigin::Untracked { mutable: false },
        }
        .into(),
    );
    let mut first_place = MirPlace::root(array_slot, Some(array_ty));
    first_place.project(Proj::ConstIndex(0), pointer_ty.clone());
    before.push(MirInstr::MakeRef {
        dest: first,
        place: first_place,
    });
    let size = tables.fresh_register(Ty::int());
    before.push(MirInstr::Const {
        dest: size,
        k: Const::Int(i64::try_from(length).expect("a call's argument count fits an Int")),
    });
    let span = tables.fresh_register(
        TyKind::Struct(
            mojito_types::types::SPAN_TYPE_NAME.to_string(),
            vec![
                TyArg::Ty(pointer_ty),
                TyArg::Origin(Origin::Untracked { mutable: false }),
            ],
        )
        .into(),
    );
    before.push(MirInstr::MakeStruct {
        dest: span,
        fields: vec![first, size],
    });
    let list = tables.fresh_register(variadic_list_type(element.clone(), owned));
    before.push(MirInstr::MakeStruct {
        dest: list,
        fields: vec![span],
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
