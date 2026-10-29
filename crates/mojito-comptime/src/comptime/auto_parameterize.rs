//! Upstream's automatic parameterization (`addImplicitTypeParams`,
//! `MojoParser/Signatures.cpp`): each parameter an argument's struct type
//! leaves unbound becomes an infer-only parameter of the declaration that
//! takes the argument.

use mojito_ast::ast::{FnParam, ParamArg};
use mojito_ast::visit::MutVisitor;

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

/// Declare an infer-only binder for each parameter that an argument's struct
/// type leaves unbound, and bind the type to the binders, on a `def` (one
/// nested in a body too), a struct method, a trait requirement, and a
/// struct's or a `def`'s own compile-time parameter of a struct type (`[t:
/// T]`).
///
/// A parameter is unbound when the struct is spelled bare (`x: List`, every
/// one of its parameters, a type pack as a pack binder), as `_` (`List[_]`,
/// `Pair[n=_]`), or under `...` (`Pair[Int, ...]`, each remaining
/// positional one). A module alias over a
/// partially bound struct (`comptime I32 = SIMD[DType.int32, _]`) spelled
/// bare stands for its struct with its holes. The struct is the program's
/// own or the `installed` package's, whose declared parameters give each
/// binder its type, with the struct's earlier parameters read as their
/// arguments. The binders follow the declaration's own infer-only
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
    /// Every other name the program declares, which hides a package struct.
    others: HashSet<String>,
    /// Each module alias whose body applies a struct with a hole, as the
    /// struct and its arguments.
    partial_aliases: HashMap<String, (String, Vec<ParamArg>)>,
    installed: Option<&'a InstalledInterfaces>,
}

