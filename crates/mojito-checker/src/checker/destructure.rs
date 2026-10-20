//! Destructuring assignment: `a, b = t`, `var a, b = t`,
//! `(var a, (ref b, _)) = pair`, nested places. The value is read once and
//! its tuple elements distributed to the target's leaves through one
//! extraction plan ([`CheckedTupleUnpackElement`]), which MIR lowers.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::{BindingMode, DestructureTarget, DestructureTargetKind};
use mojito_checked::checked::CheckedTupleUnpackElement;
use mojito_types::origin::{Mutability, Origin, OriginPlace, RefTy};
use mojito_types::types::TyKind;

impl Checker {
    /// Check `target = value`: the value must be a tuple of the target's
    /// arity at every tuple level, each leaf is declared, written, or
    /// discarded, and the plan recording each element's read is attached to
    /// the value node.
    pub(super) fn check_destructure(
        &mut self,
        target: &DestructureTarget,
        value: &Expr,
    ) -> Result<(), TypeError> {
        let items = match &target.kind {
            DestructureTargetKind::Tuple(items) => items,
            DestructureTargetKind::Sequence(_) => {
                return Err(TypeError::Unsupported(
                    "sequence destructuring targets".into(),
                ));
            }
            DestructureTargetKind::Discard
            | DestructureTargetKind::Declare { .. }
            | DestructureTargetKind::Place(_) => {
                return Err(TypeError::InvariantViolation(
                    "a destructuring target is a tuple or a sequence".into(),
                ));
            }
        };
        let vt = self.infer(value)?;
        // A place is read element by element through the reference it
        // yields; the Tuple's index accessor demands it.
        let place = is_place_expr(value);
        let served = self.tuple_index_accessor(&vt).is_some();
        let source = match place.then(|| self.reference_actual(value)).transpose() {
            Ok(source) => source,
            Err(error) if served => return Err(error),
            Err(_) => None,
        };
        // A temporary `Tuple` lives in a hidden slot for the statement, and
        // its elements are read through references into it, as a place's
        // are. The slot's owner is the root of each element reference the
        // plan records.
        let mut temporary = None;
        let source = match source {
            None if served && !place => {
                let owner = self.fresh_owner()?;
                temporary = Some(owner);
                Some(RefTy {
                    referent: vt.clone(),
                    origin: Origin::Place(OriginPlace {
                        root: owner,
                        path: Vec::new(),
                    }),
                    mutability: Mutability::Immutable,
                })
            }
            source => source,
        };
        let mut plan = self.destructure_elements(items, &vt, source.as_ref(), value)?;
        if let Some(owner) = temporary {
            for element in &mut plan {
                element.temporary = Some(owner);
            }
        }
        self.record_at(value.source_span()).tuple_unpack_plan = Some(plan);
        Ok(())
    }

    /// The plan for the elements of a tuple-typed value `vt`, read through
    /// `source` when it has storage, with each of `items` applied to its
    /// element. `value` is the expression the tuple came from, for the
    /// outward-store check.
    fn destructure_elements(
        &mut self,
        items: &[DestructureTarget],
        vt: &Ty,
        source: Option<&RefTy>,
        value: &Expr,
    ) -> Result<Vec<CheckedTupleUnpackElement>, TypeError> {
        let Some(elems) = tuple_elements(vt) else {
            return Err(TypeError::TypeMismatch {
                expected: "a tuple".to_string(),
                found: vt.to_string(),
                context: "tuple unpacking".to_string(),
            });
        };
        if elems.len() != items.len() {
            return Err(TypeError::TypeMismatch {
                expected: format!("a {}-element tuple", items.len()),
                found: vt.to_string(),
                context: "tuple unpacking".to_string(),
            });
        }
        let elems = elems.into_iter().cloned().collect::<Vec<_>>();
        let mut plan = self.tuple_unpack_plan(vt, source)?;
        for (index, (item, (element, elem))) in
            items.iter().zip(plan.iter_mut().zip(&elems)).enumerate()
        {
            // The element's own expression, for the outward-store twin: the
            // display's item when the value spells one, else the value.
            let element_value = match &value.kind {
                ExprKind::TupleLit(values) if values.len() == items.len() => &values[index],
                _ => value,
            };
            match &item.kind {
                // `_` discards its element (no binding, no read).
                DestructureTargetKind::Discard => {}
                DestructureTargetKind::Tuple(inner) => {
                    // A nested tuple is read through the element's reference
                    // (a place's or the temporary's) when it has one.
                    element.elements = self.destructure_elements(
                        inner,
                        &element.ty,
                        element.reference.as_ref(),
                        element_value,
                    )?;
                }
                DestructureTargetKind::Declare {
                    name,
                    mode: BindingMode::Var,
                } => {
                    self.require_element_copy(element)?;
                    let declared = self.inferred_binding_ty(elem, name)?;
                    self.declare(name, declared)?;
                    self.record_declared_leaf(item, name);
                }
                DestructureTargetKind::Declare {
                    name,
                    mode: BindingMode::Ref,
                } => {
                    let Some(reference) = element.reference.clone() else {
                        return Err(TypeError::Unsupported(format!(
                            "a 'ref' destructuring target ('{name}') over a value without \
                             tuple storage"
                        )));
                    };
                    let mutable = reference.mutability == Mutability::Mutable;
                    let binding_ty = Ty::new(TyKind::Ref(reference));
                    self.declare_with_mutability(name, binding_ty, mutable)?;
                    self.record_declared_leaf(item, name);
                }
                DestructureTargetKind::Declare {
                    name,
                    mode: BindingMode::Immutable,
                } => {
                    return Err(TypeError::InvariantViolation(format!(
                        "destructuring leaf '{name}' declares without a convention"
                    )));
                }
                DestructureTargetKind::Place(place) => {
                    self.destructure_into_place(place, element, elem, element_value)?;
                }
                DestructureTargetKind::Sequence(_) => {
                    return Err(TypeError::Unsupported(
                        "sequence destructuring targets".into(),
                    ));
                }
            }
        }
        Ok(plan)
    }

