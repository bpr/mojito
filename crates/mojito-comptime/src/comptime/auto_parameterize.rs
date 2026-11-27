//! Upstream's automatic parameterization (`addImplicitTypeParams`,
//! `MojoParser/Signatures.cpp`): each parameter an argument's struct type
//! leaves unbound becomes an infer-only parameter of the declaration that
//! takes the argument.

use mojito_ast::ast::{DeclaredParam, FnParam, ParamArg, auto_binder_name};
use mojito_ast::call::Unbound;
use mojito_ast::visit::MutVisitor;

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

/// Declare an infer-only binder for each parameter that an argument's struct
/// type leaves unbound, and bind the type to the binders, on a `def` (one
/// nested in a body too), a struct method, a trait requirement, and the
/// compile-time parameters of a struct type (`[t: T]`) of a struct, a
/// `def`, an alias, and a struct's or a trait's `comptime` member, as
/// upstream does on every parameter list.
///
/// A parameter is unbound when the struct is spelled bare (`x: List`, every
/// one of its parameters, a type pack as a pack binder), as `_` (`List[_]`,
/// `Pair[n=_]`), or under `...` (`Pair[Int, ...]`, each remaining
/// one, keyword-only ones included). A module alias over a partially bound struct
/// (`comptime I32 = SIMD[DType.int32, _]`, the prelude's `Scalar`)
/// spelled bare stands for its struct with its holes. A module alias with
/// compile-time parameters spelled bare (`comptime Lane[dt: DType] =
/// SIMD[dt, 1]`, `x: Lane`) leaves each one unbound, as a bare struct does.
/// A module alias of an unapplied struct (`comptime StringSlice =
/// StringSpan`) is that struct's generator, so its application is the
/// struct's. An unbound origin (`Span[Int, _]`, a bare `StringSpan`) takes
/// a binder of its own, after one for the `mut=` binder its type names
/// when the application leaves that unbound too, as upstream's `[mut_p:
/// Bool, origin_p: Origin[mut=mut_p], //]`.
/// The struct is the program's own or the `installed` package's, whose
/// declared parameters give each binder its type, with the struct's earlier
/// parameters read as their arguments. The binders follow the declaration's own infer-only
/// parameters, in argument order, as upstream inserts them; the argument
/// solves each at each call, so the elaborator instantiates the declaration
/// per argument type. `$` keeps the binders unspellable in source.
pub(super) fn parameterize_unbound_arguments(
    program: &mut [Stmt],
    installed: Option<&InstalledInterfaces>,
) {
    let mut declared = Declared::of(program, installed);
    mojito_ast::visit::walk_block_mut(&mut declared, program);
}

/// The struct declarations an argument type names, and the module aliases
/// over a partially bound struct.
struct Declared<'a> {
    /// Each of the program's own structs, with its parameters as declared.
    structs: HashMap<String, Vec<TypeParam>>,
    /// Each module alias with compile-time parameters, with its parameters
    /// as declared.
    parametric_aliases: HashMap<String, Vec<TypeParam>>,
    /// Every other name the program declares, which hides a package struct.
    others: HashSet<String>,
    /// Each module alias whose body applies a struct with a hole, as the
    /// struct and its arguments.
    partial_aliases: HashMap<String, (String, Vec<ParamArg>)>,
    /// Each module alias whose body names a struct unapplied (`comptime Q
    /// = P`), as the struct whose generator it is.
    generator_aliases: HashMap<String, String>,
    installed: Option<&'a InstalledInterfaces>,
}

