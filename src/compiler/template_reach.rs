//! What a template-served body reaches once its owner's parameters are
//! bound: a generic struct's template methods at an instance, and a generic
//! `def` at a closed call.
//!
//! A method with no compile-time construct in its body mints no clone per
//! instance, and neither does such a `def` at a closed call: the elaborator
//! instantiates the template's MIR. The clone's check is then no longer
//! there to discover what the body reaches at those arguments, so the
//! driver reads it off the template's checked types instead.

use super::{ServedRequests, StructInstanceRequest, closed_generic_argument};
use crate::ast::{Expr, Stmt, StmtKind};
use crate::checked::DiscoveryResult;
use crate::comptime::specialized_struct_template_names;
use crate::ct::CtValue;
use crate::param_expr::{ParamError, ParamExpr, ParamRef};
use crate::types::{TyRewrite, mentions, rewrite_ty};
use crate::{Ty, TyArg};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

/// What the template-served bodies of a compilation demand across its
/// discovery rounds.
pub(super) struct TemplateDemand {
    /// Structs specialized whole by the AST cloner (`Tuple`, a value-keyed
    /// struct): an application of one over a struct's parameter has no
    /// template the elaborator could instantiate.
    specialized: HashSet<String>,
    /// Template methods that still clone per instance because of what their
    /// checked bodies hold, as (struct, method). A `def` that still clones
    /// for the same reason is (`def`, "").
    keyed_methods: Vec<(String, String)>,
    /// Methods the last elaboration found reaching a compile-time-keyed
    /// stub, as (struct, method): a call of one with binders of its own
    /// keeps its per-call clone.
    stub_reaching: Vec<(String, String)>,
}

impl TemplateDemand {
    pub(super) fn new(linked: &[Stmt]) -> Self {
        Self {
            specialized: specialized_struct_template_names(linked),
            keyed_methods: Vec::new(),
            stub_reaching: Vec::new(),
        }
    }

    /// Record the methods an elaboration found reaching a compile-time-keyed
    /// stub; the next round keys them.
    pub(super) fn note_stub_reaching(&mut self, methods: Vec<(String, String)>) {
        self.stub_reaching = methods;
    }

    pub(super) fn keyed_methods(&self) -> &[(String, String)] {
        &self.keyed_methods
    }

    /// Add what the template bodies of `struct_requests` reach in `checked`
    /// to the round's requests, and name the last one that joined: an
    /// instance joins `struct_requests`, and a method that still clones joins
    /// the keyed ones. A reached instance must be served like a keyed method,
    /// because the template of a method it keeps a clone of is a trap stub.
    pub(super) fn request(
        &mut self,
        checked: &DiscoveryResult,
        struct_requests: &mut Vec<StructInstanceRequest>,
        served: &mut ServedRequests,
    ) -> Option<String> {
        let mut reach = TemplateReach::new(checked, &self.specialized);
        let mut last = None;
        let calls = closed_def_calls(checked);
        let owners = |struct_requests: &[StructInstanceRequest]| {
            [struct_requests, calls.as_slice()].concat()
        };
        // A method with compile-time parameters of its own is served per
        // call: what its body reaches over its own binders is read at each
        // closed call's arguments.
        let mut keyed = Vec::new();
        for call in closed_method_calls(checked) {
            let (instances, keys) = reach.method_call(&call);
            for request in instances {
                if !struct_requests.contains(&request) {
                    last = Some(request.template().to_string());
                    served.instances.push(request.clone());
                    struct_requests.push(request);
                }
            }
            if keys && !keyed.contains(&(call.owner.clone(), call.method.clone())) {
                keyed.push((call.owner.clone(), call.method.clone()));
            }
        }
        for request in reach.instances(&owners(struct_requests)) {
            last = Some(request.template().to_string());
            served.instances.push(request.clone());
            struct_requests.push(request);
        }
        for method in reach
            .keyed_methods(&owners(struct_requests))
            .into_iter()
            .chain(self.stub_reaching.iter().cloned())
        {
            if !keyed.contains(&method) {
                keyed.push(method);
            }
        }
        for method in keyed {
            if !self.keyed_methods.contains(&method) {
                last = Some(format!("{}.{}", method.0, method.1));
                served.keyed_templates.push(method.0.clone());
                self.keyed_methods.push(method);
            }
        }
        last
    }
}

