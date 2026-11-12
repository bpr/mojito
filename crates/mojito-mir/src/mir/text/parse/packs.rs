//! Reading an artifact older than MIR 1.159 (a value pack) or 1.160 (a
//! type pack), which declared a pack as one binder, as upstream's two:
//! the implicit infer-only list binder (`vs.values`, `Ts.values`) after
//! the leading infer-only binders, and the visible `ParameterList` or
//! `TypeList` binder after the pack's own slot
//! ([`ParamDecl::push_value_pack`], [`ParamDecl::push_type_pack`]).
//!
//! The old binder's identity becomes the list binder's, which every old
//! reference names with the raw list's meaning; every later binder moves
//! up a slot. An application's arguments aligned with the old binders
//! gain the visible binder's zero-sized value.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::{PARAMETER_LIST_TYPE_NAME, TYPE_LIST_TYPE_NAME, TyKind};

/// Every parameter list of an older artifact that declares a pack as one
/// binder, by the owner of its binders.
#[derive(Default)]
pub(super) struct PackUpgrades {
    splits: HashMap<String, PackSplit>,
    /// The binder owner of each struct or function whose parameter list
    /// the artifact declares, by the name an application names it by.
    owners: HashMap<String, String>,
    /// Owners whose lists declare more than one pack, or one pack two
    /// ways, which no split reads.
    refused: HashSet<String>,
}

impl Decoder {
    /// Scan `artifact` for the parameter lists that declare a pack as one
    /// binder, so that every identity read afterwards is renumbered.
    pub(super) fn scan_pack_upgrades(&mut self, artifact: &Value) {
        let mut scratch = Self::new(false, self.minor);
        let mut upgrades = PackUpgrades::default();
        scan(artifact, &mut scratch, &mut upgrades);
        upgrades
            .owners
            .retain(|_, owner| upgrades.splits.contains_key(owner));
        self.pack_upgrades = upgrades;
    }

    /// The identity `owner`'s binder at `slot` has: renumbered past a
    /// pack's list binder in an older artifact.
    pub(super) fn upgraded_id(&self, owner: &str, slot: usize) -> ParamId {
        let slot = self
            .pack_upgrades
            .splits
            .get(owner)
            .map_or(slot, |split| split.slot(slot));
        ParamId::new(owner, slot)
    }

    /// Whether `tag`'s declaration with `variadic` declares a pack as the
    /// one binder of an older artifact.
    pub(super) fn is_legacy_pack(&self, tag: &str, variadic: bool) -> bool {
        variadic
            && match tag {
                "value_param" => self.minor <= 158,
                "type_param" => self.minor <= 159,
                _ => false,
            }
    }

    /// The two binders an older artifact's one pack binder `decl` (already
    /// at its list binder's identity) declares, the visible binder second.
    pub(super) fn split_legacy_pack(&mut self, value: &Value, decl: ParamDecl) -> Vec<ParamDecl> {
        let id = decl.id().clone();
        if self.pack_upgrades.refused.contains(&*id.owner) {
            self.error(
                value.span,
                "a parameter list declaring more than one pack before MIR 1.160 cannot be \
                 upgraded; re-emit the artifact",
            );
            return Vec::new();
        }
        let visible_id = self.pack_upgrades.splits.get(&*id.owner).map_or_else(
            || ParamId::new(&id.owner, id.slot + 1),
            |split| ParamId::new(&id.owner, split.pack + 1),
        );
        let (list, visible) = match decl {
            // A legacy artifact predates origin binders.
            origin @ ParamDecl::Origin { .. } => return vec![origin],
            ParamDecl::Value {
                ty,
                name,
                default,
                callable_default,
                infer_only,
                constraints,
                ..
            } => {
                // Schema 1.112 and older wrote a pack's element, not the
                // list it binds.
                let (list_ty, element) = match ty.kind() {
                    TyKind::ParamList(element) if self.minor > 112 => (ty.clone(), element.clone()),
                    _ => (TyKind::ParamList(ty.clone()).into(), ty),
                };
                let list = ParamDecl::Value {
                    id,
                    name: list_name(&name),
                    ty: list_ty,
                    default: None,
                    callable_default: None,
                    infer_only: true,
                    variadic: false,
                    constraints: Vec::new(),
                };
                let reference =
                    self.context
                        .decl_ref(list.id().clone(), list.name(), list.binder_meta());
                let visible = ParamDecl::Value {
                    id: visible_id,
                    name,
                    ty: TyKind::Struct(
                        PARAMETER_LIST_TYPE_NAME.to_string(),
                        vec![TyArg::Ty(element), TyArg::Val(CtValue::Expr(reference))],
                    )
                    .into(),
                    default,
                    callable_default,
                    infer_only,
                    variadic: true,
                    constraints,
                };
                (list, visible)
            }
            ParamDecl::Type {
                id,
                name,
                bounds,
                trait_param,
                callable_bound,
                default,
                constraints,
                ..
            } => {
                // The `TypeList` lists the trait-valued binder bounding the
                // pack, else the trait its bounds compose.
                let trait_value = trait_param
                    .as_ref()
                    .and_then(|binder| {
                        let meta = self.binder_metas.get(&binder.id)?.clone();
                        Some(CtValue::Expr(self.context.decl_ref(
                            binder.id.clone(),
                            &binder.name,
                            meta,
                        )))
                    })
                    .unwrap_or_else(|| {
                        CtValue::Trait(mojito_types::types::trait_set(bounds.iter().cloned()))
                    });
                let list = ParamDecl::Type {
                    id,
                    name: list_name(&name),
                    bounds,
                    trait_param,
                    callable_bound,
                    default,
                    infer_only: true,
                    variadic: true,
                    constraints,
                };
                let reference = list.list_reference(list.id());
                let visible_ty = TyKind::Struct(
                    TYPE_LIST_TYPE_NAME.to_string(),
                    vec![
                        TyArg::Val(trait_value),
                        TyArg::Val(CtValue::Expr(reference)),
                    ],
                );
                (
                    list,
                    ParamDecl::Value {
                        id: visible_id,
                        name,
                        ty: Ty::none(),
                        default: None,
                        callable_default: None,
                        infer_only: false,
                        variadic: true,
                        constraints: Vec::new(),
                    }
                    .with_value_type(visible_ty.into()),
                )
            }
        };
        vec![self.declared(list), self.declared(visible)]
    }