impl<'a> Declared<'a> {
    fn of(program: &[Stmt], installed: Option<&'a InstalledInterfaces>) -> Self {
        let mut declared = Self {
            structs: HashMap::new(),
            others: HashSet::new(),
            partial_aliases: HashMap::new(),
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
                    if let ExprKind::TypeApply {
                        name: applied,
                        args,
                    } = &value.kind
                        && type_params.is_empty()
                        && args.iter().any(|argument| hole(argument).is_some())
                    {
                        declared
                            .partial_aliases
                            .insert(name.clone(), (applied.clone(), args.clone()));
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

    /// The parameters struct `name` declares, unless another declaration
    /// of the program hides it.
    fn parameters(&self, name: &str) -> Option<&[TypeParam]> {
        if let Some(parameters) = self.structs.get(name) {
            return Some(parameters);
        }
        if self.others.contains(name) {
            return None;
        }
        self.installed?.struct_parameters(name)
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
        let in_scope: HashSet<String> = type_params
            .iter()
            .map(|parameter| &parameter.name)
            .chain(enclosing)
            .map(|name| name.trim_start_matches('*').to_string())
            .collect();
        let mut binders = Vec::new();
        for parameter in type_params.iter_mut() {
            if parameter.callable_bound.is_some() || parameter.origin_mutability.is_some() {
                continue;
            }
            let spelled = match (&parameter.value_type, parameter.bounds.as_slice()) {
                (Some(ty), _) => ty.clone(),
                (None, [only]) => Type::Named(only.clone(), Vec::new()),
                _ => continue,
            };
            if let Some(bound) = self.bind(&spelled, &parameter.name, &in_scope, &mut binders, span)
            {
                parameter.value_type = Some(bound);
            }
        }
        for parameter in params.iter_mut() {
            if let Some(bound) = self.bind(
                &parameter.ty,
                &parameter.name,
                &in_scope,
                &mut binders,
                span,
            ) {
                parameter.ty = bound;
            }
        }
        let at = type_params
            .iter()
            .take_while(|parameter| parameter.infer_only)
            .count();
        type_params.splice(at..at, binders);
    }

    /// The type `ty`, the type of `owner`, with each parameter it leaves
    /// unbound bound to a binder pushed onto `binders`; `None` when it
    /// leaves none unbound or names no struct.
    fn bind(
        &self,
        ty: &Type,
        owner: &str,
        in_scope: &HashSet<String>,
        binders: &mut Vec<TypeParam>,
        span: Span,
    ) -> Option<Type> {
        let Type::Named(name, args) = ty else {
            return None;
        };
        if in_scope.contains(name) {
            return None;
        }
        let (name, args) = match self.partial_aliases.get(name) {
            Some((applied, alias_args)) if args.is_empty() => (applied, alias_args),
            _ => (name, args),
        };
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
                    identifier("_", span)
                }
                // A type pack's binder is a pack its argument spreads.
                Slot::Unbound if let Some(pack) = parameter.name.strip_prefix('*') => {
                    let binder = format!("*${pack}_{owner}");
                    binders.push(binder_for(parameter, &binder, &arguments, span));
                    ParamArg::Type(Type::Named(binder, Vec::new()))
                }
                Slot::Unbound => {
                    let binder = format!("${}_{owner}", parameter.name);
                    binders.push(binder_for(parameter, &binder, &arguments, span));
                    identifier(&binder, span)
                }
            };
            arguments.insert(
                parameter.name.trim_start_matches('*').to_string(),
                argument.clone(),
            );
            if parameter.infer_only || !in_order {
                keyword.push(ParamArg::Named {
                    name: parameter.name.clone(),
                    value: Box::new(argument),
                });
            } else {
                positional.push(argument);
            }
        }
        // An origin is erased from a type's identity, so a type that leaves
        // only origins unbound already takes any.
        if binders.len() == declared_binders {
            return None;
        }
        positional.extend(keyword);
        Some(Type::Named(name.clone(), positional))
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
            StmtKind::Struct {
                type_params,
                methods,
                ..
            } => {
                let enclosing: Vec<String> = type_params.iter().map(|p| p.name.clone()).collect();
                self.parameterize(type_params, &mut [], &[], span);
                for method in methods.iter_mut() {
                    self.parameterize(
                        &mut method.type_params,
                        &mut method.params,
                        &enclosing,
                        span,
                    );
                }
            }
            StmtKind::Trait { methods, .. } => {
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
            .filter(|(_, parameter)| !parameter.infer_only)
            .map(|(index, _)| index);
        for argument in args {
            match argument {
                ParamArg::Named { name, value } => {
                    let index = declared
                        .iter()
                        .position(|parameter| &parameter.name == name)?;
                    slots[index] = match hole(value) {
                        Some(Hole::One) => Slot::Unbound,
                        Some(Hole::Rest) => return None,
                        None => Slot::Bound((**value).clone()),
                    };
                }
                positional => match hole(positional) {
                    Some(Hole::Rest) => {
                        for index in positions.by_ref() {
                            if matches!(slots[index], Slot::Absent) {
                                slots[index] = Slot::Unbound;
                            }
                        }
                    }
                    Some(Hole::One) => slots[positions.next()?] = Slot::Unbound,
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

/// An unbound parameter argument's spelling.
enum Hole {
    /// `_`, one parameter.
    One,
    /// `...`, every remaining positional parameter.
    Rest,
}

fn hole(argument: &ParamArg) -> Option<Hole> {
    let (ParamArg::Value(Expr {
        kind: ExprKind::Identifier(name),
        ..
    })
    | ParamArg::Type(Type::Named(name, _))) = argument
    else {
        return None;
    };
    match name.as_str() {
        "_" => Some(Hole::One),
        "..." => Some(Hole::Rest),
        _ => None,
    }
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
    let mut read = |ty: &Type| {
        let mut ty = ty.clone();
        mojito_ast::visit::walk_type_mut(&mut substitute, &mut ty);
        ty
    };
    let value_type = parameter.value_type.as_ref().map(&mut read);
    let callable_bound = parameter.callable_bound.as_ref().map(&mut read);
    let bounds = parameter
        .bounds
        .iter()
        .map(|bound| match arguments.get(bound) {
            Some(ParamArg::Value(Expr {
                kind: ExprKind::Identifier(argument),
                ..
            })) => argument.clone(),
            _ => bound.clone(),
        })
        .collect();
    TypeParam {
        name: name.to_string(),
        bounds,
        value_type,
        callable_bound,
        origin_mutability: None,
        infer_only: true,
        default: None,
        constraints: Vec::new(),
    }
}

/// Reads each struct parameter a binder's type names as its argument.
struct Substitute<'a> {
    arguments: &'a HashMap<String, ParamArg>,
    span: Span,
}

impl MutVisitor for Substitute<'_> {
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        let ExprKind::Identifier(name) = &expr.kind else {
            return;
        };
        let replacement = match self.arguments.get(name) {
            Some(ParamArg::Value(value)) => Some(value.clone()),
            Some(ParamArg::Type(ty)) => ty.expression_spelling(self.span),
            _ => None,
        };
        if let Some(replacement) = replacement {
            *expr = replacement;
        }
    }

    fn visit_type_mut(&mut self, ty: &mut Type) {
        let name = match ty {
            Type::Named(name, args) if args.is_empty() => name,
            Type::SelfParam(name) => name,
            _ => return,
        };
        let replacement = match self.arguments.get(name.as_str()) {
            Some(ParamArg::Type(argument)) => Some(argument.clone()),
            Some(ParamArg::Value(Expr {
                kind: ExprKind::Identifier(argument),
                ..
            })) => Some(Type::Named(argument.clone(), Vec::new())),
            Some(ParamArg::Value(value)) => value.type_spelling(),
            _ => None,
        };
        if let Some(replacement) = replacement {
            *ty = replacement;
        }
    }
}