    /// A named target owns a copy of the element a place keeps, so the
    /// element must copy implicitly.
    fn require_element_copy(&self, element: &CheckedTupleUnpackElement) -> Result<(), TypeError> {
        if element.reference.is_none() {
            return Ok(());
        }
        let context = "unpacking a Tuple place copies each element".to_string();
        if !self.is_copyable(&element.ty) {
            return Err(TypeError::NonCopyable {
                ty: element.ty.to_string(),
                context,
            });
        }
        if !self.is_implicitly_copyable(&element.ty) {
            return Err(TypeError::ImplicitCopy {
                ty: element.ty.to_string(),
                context,
                transferable: false,
                copyable: true,
            });
        }
        Ok(())
    }

    /// Record a declared leaf's binding and storage type at the leaf's own
    /// occurrence, which the checked target carries for MIR.
    fn record_declared_leaf(&self, leaf: &DestructureTarget, name: &str) {
        if let Some(owner) = self.lookup_owner(name) {
            self.record_at(leaf.source_span()).binding = Some(owner);
        }
        if let Some(storage_ty) = self.lookup(name).cloned() {
            self.record_at(leaf.source_span()).binding_ty = Some(storage_ty);
        }
    }

    /// Write element `elem` to `place`: a name re-assigns the binding in
    /// scope (keeping its type), any other place is written through the
    /// ordinary place rules.
    fn destructure_into_place(
        &self,
        place: &Expr,
        element: &CheckedTupleUnpackElement,
        elem: &Ty,
        element_value: &Expr,
    ) -> Result<(), TypeError> {
        if let ExprKind::Identifier(name) = &place.kind {
            return self.destructure_into_name(place, name, element, elem);
        }
        if let Some(root) = place_root_name(place) {
            self.check_capture_access(root, true)?;
        }
        // A nominal subscript is written through `__setitem__`, which needs
        // the element as an expression to select; the destructured element
        // has none yet.
        if let ExprKind::Index { object, index } = &place.kind
            && !self.type_keyed_projection(object, index)
            && matches!(self.infer(object)?.kind(), TyKind::Struct(name, _) if self.structs.contains_key(name))
        {
            return Err(TypeError::Unsupported(
                "a nominal subscript (`xs[i]`) as a destructuring target".into(),
            ));
        }
        let target_ty = self.check_place(place)?;
        self.record_place_write_invalidation(place.source_span(), place);
        if !coerces(elem, &target_ty) {
            if let Some(mismatch) = self.struct_origin_mismatch(elem, &target_ty) {
                return Err(mismatch);
            }
            return Err(TypeError::TypeMismatch {
                expected: target_ty.to_string(),
                found: elem.to_string(),
                context: "unpacking into a place".to_string(),
            });
        }
        // Store-outward twin for unpack-into-place. A loan-carrying element
        // cannot currently reach here (an rvalue unpack demands implicitly
        // copyable elements and place targets type-match exactly), so this is
        // defensive coverage that keeps the boundary closed if those
        // restrictions loosen.
        let storage = Some(target_ty);
        self.check_outward_store(place, element_value, elem, &storage)
    }

    /// Write element `elem` to the binding `name` in scope, which keeps its
    /// type; a `ref` binding's referent is written through it.
    fn destructure_into_name(
        &self,
        place: &Expr,
        name: &str,
        element: &CheckedTupleUnpackElement,
        elem: &Ty,
    ) -> Result<(), TypeError> {
        self.require_element_copy(element)?;
        let Some(existing) = self.lookup(name).cloned() else {
            return Err(TypeError::AssignToUndeclared(name.to_string()));
        };
        self.check_capture_access(name, true)?;
        if !self.is_binding_mutable(name) {
            return Err(TypeError::ImmutableBinding(name.to_string()));
        }
        if !coerces(elem, &existing) {
            return Err(TypeError::TypeMismatch {
                expected: existing.to_string(),
                found: elem.to_string(),
                context: format!("unpacking into '{name}'"),
            });
        }
        if matches!(existing.kind(), TyKind::Ref(_)) {
            self.record_place_write_invalidation(place.source_span(), place);
        } else if let Some(owner) = self.lookup_owner(name) {
            self.record_owner_invalidation(place.source_span(), owner, Vec::new());
        }
        if let Some(owner) = self.lookup_owner(name) {
            self.record_at(place.source_span()).binding = Some(owner);
            // A name target is binder/place syntax, not an ordinary inferred
            // expression, so its complete slot metadata is retained
            // explicitly for checked MIR verification and for a nested
            // closure that captures the introduction.
            let value_ty = match existing.kind() {
                TyKind::Ref(reference) => reference.referent.clone(),
                _ => existing.clone(),
            };
            self.record_at(place.source_span()).ty = Some(value_ty);
            self.record_at(place.source_span()).place_ty = Some(existing.clone());
            self.record_at(place.source_span()).binding_ty = Some(existing);
        }
        Ok(())
    }
}