impl<'a> Declared<'a> {
    fn of(program: &[Stmt], installed: Option<&'a InstalledInterfaces>) -> Self {
        let mut declared = Self {
            structs: HashMap::new(),
            parametric_aliases: HashMap::new(),
            others: HashSet::new(),
            partial_aliases: HashMap::new(),
            generator_aliases: HashMap::new(),
            installed,
        };
        for statement in program {
            match &statement.kind {
                StmtKind::Struct {
                    name, type_params, ..
                } => {
                    declared.structs.insert(name.clone(), type_params.clone());
                }
                StmtKind::Comptime {
                    name,
                    type_params,
                    value,
                    ..
                } => {
                    if let Some((applied, args)) = value.bracket_application()
                        && type_params.is_empty()
                        && args.iter().any(|argument| hole(argument).is_some())
                    {
                        declared
                            .partial_aliases
                            .insert(name.clone(), (applied.to_string(), args.into_owned()));
                    }
                    if let ExprKind::Identifier(applied) = &value.kind
                        && type_params.is_empty()
                    {
                        declared
                            .generator_aliases
                            .insert(name.clone(), applied.clone());
                    }
                    if !type_params.is_empty() {
                        declared
                            .parametric_aliases
                            .insert(name.clone(), type_params.clone());
                    }
                    declared.others.insert(name.clone());
                }
                StmtKind::Trait { name, .. } | StmtKind::Def { name, .. } => {
                    declared.others.insert(name.clone());
                }
                _ => {}
            }
        }
        declared
    }

    /// The parameters struct or parametric module alias `name` declares:
    /// the program's own, or, unless another declaration of the program
    /// hides it, the `installed` package's (`ImmPointer`).
    fn parameters(&self, name: &str) -> Option<&[TypeParam]> {
        if let Some(parameters) = self
            .structs
            .get(name)
            .or_else(|| self.parametric_aliases.get(name))
        {
            return Some(parameters);
        }
        if self.others.contains(name) {
            return None;
        }
        let installed = self.installed?;
        installed
            .struct_parameters(name)
            .or_else(|| installed.parametric_alias_parameters(name))
    }

    /// The struct whose generator the alias `name` names: the program's own
    /// module alias, or, unless another declaration of the program hides
    /// it, the `installed` package's (`comptime StringSlice = StringSpan`).
    fn generator_alias(&self, name: &str) -> Option<String> {
        if let Some(applied) = self.generator_aliases.get(name) {
            return Some(applied.clone());
        }
        if self.structs.contains_key(name) || self.others.contains(name) {
            return None;
        }
        self.installed?.generator_alias(name).map(str::to_string)
    }

    /// The struct and arguments the partial alias `name` applies: the
    /// program's own module alias, or, unless another declaration of the
    /// program hides it, the `installed` package's
    /// (`comptime Scalar = SIMD[_, length=1]`).
    fn partial_alias(&self, name: &str) -> Option<(String, Vec<ParamArg>)> {
        if let Some(alias) = self.partial_aliases.get(name) {
            return Some(alias.clone());
        }
        if self.structs.contains_key(name) || self.others.contains(name) {
            return None;
        }
        // An alias with compile-time parameters of its own
        // (`comptime MutPointer[T, origin: MutOrigin, …] = Pointer[…]`)
        // binds them, as the program's own does ([`Self::parameters`]).
        let installed = self.installed?;
        if installed.parametric_alias_parameters(name).is_some() {
            return None;
        }
        let (applied, args) = installed.alias_application(name)?;
        args.iter()
            .any(|argument| hole(argument).is_some())
            .then_some((applied, args))
    }

    /// Parameterize one declaration's compile-time parameters
    /// `type_params` and arguments `params`; `enclosing` names the
    /// enclosing struct's parameters, which a bare name may also spell.
    fn parameterize(
        &self,
        type_params: &mut Vec<TypeParam>,
        params: &mut [FnParam],
        enclosing: &[String],
        span: Span,
    ) {
        self.parameterize_with_receiver(type_params, params, None, enclosing, span);
    }

