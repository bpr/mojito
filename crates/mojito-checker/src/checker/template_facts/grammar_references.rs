//! Certificate grammar for references and pointers: reference results,
//! reference calls, `ref` locals, and pointers into owned storage.

use super::{BodyShape, fact_at};
use mojito_ast::ast::{Expr, ExprKind, Stmt};
use mojito_checked::templates::{CheckedBodyFacts, MethodFeatures, OccurrenceId};
use mojito_types::types::Ty;

impl BodyShape<'_> {
    /// The value of a `return` in a method that returns a reference: a field
    /// of `self`, a pointer slot, a `ref` local, a `mut` or `ref` parameter, a
    /// reference a call on a field yields, or a field read through such a
    /// reference, of exactly the declared referent type, so neither check
    /// converts it.
    ///
    /// The `return` keeps the place as a handle because the declaration
    /// returns a reference, whatever the place's type, and demands neither a
    /// copy nor a move of it. Whether the place lies within the declared
    /// origin is judged on its path and the signature, which no instance
    /// changes.
    pub(super) fn returned_place(&self, value: &Expr) -> bool {
        let id = self.occurrence(value);
        let forwarded = matches!(&value.kind, ExprKind::Identifier(name)
            if self.reference_local(name) || self.borrowed_params.contains(&name.as_str()));
        let admitted = (forwarded
            || self.receiver_field(value)
            || self.slot(value)
            || self.reference_call(value)
            || self.pack_accessor(value)
            || self.reference_member(value)
            || self.pack_element(value))
            && self
                .reference_result
                .is_some_and(|referent| self.typed(value, referent))
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.reference_value_uses, id) == Some(&false)
                    && !facts.copy_place_value_uses.contains(&id)
            });
        if admitted {
            self.handle(id);
        }
        admitted && self.holds(MethodFeatures::REFERENCE_RESULT)
    }

    /// A reference-returning call on a field of `self` or of a parameter
    /// holding a struct (`other.items[i]`), passing scalars:
    /// a subscript or a named accessor whose recorded contract is a
    /// `closed_reference_contract`. It is admitted as a returned place, a
    /// whole value read, a `ref` declaration's value, or the reference a
    /// field is read or a method called through ([`Self::through`]), or an
    /// argument ([`Self::reference_argument`]), never as an operand.
    /// An iterator's `__next__` marks its copyable read by another rule.
    pub(super) fn reference_call(&self, expr: &Expr) -> bool {
        let (object, method, arguments) = match &expr.kind {
            ExprKind::Index { object, index } => {
                (object, "__getitem__", std::slice::from_ref(&**index))
            }
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } if kwargs.is_empty() && method != "__next__" => {
                (object, method.as_str(), args.as_slice())
            }
            _ => return false,
        };
        let on_self =
            self.receiver && matches!(&object.kind, ExprKind::Identifier(name) if name == "self");
        let admitted = !self.keyed
            && (self.receiver_field(object)
                || on_self
                || self.value_local(object)
                || self.parameter_field(object))
            && arguments
                .iter()
                .all(|argument| self.expression(argument) && self.scalar(argument))
            && self.facts.is_none_or(|facts| {
                self.named_contract(facts, expr, object, method)
                    .is_some_and(mojito_checked::templates::closed_reference_contract)
            });
        if admitted {
            self.references.borrow_mut().push(self.occurrence(expr));
            self.borrowed_field_receiver(object);
        }
        admitted && self.holds(MethodFeatures::REFERENCE_CALLS)
    }

    /// A reference call read by value, which the template marked a copyable
    /// read: a referent that is not implicitly copyable records nothing
    /// there, and its clone check would refuse the read.
    pub(super) fn reference_read(&self, expr: &Expr) -> bool {
        let id = self.occurrence(expr);
        (self.reference_call(expr) || self.module_reference_call(expr))
            && self
                .facts
                .is_none_or(|facts| facts.copyable_reference_result_reads.contains(&id))
    }

    /// Whether the references the check recorded are exactly the ones the
    /// grammar admitted: each handle kept at a returned place, a `ref`
    /// declaration's value, or the base of a field read through a reference,
    /// each receiver borrowed through a reference, and each reference result,
    /// interior generation, and copyable read at an admitted reference call.
    /// Every other writer of those tables decides on a type or on a
    /// binding's declaration. Likewise each subscript descriptor sits at a
    /// subscript admitted as a store's base, and each kept call place at an
    /// argument admitted as one.
    pub(super) fn references_recorded(&self, facts: &CheckedBodyFacts) -> bool {
        let handles = self.handles.borrow();
        let references = self.references.borrow();
        let receivers = self.receivers.borrow();
        let subscripts = self.subscripts.borrow();
        let places = self.places.borrow();
        let copied_writes = self.copied_writes.borrow();
        facts.subscript_descriptors.len() == subscripts.len()
            && facts
                .subscript_descriptors
                .iter()
                .all(|(id, _)| subscripts.contains(id))
            && facts.call_place_uses.len() == places.len()
            && facts.call_place_uses.iter().all(|id| places.contains(id))
            && facts.borrowed_reference_receivers.len() == receivers.len()
            && facts
                .borrowed_reference_receivers
                .iter()
                .all(|id| receivers.contains(id))
            && facts.reference_value_uses.len() == handles.len()
            && facts
                .reference_value_uses
                .iter()
                .all(|(id, _)| handles.contains(id))
            && references
                .iter()
                .all(|id| fact_at(&facts.reference_results, *id).is_some())
            && facts
                .reference_results
                .iter()
                .map(|(id, _)| id)
                .chain(facts.interior_references.iter().map(|(id, _)| id))
                .chain(&facts.copyable_reference_result_reads)
                .all(|id| references.contains(id) || copied_writes.contains(id))
    }

    /// The place a `ref` declaration binds: `self`, a field of it, a
    /// parameter, a `var` local, or a reference call on a field.
    ///
    /// The declaration decides the binding's mutability from the value's own
    /// reference and the binding it names, re-stamps an origin path computed
    /// upstream, and runs no check of its own (`StmtKind::RefDecl`). Its type
    /// is that reference, kept by template owner, and a later use resolves
    /// through it and records the referent. So an instance substitutes the
    /// referent and gets its own bindings back in the origin.
    pub(super) fn bound_place(&self, statement: &Stmt, value: &Expr) -> bool {
        let named = match &value.kind {
            ExprKind::Identifier(name) => {
                (self.receiver && name == "self")
                    || self.params.contains(&name.as_str())
                    || self.local(name)
                    || self.declared(name)
            }
            _ => self.receiver_field(value),
        };
        let declaration = OccurrenceId {
            syntax: self.origins.origin(statement.syntax_id),
            copy: 0,
        };
        let id = self.occurrence(value);
        let admitted = (named || self.reference_call(value) || self.pack_accessor(value))
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.reference_binding_types, declaration).is_some_and(|reference| {
                    fact_at(&facts.expression_types, id) == Some(&reference.referent)
                })
            });
        if admitted {
            self.handle(id);
        }
        admitted
    }

    /// A field read through a reference, which keeps the reference as a
    /// handle: a field has its declared type under the referent's arguments
    /// in a template and a clone alike.
    pub(super) fn reference_member(&self, expr: &Expr) -> bool {
        let ExprKind::Member { object, .. } = &expr.kind else {
            return false;
        };
        let admitted = self.through(object);
        if admitted {
            self.handle(self.occurrence(object));
        }
        admitted
    }

    /// The receiver of a method call made through a reference. The call
    /// borrows it, which is decided by what the receiver is and never by its
    /// type. `named_contract` then demands a nominal struct there, so a
    /// method of a bare parameter, which a clone selects again, stays out.
    pub(super) fn reference_receiver(&self, object: &Expr) -> bool {
        let admitted = self.through(object) || self.module_reference_call(object);
        if admitted {
            let id = self.occurrence(object);
            let mut receivers = self.receivers.borrow_mut();
            if !receivers.contains(&id) {
                receivers.push(id);
            }
        }
        admitted && self.holds(MethodFeatures::REFERENCE_RECEIVERS)
    }

    /// Note that `id` is kept as a reference handle.
    pub(super) fn handle(&self, id: OccurrenceId) {
        let mut handles = self.handles.borrow_mut();
        if !handles.contains(&id) {
            handles.push(id);
        }
    }

    /// A pointer into storage `self` owns: a field of `self`, a `var` local
    /// (`var new_data = unsafe_alloc[Self.T](n)`), or a field of one, whose
    /// recorded type is a pointer with no tracked provenance, or an element
    /// offset from one. Such a pointer holds no loan and names no place, and
    /// it is a pointer under every instance, so its methods are the built-in
    /// ones.
    pub(super) fn pointer(&self, expr: &Expr) -> bool {
        let admitted = match &expr.kind {
            // An untracked pointer field of `self`, of a local, or of a
            // parameter, or a `var` local, or a field whose provenance is the
            // struct's own origin parameter (`Span._data`): neither names a
            // checker-local place, so the retained type is the template's
            // under every instance.
            ExprKind::Member { .. } | ExprKind::Identifier(_) => {
                let local = matches!(&expr.kind, ExprKind::Identifier(name) if self.declared(name));
                (self.receiver_field(expr)
                    || self.local_field(expr)
                    || self.parameter_field(expr)
                    || local)
                    && self.facts.is_none_or(|facts| {
                        fact_at(&facts.expression_types, self.occurrence(expr)).is_some_and(|ty| {
                            matches!(ty, Ty::Pointer { origin, .. }
                            if matches!(
                                origin.as_origin(),
                                None | Some(mojito_types::origin::Origin::Param(_))
                            ))
                        })
                    })
            }
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } => {
                method == "unsafe_offset"
                    && kwargs.is_empty()
                    && matches!(args.as_slice(), [offset]
                        if self.expression(offset) && self.scalar(offset))
                    && self.pointer(object)
            }
            _ => false,
        };
        admitted && self.holds(MethodFeatures::POINTER_SLOTS)
    }

    /// A tracked pointer to `self` or a field of it, or a pointer field of
    /// `self`, rebound to the whole receiver or an interior of it:
    /// `Pointer(to=self.items).unsafe_origin_cast[origin_of(self)]()`,
    /// `self.data.unsafe_origin_cast[origin_of(self)._get_owned_interior["element"]]()`.
    ///
    /// The pointee is the place's declared type under the instance's
    /// arguments, and both provenances are the receiver's own place, the
    /// inner one rooted at `self` or untracked and the cast's the symbolic
    /// `origin_of(self)` with its interior tags, so an instance roots them
    /// at its own `self`.
    pub(super) fn receiver_pointer(&self, expr: &Expr) -> bool {
        let ExprKind::Invoke {
            callee,
            param_args,
            args,
            kwargs,
        } = &expr.kind
        else {
            return false;
        };
        let ExprKind::Member { object, field } = &callee.kind else {
            return false;
        };
        let receiver_origin = matches!(param_args.as_slice(),
            [mojito_ast::ast::ParamArg::Value(origin)] if self.receiver_origin(origin));
        let pointer_to_receiver = matches!(&object.kind, ExprKind::Call { name, param_args, args, kwargs }
            if name == "Pointer"
                && param_args.is_empty()
                && args.is_empty()
                && matches!(kwargs.as_slice(), [to]
                    if to.name == "to"
                        && (self.receiver_itself(&to.value) || self.receiver_field(&to.value))))
            || (self.receiver_field(object) && self.pointer(object));
        field == "unsafe_origin_cast"
            && receiver_origin
            && args.is_empty()
            && kwargs.is_empty()
            && pointer_to_receiver
            && self.holds(MethodFeatures::RECEIVER_POINTERS)
    }

    /// One element slot of such a pointer, `pointer[scalar]`.
    pub(super) fn slot(&self, expr: &Expr) -> bool {
        matches!(&expr.kind, ExprKind::Index { object, index }
            if self.pointer(object) && self.expression(index) && self.scalar(index))
    }

    /// A statement-level pointer operation that yields nothing: destroying
    /// the pointee, freeing the allocation, or initializing the pointee with
    /// a whole value moved in (`unsafe_write(Self.T())`) or a copy of a
    /// named place (`unsafe_write(copy=fill)`).
    ///
    /// A copying write marks its value a copyable read whatever its type, so
    /// an instance keeps the template's mark ([`Self::copied_writes`]).
    pub(super) fn pointer_statement(&self, expr: &Expr) -> bool {
        let ExprKind::MethodCall {
            object,
            method,
            args,
            kwargs,
        } = &expr.kind
        else {
            return false;
        };
        let operand = match (method.as_str(), args.as_slice(), kwargs.as_slice()) {
            ("unsafe_deinit_pointee" | "unsafe_free" | "free", [], []) => true,
            ("unsafe_write", [value], []) => self.whole_value(value),
            ("unsafe_write", [], [copy]) if copy.name == "copy" => {
                let admitted = self.copied_place(&copy.value);
                if admitted {
                    self.copied_writes
                        .borrow_mut()
                        .push(self.occurrence(&copy.value));
                }
                admitted
            }
            _ => false,
        };
        operand && self.pointer(object)
    }

    /// A module function's reference result read by value or borrowed as a
    /// receiver (`pick(a, b, first)` returning `ref[origin_of(a, b)] T`): an
    /// admitted call whose recorded reference, with its origin naming the
    /// body's own places, is kept by template owner as a reference call's is.
    ///
    /// An argument the call keeps as a caller place is a named place bound
    /// to a `ref` parameter, which the call's conventions and the argument's
    /// syntax decide alone.
    fn module_reference_call(&self, expr: &Expr) -> bool {
        let ExprKind::Call {
            param_args,
            args,
            kwargs,
            ..
        } = &expr.kind
        else {
            return false;
        };
        let id = self.occurrence(expr);
        let kept_place = |index: usize, argument: &Expr| {
            let named = match &argument.kind {
                ExprKind::Identifier(name) => {
                    self.declared(name) || self.params.contains(&name.as_str())
                }
                _ => self.receiver_field(argument),
            };
            let argument_id = self.occurrence(argument);
            self.facts.is_none_or(|facts| {
                if !facts.call_place_uses.contains(&argument_id) {
                    return true;
                }
                let kept = named
                    && fact_at(&facts.call_parameters, id)
                        .and_then(|params| params.get(index))
                        .is_some_and(|parameter| {
                            parameter.convention == Some(mojito_ast::ast::ArgConvention::Ref)
                        });
                if kept {
                    let mut places = self.places.borrow_mut();
                    if !places.contains(&argument_id) {
                        places.push(argument_id);
                    }
                }
                kept && self.holds(MethodFeatures::PLACE_ARGUMENTS)
            })
        };
        let admitted = !self.keyed
            && param_args.is_empty()
            && kwargs.is_empty()
            && self.expression(expr)
            && args
                .iter()
                .enumerate()
                .all(|(index, argument)| kept_place(index, argument))
            && self
                .facts
                .is_none_or(|facts| fact_at(&facts.reference_results, id).is_some());
        if admitted {
            self.references.borrow_mut().push(id);
        }
        admitted && self.holds(MethodFeatures::REFERENCE_CALLS)
    }

    /// A reference the body reads a field or calls a method through: a `ref`
    /// local, or a reference call's result.
    fn through(&self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Identifier(name) => {
                self.reference_local(name) && self.holds(MethodFeatures::REFERENCE_LOCALS)
            }
            _ => {
                (self.reference_call(expr) || self.pack_accessor(expr))
                    && self.holds(MethodFeatures::REFERENCE_RECEIVERS)
            }
        }
    }

    /// A `ref` field of the receiver (`self.src` over
    /// `var src: ref[origin] Array[...]`) a reference call borrows for its
    /// own receiver rather than reading the referent out. The field's
    /// declaration decides that, so every instance borrows it too.
    fn borrowed_field_receiver(&self, object: &Expr) {
        let id = self.occurrence(object);
        let borrowed = self.receiver_field(object)
            && self
                .facts
                .is_some_and(|facts| facts.borrowed_reference_receivers.contains(&id));
        let mut receivers = self.receivers.borrow_mut();
        if borrowed && !receivers.contains(&id) {
            receivers.push(id);
        }
    }

    /// `origin_of(self)`, or an interior of it named by a string literal
    /// (`origin_of(self)._get_owned_interior["element"]`): the receiver's
    /// own symbolic place, which names no binding.
    fn receiver_origin(&self, origin: &Expr) -> bool {
        match &origin.kind {
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } => {
                name == "origin_of"
                    && param_args.is_empty()
                    && kwargs.is_empty()
                    && matches!(args.as_slice(), [place] if self.receiver_itself(place))
            }
            ExprKind::Index { object, index } => {
                matches!(&object.kind, ExprKind::Member { object, field }
                    if field == "_get_owned_interior" && self.receiver_origin(object))
                    && matches!(index.kind, ExprKind::Str(_))
            }
            _ => false,
        }
    }
}