/// The symbolic struct applications of each generic struct's template
/// methods, read lazily from one check's facts.
struct TemplateReach<'a> {
    checked: &'a DiscoveryResult,
    specialized: &'a HashSet<String>,
    templates: HashMap<&'a str, &'a Stmt>,
    reached: HashMap<String, Vec<Application>>,
}

impl<'a> TemplateReach<'a> {
    fn new(checked: &'a DiscoveryResult, specialized: &'a HashSet<String>) -> Self {
        let templates = checked
            .statements
            .iter()
            .filter_map(|statement| match &statement.kind {
                StmtKind::Struct {
                    name, type_params, ..
                }
                | StmtKind::Def {
                    name, type_params, ..
                } if !type_params.is_empty() => Some((name.as_str(), statement)),
                // A non-generic struct's method may declare binders of its own.
                StmtKind::Struct { name, methods, .. }
                    if methods.iter().any(|method| !method.type_params.is_empty()) =>
                {
                    Some((name.as_str(), statement))
                }
                _ => None,
            })
            .collect();
        Self {
            checked,
            specialized,
            templates,
            reached: HashMap::new(),
        }
    }

    /// The closed instances the template bodies of `requests` reach,
    /// transitively, that `requests` does not hold.
    fn instances(&mut self, requests: &[StructInstanceRequest]) -> Vec<StructInstanceRequest> {
        let mut found: Vec<StructInstanceRequest> = Vec::new();
        let mut pending: Vec<StructInstanceRequest> = requests.to_vec();
        while let Some(request) = pending.pop() {
            let template = request.template().to_string();
            let mut bind = BindOwner {
                owners: vec![(&template, request.arguments())],
            };
            let specialized = self.specialized;
            let reached: Vec<StructInstanceRequest> = self
                .applications(&template)
                .iter()
                .filter(|application| {
                    !application.keyed && !specialized.contains(&application.name)
                })
                .filter_map(|application| {
                    let arguments = application
                        .arguments
                        .iter()
                        .map(|argument| bind.argument(argument))
                        .collect::<Option<Vec<_>>>()?;
                    arguments
                        .iter()
                        .all(|argument| {
                            closed_generic_argument(argument)
                                && !matches!(
                                    argument,
                                    TyArg::Val(CtValue::Deferred(_) | CtValue::Marker(_))
                                )
                        })
                        .then(|| StructInstanceRequest::new(application.name.clone(), arguments))
                })
                .collect();
            for instance in reached {
                if !requests.contains(&instance) && !found.contains(&instance) {
                    found.push(instance.clone());
                    pending.push(instance);
                }
            }
        }
        found
    }

    /// The template bodies of the owners `requests` instantiate that only
    /// an instance's own check can serve, as (struct, method), or (`def`, "")
    /// for a `def`'s:
    /// one that applies a struct specialized whole, or builds a tuple, over
    /// the struct's parameters, and one that calls an overloaded method with
    /// compile-time parameters of its own, which runs as a per-call clone.
    fn keyed_methods(&mut self, requests: &[StructInstanceRequest]) -> Vec<(String, String)> {
        let mut templates: Vec<&str> = requests
            .iter()
            .map(StructInstanceRequest::template)
            .collect();
        templates.sort_unstable();
        templates.dedup();
        let mut keyed = Vec::new();
        for template in templates {
            let specialized = self.specialized;
            for application in self.applications(template) {
                let key = (template.to_string(), application.method.clone());
                if application.names(template)
                    && (application.keyed || specialized.contains(&application.name))
                    && !keyed.contains(&key)
                {
                    keyed.push(key);
                }
            }
        }
        keyed
    }

    /// What one closed call of a method with binders of its own reaches
    /// over them: the closed instances its body applies, and whether the
    /// body only a per-call clone can serve (a tuple, or a struct
    /// specialized whole, over the method's own binders).
    fn method_call(&mut self, call: &MethodCall) -> (Vec<StructInstanceRequest>, bool) {
        let binder_owner = call.binder_owner();
        let mut bind = BindOwner {
            owners: vec![
                (call.owner.as_str(), call.owner_arguments.as_slice()),
                (binder_owner.as_str(), call.arguments.as_slice()),
            ],
        };
        let specialized = self.specialized;
        let mut instances = Vec::new();
        let mut keyed = false;
        for application in self.applications(&call.owner).iter().filter(|application| {
            application.method == call.method && application.names(&binder_owner)
        }) {
            if application.keyed || specialized.contains(&application.name) {
                keyed = true;
                continue;
            }
            let Some(arguments) = application
                .arguments
                .iter()
                .map(|argument| bind.argument(argument))
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            if arguments.iter().all(|argument| {
                closed_generic_argument(argument)
                    && !matches!(
                        argument,
                        TyArg::Val(CtValue::Deferred(_) | CtValue::Marker(_))
                    )
            }) {
                let request = StructInstanceRequest::new(application.name.clone(), arguments);
                if !instances.contains(&request) {
                    instances.push(request);
                }
            }
        }
        (instances, keyed)
    }