    /// [`Self::parameterize`] with a method's custom receiver annotation
    /// (`self: Pointer[U, _]` under `@__allow_legacy_custom_self_type`),
    /// argument 0, parameterized first as upstream's `self.origin`.
    fn parameterize_with_receiver(
        &self,
        type_params: &mut Vec<TypeParam>,
        params: &mut [FnParam],
        receiver: Option<&mut Expr>,
        enclosing: &[String],
        span: Span,
    ) {
        let in_scope: HashSet<String> = type_params
            .iter()
            .map(|parameter| &parameter.name)
            .chain(enclosing)
            .map(|name| name.trim_start_matches('*').to_string())
            .collect();
        let mut binders = Vec::new();
        // Origin metadata the checker infers from places is not a
        // struct-typed argument, though `Origin` is a library struct.
        let erased: Vec<bool> = type_params
            .iter()
            .map(|parameter| parameter.is_erased_origin_param(type_params))
            .collect();
        for (parameter, erased) in type_params.iter_mut().zip(erased) {
            if erased || parameter.callable_bound.is_some() || parameter.origin_mutability.is_some()
            {
                continue;
            }
            match (&mut parameter.value_type, parameter.bounds.as_slice()) {
                (Some(annotation), _) => {
                    self.rebind(annotation, &parameter.name, &in_scope, &mut binders);
                }
                (
                    None,
                    [
                        spelled @ Expr {
                            kind: ExprKind::Identifier(_),
                            ..
                        },
                    ],
                ) => {
                    parameter.value_type =
                        self.bind(spelled, &parameter.name, &in_scope, &mut binders, span);
                }
                _ => {}
            }
        }
        if let Some(receiver) = receiver {
            self.rebind(receiver, "self", &in_scope, &mut binders);
        }
        // A named `out` result is the declaration's result, not an
        // argument: its type is never parameterized.
        for parameter in params
            .iter_mut()
            .filter(|parameter| !parameter.is_named_result())
        {
            self.rebind(&mut parameter.ty, &parameter.name, &in_scope, &mut binders);
        }
        let at = type_params
            .iter()
            .take_while(|parameter| parameter.infer_only)
            .count();
        type_params.splice(at..at, binders);
    }

    /// [`Self::bind`] in place: `annotation`, keeping its source, is
    /// replaced by its bound application when it leaves a parameter
    /// unbound.
    fn rebind(
        &self,
        annotation: &mut Expr,
        owner: &str,
        in_scope: &HashSet<String>,
        binders: &mut Vec<TypeParam>,
    ) {
        if let Some(mut bound) = self.bind(annotation, owner, in_scope, binders, annotation.span) {
            bound.source = annotation.source.take();
            *annotation = bound;
        }
    }

