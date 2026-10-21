//! Destructuring assignment: `a, b = t`, `var a, b = t`,
//! `(var a, (ref b, _)) = pair`, `var [a, b] = values`, nested places.
//!
//! A tuple target reads the value once and distributes its tuple elements
//! to the target's leaves through one extraction plan
//! ([`CheckedTupleUnpackElement`]), which MIR lowers.
//!
//! A sequence target is upstream's list-pattern store, a straight line of
//! ordinary calls and stores, so it is a checked desugar (`desugar.rs`)
//! rather than a plan: the value is anchored once
//! (`ref $seq<id> = place` made immutable, or `var $seq<id> = value`), its
//! type must declare `Sized`, the raising
//! `check_list_length($seq<id>, n, True)` runs before any store, and then
//! each element is read through `$seq<id>.__getitem__(i)` and stored into
//! its target in order, the target's own subexpressions evaluating after
//! that read. `_` still reads its element, an owned element moves into a
//! `var` leaf, and a nested tuple or sequence target destructures the
//! element read into a hidden `$elem<id>`.
//!
//! A star `[t0, *ts, t2, t3]` makes the check a minimum
//! (`check_list_length($seq<id>, 3, False)`). A suffix after it starts at
//! `var $suffix<id> = $seq<id>.__len__() - 2`, read once; the star's target
//! takes the subscript `$seq<id>[1:$suffix<id>]` (`$seq<id>[1:]` when the
//! star is last), `_` included, and the suffix's elements are read at
//! `$suffix<id>` and `$suffix<id> + k`.

use super::desugar::{StatementDesugar, Synth};
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::{BindingMode, DestructureTarget, DestructureTargetKind, SequenceTarget};
use mojito_checked::checked::CheckedTupleUnpackElement;
use mojito_types::origin::{Mutability, Origin, OriginPlace, RefTy};
use mojito_types::types::TyKind;