    fn applications(&mut self, template: &str) -> &[Application] {
        if !self.reached.contains_key(template) {
            let applications = self
                .templates
                .get(template)
                .map(|statement| template_applications(self.checked, template, statement))
                .unwrap_or_default();
            self.reached.insert(template.to_string(), applications);
        }
        &self.reached[template]
    }
}

/// What one template method's body holds over its struct's parameters or
/// its own: a struct application, or (`keyed`, naming no struct) a construct
/// that keeps the method's per-instance or per-call clone.
struct Application {
    method: String,
    name: String,
    arguments: Vec<TyArg>,
    keyed: bool,
    /// The binder owners the application names: the template's, a method's
    /// own, or (`None`) every owner, for a construct keyed by its call.
    owners: Option<Vec<String>>,
}

impl Application {
    fn names(&self, owner: &str) -> bool {
        self.owners
            .as_ref()
            .is_none_or(|owners| owners.iter().any(|named| named == owner))
    }
}

/// One closed call of a struct method that declares compile-time parameters
/// of its own, as the checker recorded it.
struct MethodCall {
    owner: String,
    owner_arguments: Vec<TyArg>,
    method: String,
    overload: Option<String>,
    arguments: Vec<TyArg>,
}

impl MethodCall {
    /// The owner the method's own binders carry: the template struct's
    /// method, qualified by its signature when it is overloaded.
    fn binder_owner(&self) -> String {
        let template = crate::symbol::specialization_template(&self.owner).unwrap_or(&self.owner);
        format!(
            "{template}.{}{}",
            self.method,
            self.overload.as_deref().unwrap_or("")
        )
    }
}

/// The owners' parameters replaced by one instance's arguments: a struct's,
/// a `def`'s, or a method's own beside its struct's.
struct BindOwner<'a> {
    owners: Vec<(&'a str, &'a [TyArg])>,
}

impl BindOwner<'_> {
    fn argument(&mut self, argument: &TyArg) -> Option<TyArg> {
        match argument {
            TyArg::Ty(ty) => rewrite_ty(ty, self).ok().map(TyArg::Ty),
            other => Some(other.clone()),
        }
    }
}

impl TyRewrite for BindOwner<'_> {
    fn param(&mut self, binder: &ParamRef) -> Option<Ty> {
        let (_, arguments) = self
            .owners
            .iter()
            .find(|(owner, _)| *owner == &*binder.id.owner)?;
        match arguments.get(binder.id.slot)? {
            TyArg::Ty(ty) => Some(ty.clone()),
            TyArg::Val(_) | TyArg::Origin(_) => None,
        }
    }

    fn expr(&mut self, expr: &ParamExpr) -> Result<ParamExpr, ParamError> {
        Ok(expr.clone())
    }
}