    /// The annotation `annotation`, the type of `owner`, applied with each
    /// parameter it leaves unbound bound to a binder pushed onto `binders`:
    /// a struct named bare (`List`) or applied (`Pair[Int, _]`). `None`
    /// when it leaves none unbound or names no struct.
    fn bind(
        &self,
        annotation: &Expr,
        owner: &str,
        in_scope: &HashSet<String>,
        binders: &mut Vec<TypeParam>,
        span: Span,
    ) -> Option<Expr> {
        let (name, args) = applied_name(annotation)?;
        if in_scope.contains(name) {
            return None;
        }
        let args = args.as_ref();
        let generator = self.generator_alias(name);
        let name = generator.as_deref().unwrap_or(name);
        let alias = self
            .partial_alias(name)
            .and_then(|(applied, alias_args)| Some((applied, fill_holes(&alias_args, args)?)));
        let (name, args) = alias
            .as_ref()
            .map_or((name, args), |(applied, alias_args)| {
                (applied.as_str(), alias_args.as_slice())
            });
        let declared = self.parameters(name)?;
        // A pack takes every remaining positional argument, so only a bare
        // struct leaves it unbound.
        if !args.is_empty()
            && declared
                .iter()
                .any(|parameter| parameter.name.starts_with('*'))
        {
            return None;
        }
        let slots = Slots::of(declared, args)?;
        // Every struct's unbound origin binds per call, as upstream's does,
        // and so does that of a parametric alias (`MutPointer[T, _]`). A
        // partial alias's hole still reaches the checker as `_` (R2441).
        let binds_origin = alias.is_none();
        let declared_binders = binders.len();
        let mut arguments: HashMap<String, ParamArg> = HashMap::new();
        let mut positional = Vec::new();
        let mut keyword = Vec::new();
        let mut in_order = true;
        for (parameter, slot) in declared.iter().zip(slots.0) {
            let argument = match slot {
                Slot::Absent => {
                    in_order &= parameter.infer_only;
                    continue;
                }
                Slot::Bound(argument) => argument,
                Slot::Unbound if parameter.is_erased_origin_param(declared) => {
                    if parameter.infer_only {
                        continue;
                    }
                    if binds_origin {
                        // An origin whose mutability names an infer-only
                        // sibling the application leaves unbound
                        // (`Origin[mut=mut]`) takes its own `Bool` binder
                        // first, as upstream's `[mut_p: Bool, origin_p:
                        // Origin[mut=mut_p], //]`.
                        if let Some(sibling) = mutability_sibling(parameter, declared)
                            && !arguments.contains_key(&sibling.name)
                        {
                            let binder = auto_binder_name(&sibling.name, owner);
                            binders.push(binder_for(sibling, &binder, &arguments, span));
                            arguments.insert(sibling.name.clone(), identifier(&binder, span));
                        }
                        let binder = auto_binder_name(&parameter.name, owner);
                        binders.push(binder_for(parameter, &binder, &arguments, span));
                        identifier(&binder, span)
                    } else {
                        identifier("_", span)
                    }
                }
                // A type pack's binder is a pack its argument spreads.
                Slot::Unbound if let Some(pack) = parameter.name.strip_prefix('*') => {
                    let binder = auto_binder_name(pack, owner);
                    binders.push(binder_for(
                        parameter,
                        &format!("*{binder}"),
                        &arguments,
                        span,
                    ));
                    let pack = Expr::new(ExprKind::Identifier(binder), span);
                    ParamArg::Value(Expr::new(ExprKind::Spread(Box::new(pack)), span))
                }
                Slot::Unbound => {
                    let binder = auto_binder_name(&parameter.name, owner);
                    binders.push(binder_for(parameter, &binder, &arguments, span));
                    identifier(&binder, span)
                }
            };
            arguments.insert(
                parameter.name.trim_start_matches('*').to_string(),
                argument.clone(),
            );
            if parameter.infer_only || parameter.keyword_only || !in_order {
                keyword.push(ParamArg::Named {
                    name: parameter.name.clone(),
                    value: Box::new(argument),
                });
            } else {
                positional.push(argument);
            }
        }
        // A type that leaves only `_` origins unbound already takes any.
        if binders.len() == declared_binders {
            return None;
        }
        positional.extend(keyword);
        mojito_ast::ast::named_subscript(name, &positional, span)
    }
}

/// Each declaration, a `def` nested in a body included, parameterized as it
/// is visited.
impl MutVisitor for Declared<'_> {
    fn visit_stmt_mut(&mut self, statement: &mut Stmt) {
        let span = statement.span;
        match &mut statement.kind {
            StmtKind::Def {
                type_params,
                params,
                ..
            } => self.parameterize(type_params, params, &[], span),
            StmtKind::Comptime { type_params, .. } => {
                self.parameterize(type_params, &mut [], &[], span);
            }
            StmtKind::Struct {
                type_params,
                associated,
                methods,
                ..
            } => {
                let enclosing: Vec<String> = type_params.iter().map(|p| p.name.clone()).collect();
                self.parameterize(type_params, &mut [], &[], span);
                for member in associated.iter_mut() {
                    self.parameterize(&mut member.params, &mut [], &enclosing, span);
                }
                for method in methods.iter_mut() {
                    let custom = method.has_self
                        && method.self_convention != Some(mojito_ast::ast::ArgConvention::Out)
                        && mojito_ast::ast::allows_custom_self_type(&method.decorators);
                    self.parameterize_with_receiver(
                        &mut method.type_params,
                        &mut method.params,
                        method.self_type.as_mut().filter(|_| custom),
                        &enclosing,
                        span,
                    );
                }
            }
            StmtKind::Trait {
                methods,
                comptime_members,
                ..
            } => {
                for member in comptime_members.iter_mut() {
                    self.parameterize(&mut member.params, &mut [], &[], span);
                }
                for method in methods.iter_mut() {
                    self.parameterize(&mut method.type_params, &mut method.params, &[], span);
                }
            }
            _ => {}
        }
    }
}