impl Checker {
    /// Check the statement `stmt`, `target = value`. For a tuple target the
    /// value must be a tuple of the target's arity at every tuple level,
    /// each leaf is declared, written, or discarded, and the plan recording
    /// each element's read is attached to the value node; a sequence target
    /// is checked as its desugar (see the module docs).
    pub(super) fn check_destructure(
        &mut self,
        stmt: &Stmt,
        target: &DestructureTarget,
        value: &Expr,
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<(), TypeError> {
        let items = match &target.kind {
            DestructureTargetKind::Tuple(items) => items,
            DestructureTargetKind::Sequence(items) => {
                return self.check_sequence_destructure(stmt, items, value, ret, in_loop);
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

    /// Check `[t0, t1, ...] = value`, the statement `stmt`, as upstream's
    /// list-pattern store, and record the statements it desugars to: the
    /// anchor, the raising length check, then each element's read and
    /// store, each checked in the current scope so the leaves outlive it.
    fn check_sequence_destructure(
        &mut self,
        stmt: &Stmt,
        items: &[SequenceTarget],
        value: &Expr,
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<(), TypeError> {
        let count = items.len();
        let star = items
            .iter()
            .position(|item| matches!(item, SequenceTarget::Star(_)));
        let synth = Synth::for_statement(stmt, value.source.clone());
        let anchor_name = format!("$seq{}", synth.parent().0);
        let (anchor, ty, _) =
            self.anchor_subject(&synth, &anchor_name, value, true, ret, in_loop)?;
        if matches!(
            ty.kind(),
            TyKind::Int | TyKind::UInt | TyKind::Float64 | TyKind::Simd { .. }
        ) {
            return Err(TypeError::Unsupported(format!(
                "sequence destructuring of '{ty}', which has no '__len__'/'__getitem__' yet"
            )));
        }
        self.require_sized(&ty)?;
        let check_list_length = builtin_stub("check_list_length");
        if self.lookup(&check_list_length).is_none() {
            return Err(TypeError::Unsupported(format!(
                "sequence destructuring needs the bundled '{STUBS_MODULE}' module"
            )));
        }
        if !self.raising_allowed() {
            return Err(TypeError::UnhandledRaise(
                "a sequence destructuring assignment".into(),
            ));
        }
        let length_check = synth.stmt(StmtKind::Expr(synth.expr(ExprKind::Call {
            name: check_list_length,
            param_args: Vec::new(),
            args: vec![
                synth.identifier(&anchor_name),
                synth.int(count - usize::from(star.is_some())),
                synth.expr(ExprKind::Bool(star.is_none())),
            ],
            kwargs: Vec::new(),
        })));
        self.check_stmt(&length_check, ret, in_loop)?;
        let mut statements = vec![anchor, length_check];
        // A star before a suffix reads the length once more, for the
        // suffix's start: `var $suffix<id> = $seq<id>.__len__() - k`.
        let suffix = match star {
            Some(star) if star + 1 != count => {
                let name = format!("$suffix{}", synth.parent().0);
                let length = synth.expr(ExprKind::MethodCall {
                    object: Box::new(synth.identifier(&anchor_name)),
                    method: "__len__".to_string(),
                    args: Vec::new(),
                    kwargs: Vec::new(),
                });
                let declaration = synth.stmt(StmtKind::VarDecl {
                    name: name.clone(),
                    ty: None,
                    value: synth.expr(ExprKind::Infix(
                        InfixOp::Sub,
                        Box::new(length),
                        Box::new(synth.int(count - star - 1)),
                    )),
                });
                self.check_stmt(&declaration, ret, in_loop)?;
                statements.push(declaration);
                Some((star + 1, name))
            }
            _ => None,
        };
        for (index, item) in items.iter().enumerate() {
            let anchor = Box::new(synth.identifier(&anchor_name));
            let (read, target) = match (item, &suffix) {
                (SequenceTarget::Star(target), _) => (
                    synth.expr(ExprKind::Slice {
                        object: anchor,
                        lower: Some(Box::new(synth.int(index))),
                        upper: suffix
                            .as_ref()
                            .map(|(_, name)| Box::new(synth.identifier(name))),
                        step: None,
                    }),
                    target,
                ),
                (SequenceTarget::Element(target), Some((first, name))) if index >= *first => {
                    let start = synth.identifier(name);
                    let position = match index - first {
                        0 => start,
                        offset => synth.expr(ExprKind::Infix(
                            InfixOp::Add,
                            Box::new(start),
                            Box::new(synth.int(offset)),
                        )),
                    };
                    (getitem(&synth, anchor, position), target)
                }
                (SequenceTarget::Element(target), _) => {
                    (getitem(&synth, anchor, synth.int(index)), target)
                }
            };
            self.store_sequence_element(&synth, target, read, &mut statements, ret, in_loop)?;
        }
        self.statement_desugars
            .borrow_mut()
            .insert(stmt.source_span(), StatementDesugar { statements });
        Ok(())
    }

    /// Store the element `read` reads into the sequence target `item`,
    /// appending the checked statements that do it to `statements`: `_`
    /// discards the read, a `var` leaf declares it, a `ref` leaf binds it to
    /// read, a name is assigned it, and any other place, tuple, or sequence
    /// target takes the read through a hidden `$elem<id>` first, since its
    /// own subexpressions evaluate after the read.
    fn store_sequence_element(
        &mut self,
        synth: &Synth,
        item: &DestructureTarget,
        read: Expr,
        statements: &mut Vec<Stmt>,
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<(), TypeError> {
        let read_span = read.source_span();
        let store = match &item.kind {
            DestructureTargetKind::Discard => StmtKind::Assign {
                name: "_".to_string(),
                value: read,
            },
            DestructureTargetKind::Declare {
                name,
                mode: BindingMode::Var,
            } => StmtKind::VarDecl {
                name: name.clone(),
                ty: None,
                value: read,
            },
            DestructureTargetKind::Declare {
                name,
                mode: BindingMode::Ref,
            } => {
                let declaration = synth.stmt(StmtKind::RefDecl {
                    name: name.clone(),
                    value: read,
                });
                self.check_stmt(&declaration, ret, in_loop)?;
                self.make_reference_immutable(name, &declaration, read_span);
                statements.push(declaration);
                return Ok(());
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
                if let ExprKind::Identifier(name) = &place.kind {
                    if self.lookup(name).is_none() {
                        return Err(TypeError::AssignToUndeclared(name.clone()));
                    }
                    StmtKind::Assign {
                        name: name.clone(),
                        value: read,
                    }
                } else {
                    self.require_mutable_subscript(place)?;
                    let (element, owned) =
                        self.read_into_hidden(synth, read, statements, ret, in_loop)?;
                    let element = if owned {
                        synth.expr(ExprKind::Transfer(Box::new(element)))
                    } else {
                        element
                    };
                    StmtKind::SetPlace {
                        place: place.clone(),
                        value: element,
                    }
                }
            }
            DestructureTargetKind::Tuple(_) | DestructureTargetKind::Sequence(_) => {
                let (element, _) = self.read_into_hidden(synth, read, statements, ret, in_loop)?;
                StmtKind::Destructure {
                    target: item.clone(),
                    value: element,
                }
            }
        };
        let store = synth.stmt(store);
        self.check_stmt(&store, ret, in_loop)?;
        statements.push(store);
        Ok(())
    }

    /// A nominal subscript target (`s[i]`) must name a mutable place, a
    /// getter's mutable reference: upstream stores into the subscript's
    /// reference and never calls `__setitem__`.
    fn require_mutable_subscript(&self, place: &Expr) -> Result<(), TypeError> {
        let ExprKind::Index { object, index } = &place.kind else {
            return Ok(());
        };
        if self.type_keyed_projection(object, index)
            || !matches!(self.infer(object)?.kind(), TyKind::Struct(name, _) if self.structs.contains_key(name))
        {
            return Ok(());
        }
        self.infer(place)?;
        if self
            .infer_reference_value(place)
            .is_some_and(|reference| reference.mutability == Mutability::Mutable)
        {
            return Ok(());
        }
        Err(TypeError::NotMutableInCollectionLiteral(
            place_root_name(place).map_or_else(
                || "a subscript".to_string(),
                |root| format!("a subscript of '{root}'"),
            ),
        ))
    }

    /// Read `read` into a hidden `$elem<id>`, appending its checked
    /// declaration to `statements`: `ref` (to read) over a reference
    /// result, `var` over an owned one. The hidden name and whether the
    /// read is owned.
    fn read_into_hidden(
        &mut self,
        synth: &Synth,
        read: Expr,
        statements: &mut Vec<Stmt>,
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<(Expr, bool), TypeError> {
        let read_span = read.source_span();
        let name = format!("$elem{}", read.syntax_id.0);
        self.infer(&read)?;
        let reference = self.infer_reference_value(&read).is_some();
        let declaration = synth.stmt(if reference {
            StmtKind::RefDecl {
                name: name.clone(),
                value: read,
            }
        } else {
            StmtKind::VarDecl {
                name: name.clone(),
                ty: None,
                value: read,
            }
        });
        self.check_stmt(&declaration, ret, in_loop)?;
        if reference {
            self.make_reference_immutable(&name, &declaration, read_span);
        }
        statements.push(declaration);
        Ok((synth.identifier(&name), !reference))
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
                        "a sequence target inside a tuple target".into(),
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

/// `object.__getitem__(index)`, the attribute call upstream's store makes
/// for an element, made by `synth`.
fn getitem(synth: &Synth, object: Box<Expr>, index: Expr) -> Expr {
    synth.expr(ExprKind::MethodCall {
        object,
        method: "__getitem__".to_string(),
        args: vec![index],
        kwargs: Vec::new(),
    })
}