    /// `decls` in slot order once an older pack split among them: the list
    /// binder after the leading infer-only binders.
    pub(super) fn order_upgraded(&self, decls: &mut [ParamDecl]) {
        if decls
            .first()
            .is_some_and(|decl| self.pack_upgrades.splits.contains_key(&*decl.id().owner))
        {
            decls.sort_by_key(|decl| decl.id().slot);
        }
    }

    /// The arguments of an application of the struct or function `name`,
    /// aligned with its binders: an older artifact's, aligned with the one
    /// pack binder, regrouped around its two.
    pub(super) fn upgraded_named_arguments(&self, name: &str, arguments: Vec<TyArg>) -> Vec<TyArg> {
        if mojito_types::types::binds_pack_elementwise(name) {
            return arguments;
        }
        match self.pack_upgrades.owners.get(name) {
            Some(owner) => self.upgraded_owner_arguments(owner, arguments),
            None => arguments,
        }
    }

    /// [`Self::upgraded_named_arguments`] for the binders of `owner`.
    pub(super) fn upgraded_owner_arguments(
        &self,
        owner: &str,
        arguments: Vec<TyArg>,
    ) -> Vec<TyArg> {
        let Some(split) = self.pack_upgrades.splits.get(owner) else {
            return arguments;
        };
        split.regroup(arguments, |type_pack| TyArg::Val(pack_value(type_pack)))
    }

    /// A call of a generic callable value or method whose solved
    /// arguments align with the contract it carries: an older artifact's,
    /// aligned with the contract's one pack binder, regrouped around its
    /// two.
    pub(super) fn upgrade_contract_arguments(&self, instruction: &mut MirInstr) {
        if let MirInstr::CallIndirect {
            param_decls,
            instantiated_args,
            ..
        }
        | MirInstr::MethodCall {
            param_decls,
            instantiated_args,
            ..
        } = instruction
            && let Some(decl) = param_decls.first()
        {
            let owner = decl.id().owner.clone();
            *instantiated_args =
                self.upgraded_owner_arguments(&owner, std::mem::take(instantiated_args));
        }
    }

    /// A `param_apply`'s arguments, as [`Self::upgraded_named_arguments`]
    /// regroups a type argument list.
    pub(super) fn upgraded_apply_arguments(
        &self,
        function: &str,
        arguments: Vec<ParamExpr>,
    ) -> Vec<ParamExpr> {
        let Some(split) = self
            .pack_upgrades
            .owners
            .get(function)
            .and_then(|owner| self.pack_upgrades.splits.get(owner))
        else {
            return arguments;
        };
        let Ok(value) = self.context.constant(pack_value(false)) else {
            return arguments;
        };
        let Ok(types) = self.context.constant(pack_value(true)) else {
            return arguments;
        };
        split.regroup(arguments, |type_pack| {
            if type_pack {
                types.clone()
            } else {
                value.clone()
            }
        })
    }
}

/// How the one pack binder of an older parameter list splits.
#[derive(Clone, Copy, PartialEq, Eq)]
struct PackSplit {
    /// The old binder's slot, after which the visible binder goes.
    pack: usize,
    /// The list binder's slot, after the leading infer-only binders.
    list: usize,
    /// How many binders the old list declares.
    count: usize,
    type_pack: bool,
}

impl PackSplit {
    const fn slot(self, old: usize) -> usize {
        if old < self.list {
            old
        } else if old == self.pack {
            self.list
        } else {
            old + 1
        }
    }