/// What an application gives each declared parameter.
enum Slot {
    Absent,
    Bound(ParamArg),
    Unbound,
}

struct Slots(Vec<Slot>);

impl Slots {
    /// Each of `declared`'s slots under the application `args`; `None` when
    /// it leaves none unbound or does not fit the declaration.
    fn of(declared: &[TypeParam], args: &[ParamArg]) -> Option<Self> {
        let mut slots: Vec<Slot> = declared
            .iter()
            .map(|parameter| {
                if args.is_empty() && !parameter.name.starts_with('$') {
                    Slot::Unbound
                } else {
                    Slot::Absent
                }
            })
            .collect();
        let mut positions = declared
            .iter()
            .enumerate()
            .filter(|(_, parameter)| !parameter.infer_only && !parameter.keyword_only)
            .map(|(index, _)| index);
        for argument in args {
            match argument {
                ParamArg::Named { name, value } => {
                    let index = declared
                        .iter()
                        .position(|parameter| &parameter.name == name)?;
                    slots[index] = match hole(value) {
                        Some(Unbound::Hole) => Slot::Unbound,
                        Some(Unbound::Rest) => return None,
                        None => Slot::Bound((**value).clone()),
                    };
                }
                positional @ ParamArg::Value(_) => match hole(positional) {
                    // `...` leaves every remaining parameter unbound, a
                    // keyword-only one (`Pointer`'s `address_space`) too.
                    Some(Unbound::Rest) => {
                        let keywords = declared.iter().enumerate().filter(|(_, parameter)| {
                            parameter.keyword_only && !parameter.infer_only
                        });
                        for index in positions.by_ref().chain(keywords.map(|(index, _)| index)) {
                            if matches!(slots[index], Slot::Absent) {
                                slots[index] = Slot::Unbound;
                            }
                        }
                    }
                    Some(Unbound::Hole) => slots[positions.next()?] = Slot::Unbound,
                    None => slots[positions.next()?] = Slot::Bound(positional.clone()),
                },
            }
        }
        slots
            .iter()
            .any(|slot| matches!(slot, Slot::Unbound))
            .then_some(Self(slots))
    }
}

/// A partial alias's arguments `alias_args` applied to `args`
/// (`comptime S = V[_, length=1]` spelled `S[_]` or `S[dt]`): each
/// positional argument fills the alias's next hole. `None` when `args`
/// spell a keyword or outnumber the holes.
fn fill_holes(alias_args: &[ParamArg], args: &[ParamArg]) -> Option<Vec<ParamArg>> {
    if args
        .iter()
        .any(|argument| matches!(argument, ParamArg::Named { .. }))
    {
        return None;
    }
    let mut applied = args.iter();
    let filled = alias_args
        .iter()
        .map(|argument| match hole(argument) {
            Some(Unbound::Hole) => applied.next().unwrap_or(argument).clone(),
            _ => argument.clone(),
        })
        .collect();
    applied.next().is_none().then_some(filled)
}

/// The infer-only sibling of `declared` that origin parameter `parameter`'s
/// mutability names (`mut` of `origin: Origin[mut=mut]`).
fn mutability_sibling<'p>(
    parameter: &TypeParam,
    declared: &'p [TypeParam],
) -> Option<&'p TypeParam> {
    let Some(Expr {
        kind: ExprKind::Identifier(name),
        ..
    }) = &parameter.origin_mutability
    else {
        return None;
    };
    declared
        .iter()
        .find(|sibling| sibling.infer_only && &sibling.name == name)
}

fn hole(argument: &ParamArg) -> Option<Unbound> {
    mojito_ast::call::unbound_marker(argument)
}