fn template_applications(
    checked: &DiscoveryResult,
    template: &str,
    statement: &Stmt,
) -> Vec<Application> {
    struct Bodies<'a> {
        checked: &'a DiscoveryResult,
        template: &'a str,
        method: &'a str,
        found: Vec<Application>,
    }

    impl Bodies<'_> {
        /// Whether `owner` is the template's own, or the owner of one of its
        /// methods' own binders (`S.show`, `Variant.isa` on a
        /// specialization of `Variant`).
        fn owns(&self, owner: &str) -> bool {
            let template =
                crate::symbol::specialization_template(self.template).unwrap_or(self.template);
            owner == self.template
                || owner
                    .strip_prefix(template)
                    .is_some_and(|rest| rest.starts_with('.'))
        }

        fn collect(&mut self, ty: &Ty) {
            let named_owners = |ty: &Ty| {
                let owners = RefCell::new(Vec::new());
                mentions(ty, &|inner| {
                    if let Ty::Param { binder, .. } = inner
                        && self.owns(&binder.id.owner)
                        && !owners.borrow().contains(&binder.id.owner.to_string())
                    {
                        owners.borrow_mut().push(binder.id.owner.to_string());
                    }
                    false
                });
                owners.into_inner()
            };
            let found = RefCell::new(Vec::new());
            mentions(ty, &|inner| {
                match inner {
                    Ty::Struct(name, arguments) => {
                        let owners = named_owners(inner);
                        if !owners.is_empty() {
                            found.borrow_mut().push((
                                name.clone(),
                                arguments.clone(),
                                false,
                                owners,
                            ));
                        }
                    }
                    Ty::Tuple(_) => {
                        let owners = named_owners(inner);
                        if !owners.is_empty() {
                            found.borrow_mut().push((
                                String::new(),
                                Vec::new().into(),
                                true,
                                owners,
                            ));
                        }
                    }
                    _ => {}
                }
                false
            });
            for (name, arguments, keyed, owners) in found.into_inner() {
                self.record(name, arguments.into(), keyed, Some(owners));
            }
        }

        fn record(
            &mut self,
            name: String,
            arguments: Vec<TyArg>,
            keyed: bool,
            owners: Option<Vec<String>>,
        ) {
            let seen = self.found.iter().any(|application| {
                application.method == self.method
                    && application.name == name
                    && application.arguments == arguments
                    && application.keyed == keyed
                    && application.owners == owners
            });
            if !seen {
                self.found.push(Application {
                    method: self.method.to_string(),
                    name,
                    arguments,
                    keyed,
                    owners,
                });
            }
        }
    }

    impl mojito_ast::visit::Visitor for Bodies<'_> {
        fn visit_expr(&mut self, expression: &Expr) {
            let checked = self.checked;
            if checked
                .method_instantiations
                .get(&expression.source_span())
                .is_some_and(|instantiation| instantiation.overload.is_some())
            {
                self.record(String::new(), Vec::new(), true, None);
            }
            for ty in [
                checked.expression_type(expression),
                checked.expression_place_type(expression),
                checked.expression_binding_type(expression),
            ]
            .into_iter()
            .flatten()
            {
                self.collect(ty);
            }
        }
    }

    let mut bodies = Bodies {
        checked,
        template,
        method: "",
        found: Vec::new(),
    };
    match &statement.kind {
        StmtKind::Struct { methods, .. } => {
            for method in methods.iter().filter(|method| method.self_ty.is_none()) {
                bodies.method = &method.name;
                mojito_ast::visit::walk_block(&mut bodies, &method.body);
            }
        }
        StmtKind::Def { body, .. } => mojito_ast::visit::walk_block(&mut bodies, body),
        _ => {}
    }
    bodies.found
}

/// The closed calls of generic `def`s in `checked` that still name their
/// template, each as its callee with the arguments it binds. Such a call may
/// be served by the template.
fn closed_def_calls(checked: &DiscoveryResult) -> Vec<StructInstanceRequest> {
    let mut calls: Vec<StructInstanceRequest> = Vec::new();
    for instantiation in checked.generic_instantiations.values() {
        if !instantiation.arguments.iter().all(closed_generic_argument) {
            continue;
        }
        let call = StructInstanceRequest::new(
            instantiation.callee.clone(),
            instantiation.arguments.clone(),
        );
        if !calls.contains(&call) {
            calls.push(call);
        }
    }
    calls.sort_by_cached_key(|call| format!("{}{:?}", call.template(), call.arguments()));
    calls
}

/// The closed calls of struct methods with compile-time parameters of their
/// own in `checked`, each with the receiver instance's arguments and the
/// method's own. Such a call may be served by the template.
fn closed_method_calls(checked: &DiscoveryResult) -> Vec<MethodCall> {
    let mut calls: Vec<MethodCall> = checked
        .method_instantiations
        .values()
        .filter(|instantiation| {
            instantiation
                .arguments
                .iter()
                .chain(&instantiation.owner_arguments)
                .all(closed_generic_argument)
        })
        .map(|instantiation| MethodCall {
            owner: instantiation.owner.clone(),
            owner_arguments: instantiation.owner_arguments.clone(),
            method: instantiation.method.clone(),
            overload: instantiation.overload.clone(),
            arguments: instantiation.arguments.clone(),
        })
        .collect();
    calls.sort_by_cached_key(|call| {
        format!(
            "{}.{}{:?}{:?}",
            call.owner, call.method, call.owner_arguments, call.arguments
        )
    });
    calls.dedup_by(|a, b| {
        a.owner == b.owner
            && a.method == b.method
            && a.overload == b.overload
            && a.owner_arguments == b.owner_arguments
            && a.arguments == b.arguments
    });
    calls
}