    /// `arguments`, aligned with the old binders, aligned with the new: an
    /// argument list of any other length is not an application of these
    /// binders and is returned unchanged.
    fn regroup<T>(self, arguments: Vec<T>, visible: impl FnOnce(bool) -> T) -> Vec<T> {
        if arguments.len() != self.count {
            return arguments;
        }
        let mut slots: Vec<Option<T>> = std::iter::repeat_with(|| None)
            .take(self.count + 1)
            .collect();
        for (old, argument) in arguments.into_iter().enumerate() {
            slots[self.slot(old)] = Some(argument);
        }
        slots[self.pack + 1] = Some(visible(self.type_pack));
        slots.into_iter().flatten().collect()
    }
}

fn scan(value: &Value, scratch: &mut Decoder, upgrades: &mut PackUpgrades) {
    match &value.kind {
        ValueKind::Record(_, fields) => {
            if let Ok(decls) = scratch.field(fields, "param_decls")
                && let Ok(name) = scratch
                    .field(fields, "name")
                    .or_else(|()| scratch.field(fields, "lowered_name"))
                && let Some(name) = scratch.symbol(name)
                && let Some(owner) = scratch
                    .list(decls)
                    .ok()
                    .and_then(|decls| decls.first())
                    .and_then(|decl| binder_owner(decl, scratch))
            {
                upgrades.owners.entry(name).or_insert(owner);
            }
            for field in fields {
                scan(&field.value, scratch, upgrades);
            }
        }
        ValueKind::List(values) => {
            scan_decls(values, scratch, upgrades);
            for value in values {
                scan(value, scratch, upgrades);
            }
        }
        ValueKind::Positional(_, inner) => scan(inner, scratch, upgrades),
        ValueKind::Atom(_) | ValueKind::String(_) => {}
    }
}

/// Record the split of `values` when it is a parameter list declaring a
/// pack as one binder.
fn scan_decls(values: &[Value], scratch: &mut Decoder, upgrades: &mut PackUpgrades) {
    let mut decls = Vec::new();
    for value in values {
        let ValueKind::Record(tag, fields) = &value.kind else {
            return;
        };
        if tag != "type_param" && tag != "value_param" {
            return;
        }
        let flag = |scratch: &mut Decoder, name| {
            scratch
                .field(fields, name)
                .ok()
                .and_then(|value| scratch.boolean(value))
                .unwrap_or(false)
        };
        let infer_only = flag(scratch, "infer_only");
        let variadic = flag(scratch, "variadic");
        let pack = scratch.is_legacy_pack(tag, variadic);
        let slot = scratch
            .field(fields, "slot")
            .ok()
            .and_then(|value| scratch.uint(value));
        decls.push((binder_owner(value, scratch), slot, infer_only, pack));
    }
    let packs: Vec<usize> = (0..decls.len()).filter(|&at| decls[at].3).collect();
    let Some(&at) = packs.first() else {
        return;
    };
    let (Some(owner), Some(pack)) = (decls[at].0.clone(), decls[at].1) else {
        return;
    };
    let split = PackSplit {
        pack,
        list: decls[..at].iter().take_while(|decl| decl.2).count(),
        count: decls.len(),
        type_pack: matches!(&values[at].kind, ValueKind::Record(tag, _) if tag == "type_param"),
    };
    let conflicting = packs.len() > 1
        || upgrades
            .splits
            .get(&owner)
            .is_some_and(|known| *known != split);
    if conflicting {
        upgrades.splits.remove(&owner);
        upgrades.refused.insert(owner);
    } else if !upgrades.refused.contains(&owner) {
        upgrades.splits.insert(owner, split);
    }
}

fn binder_owner(decl: &Value, scratch: &mut Decoder) -> Option<String> {
    let ValueKind::Record(_, fields) = &decl.kind else {
        return None;
    };
    let owner = scratch.field(fields, "owner").ok()?;
    scratch.string(owner)
}

/// The implicit list binder's name of the pack spelled `name` (`*vs`).
fn list_name(name: &str) -> String {
    format!("{}.values", name.trim_start_matches('*'))
}

fn pack_value(type_pack: bool) -> CtValue {
    if type_pack {
        ParamDecl::type_list_value()
    } else {
        ParamDecl::parameter_list_value()
    }
}

#[cfg(test)]
mod tests {
    use super::PackSplit;

    /// `[k, a, vs, b]` with `k` infer-only reads as `[k, vs.values, a,
    /// vs, b]`: the list after `k`, the visible binder after the old
    /// pack's slot.
    #[test]
    fn older_arguments_regroup_around_the_split_pack() {
        let split = PackSplit {
            pack: 2,
            list: 1,
            count: 4,
            type_pack: false,
        };
        let regrouped = split.regroup(vec!["k", "a", "list", "b"], |_| "visible");
        assert_eq!(regrouped, ["k", "list", "a", "visible", "b"]);
        assert_eq!(split.regroup(vec!["k"], |_| "visible"), ["k"]);
    }
}