fn identifier(name: &str, span: Span) -> ParamArg {
    ParamArg::Value(Expr::new(ExprKind::Identifier(name.to_string()), span))
}

/// The infer-only binder `name` for the struct parameter `parameter`, its
/// type reading each earlier parameter as its argument in `arguments`.
fn binder_for(
    parameter: &TypeParam,
    name: &str,
    arguments: &HashMap<String, ParamArg>,
    span: Span,
) -> TypeParam {
    let mut substitute = Substitute { arguments, span };
    let mut read = |annotation: &Expr| {
        let mut annotation = annotation.clone();
        mojito_ast::visit::walk_annotation_names_mut(
            &mut substitute,
            &mut annotation,
            &mut Substitute::at_name,
        );
        annotation
    };
    let value_type = parameter.value_type.as_ref().map(&mut read);
    let callable_bound = parameter.callable_bound.as_ref().map(&mut read);
    let origin_mutability = parameter.origin_mutability.as_ref().map(|mutability| {
        let mut mutability = mutability.clone();
        mojito_ast::visit::walk_expr_mut(&mut substitute, &mut mutability);
        mutability
    });
    let bounds = parameter
        .bounds
        .iter()
        .map(|bound| {
            let ExprKind::Identifier(name) = &bound.kind else {
                return bound.clone();
            };
            match arguments.get(name) {
                Some(ParamArg::Value(Expr {
                    kind: ExprKind::Identifier(argument),
                    ..
                })) => Expr::new(ExprKind::Identifier(argument.clone()), bound.span),
                _ => bound.clone(),
            }
        })
        .collect();
    TypeParam {
        name: name.to_string(),
        bounds,
        value_type,
        callable_bound,
        origin_mutability,
        infer_only: true,
        keyword_only: false,
        default: None,
    }
}

/// Reads each struct parameter a binder's type names as its argument.
struct Substitute<'a> {
    arguments: &'a HashMap<String, ParamArg>,
    span: Span,
}

impl Substitute<'_> {
    /// A name a binder's annotation spells as a type (`T`, `Self.T`), read
    /// as the argument of the parameter it names when that argument spells
    /// a type: a spread pack, a name, or a type spelling.
    fn at_name(&mut self, name: &mut Expr) {
        let parameter = match &name.kind {
            ExprKind::Identifier(parameter) => parameter.as_str(),
            _ => match name.self_member() {
                Some(parameter) => parameter,
                None => return,
            },
        };
        if let Some(ParamArg::Value(argument)) = self.arguments.get(parameter)
            && (matches!(argument.kind, ExprKind::Spread(_) | ExprKind::Identifier(_))
                || spells_type(argument))
        {
            let span = name.span;
            *name = argument.clone();
            name.span = span;
        }
    }
}

impl MutVisitor for Substitute<'_> {
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        // A spread pack's operand has already read the argument pack's own
        // spread; the spread names that pack.
        if let ExprKind::Spread(operand) = &mut expr.kind
            && let ExprKind::Spread(pack) = &mut operand.kind
        {
            let pack = std::mem::replace(&mut **pack, Expr::new(ExprKind::None, self.span));
            **operand = pack;
            return;
        }
        let ExprKind::Identifier(name) = &expr.kind else {
            return;
        };
        if let Some(ParamArg::Value(value)) = self.arguments.get(name) {
            *expr = value.clone();
        }
    }
}

/// Whether an expression spells a type rather than a value: `Self` or a
/// name that is not lowercase, a bracket applying such a name (`S[Int]`),
/// or a member chain rooted at one (`M.Inner`, `Self.T`).
fn spells_type(expr: &Expr) -> bool {
    let type_name = |name: &str| {
        name.chars()
            .next()
            .is_some_and(|first| !first.is_lowercase())
    };
    match &expr.kind {
        ExprKind::Identifier(name) => name == "Self" || type_name(name),
        ExprKind::Member { object, .. } => spells_type(object),
        _ => expr
            .bracket_application()
            .is_some_and(|(name, _)| type_name(name)),
    }
}
