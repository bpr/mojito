//! What a struct type reads of its arguments: its lowered layout, and each
//! lifecycle member a body runs on a value of it.
//!
//! Upstream runs `RemoveUnusedParams` after `LowerLIT`, which has rewritten
//! each `!lit.struct<@Box, [n]>` into the structural `!kgen.struct` of its
//! substituted fields (`lowerStructType`): a struct argument that no field
//! type spells is gone from every type the pass walks, and a pointer
//! field's pointee is `none` (`PointerType::get(noneType, …)`). A struct argument stays a read where a member
//! runs, since its call passes the struct's arguments as parameter values;
//! a member the struct does not declare is synthesized from its fields'
//! members, so it reads what they read.
//!
//! Mojito's MIR keeps the nominal type, so this module answers both
//! questions over the declarations, and `unread_parameters` walks a struct
//! type's arguments through them. A struct declaration carries no
//! parametric alignment or register-passability today; one that did would
//! read what it names as well.

use crate::mir::{MirDeclarations, has_no_destructor};
use mojito_types::types::{ParamDecl, Ty, TyArg, TyKind, names_binder};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};

/// The parameters of a struct, by position, that a question about it reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutReads {
    /// Every argument: the struct is unknown, holds a variadic pack, or
    /// contains itself.
    All,
    /// The positions read.
    Positions(BTreeSet<usize>),
}

impl LayoutReads {
    /// Whether the argument at `position` is read.
    pub fn reads(&self, position: usize) -> bool {
        match self {
            Self::All => true,
            Self::Positions(positions) => positions.contains(&position),
        }
    }
}

/// The positions of struct `name`'s parameters that its lowered layout
/// reads.
///
/// Those are the ones its field types spell, where a struct field reads
/// only the arguments its own layout reads and a pointer field nothing of
/// its pointee. A pointer elsewhere in a field, and a reference field, read
/// their pointee in full.
pub fn struct_layout_reads(name: &str, declarations: &MirDeclarations) -> LayoutReads {
    StructReads::new(declarations).layout(name)
}

/// The lifecycle members a drop, a copy, and a move run, which a struct
/// declaration's member table (`StructDeclaration::members`) records.
pub const LIFECYCLE_MEMBERS: [&str; 3] = ["__deinit__", "__copyinit__", "__moveinit__"];

/// The members a backend reaches from a value's type, not from a call
/// naming them.
///
/// A concrete struct declaration's member table records them beside its
/// lifecycle members: display (`write_to`, `write_repr_to`, a
/// writer's `write_string`), hashing (`__hash__`, a hasher's
/// `_update_with_simd`), equality (`__eq__`, and `__ne__` through it), the
/// `__int__` a scalar construction converts through, a keyword collector's
/// `__setitem__`, and the `__call__` a callable value dispatches.
pub const DISPATCHED_MEMBERS: [&str; 10] = [
    "write_to",
    "write_repr_to",
    "write_string",
    "__hash__",
    "_update_with_simd",
    "__eq__",
    "__ne__",
    "__int__",
    "__setitem__",
    "__call__",
];

/// The constructors a backend runs from a type with no call naming them.
///
/// Those are a keyword collector's empty one and a slice bound's `Optional`
/// ones. A constructor a call reaches is the instance the call names, never
/// a table entry.
pub const CONSTRUCTED_MEMBERS: [&str; 1] = ["__init__"];

/// The member a struct's member table records the method `method` under
/// (its spelling before any `$` qualifier), when it is a lifecycle,
/// dispatched, or constructed member.
pub fn table_member(method: &str) -> Option<&'static str> {
    let base = method.split('$').next().unwrap_or(method);
    LIFECYCLE_MEMBERS
        .into_iter()
        .chain(DISPATCHED_MEMBERS)
        .chain(CONSTRUCTED_MEMBERS)
        .find(|member| *member == base)
}

/// [`struct_layout_reads`] and the member reads of one walk's struct types,
/// each struct answered once.
pub struct StructReads<'d> {
    declarations: &'d MirDeclarations,
    layouts: RefCell<HashMap<String, LayoutReads>>,
    members: RefCell<HashMap<(String, &'static str), MemberNeeds>>,
}

impl<'d> StructReads<'d> {
    pub fn new(declarations: &'d MirDeclarations) -> Self {
        Self {
            declarations,
            layouts: RefCell::new(HashMap::new()),
            members: RefCell::new(HashMap::new()),
        }
    }

    /// [`struct_layout_reads`] of `name`.
    pub fn layout(&self, name: &str) -> LayoutReads {
        self.layout_in(name, &mut Vec::new())
    }

    /// The parts of `ty` that running its lifecycle `member` reads in full:
    /// a struct argument a declared member takes, a type parameter whose
    /// values it runs the member on, and anything this cannot see into.
    /// Empty when the member runs no code that reads a parameter, as for a
    /// struct with no declared member over fields with none.
    pub fn member_reads(&self, ty: &Ty, member: &'static str) -> Vec<TyArg> {
        self.member_parts(ty, member, &mut Vec::new())
            .into_iter()
            .map(|(part, _)| part)
            .collect()
    }

    fn layout_in(&self, name: &str, visiting: &mut Vec<String>) -> LayoutReads {
        if let Some(reads) = self.layouts.borrow().get(name) {
            return reads.clone();
        }
        let Some(structure) = self.declarations.structure(name) else {
            return LayoutReads::All;
        };
        if structure.param_decls.iter().any(is_variadic) || visiting.iter().any(|seen| seen == name)
        {
            return LayoutReads::All;
        }
        visiting.push(name.to_string());
        // A pointer field's pointee lowers to `none`.
        let parts: Vec<TyArg> = structure
            .fields
            .iter()
            .filter(|(_, field)| !matches!(field.kind(), TyKind::Pointer { .. }))
            .flat_map(|(_, field)| self.layout_parts(field, visiting))
            .collect();
        visiting.pop();
        let reads = LayoutReads::Positions(
            structure
                .param_decls
                .iter()
                .enumerate()
                .filter(|(_, decl)| parts.iter().any(|part| names(part, decl)))
                .map(|(position, _)| position)
                .collect(),
        );
        self.layouts
            .borrow_mut()
            .insert(name.to_string(), reads.clone());
        reads
    }

    /// The parts of the field type `ty` its layout reads in full.
    fn layout_parts(&self, ty: &Ty, visiting: &mut Vec<String>) -> Vec<TyArg> {
        match ty.kind() {
            TyKind::Struct(name, arguments) if !arguments.is_empty() => {
                let reads = self.layout_in(name, visiting);
                arguments
                    .iter()
                    .enumerate()
                    .filter(|(position, _)| reads.reads(*position))
                    .flat_map(|(_, argument)| match argument {
                        TyArg::Ty(ty) => self.layout_parts(ty, visiting),
                        other => vec![other.clone()],
                    })
                    .collect()
            }
            TyKind::Tuple(items) | TyKind::RuntimePack(items) | TyKind::Variant(items) => items
                .iter()
                .flat_map(|item| self.layout_parts(item, visiting))
                .collect(),
            _ => vec![TyArg::Ty(ty.clone())],
        }
    }

    /// What `name`'s `member` reads of each of its parameters, by position:
    /// `None` when it reads every one whole (a declared member, an unknown
    /// or variadic struct, or one that contains itself).
    fn member_needs(
        &self,
        name: &str,
        member: &'static str,
        visiting: &mut Vec<String>,
    ) -> MemberNeeds {
        let key = (name.to_string(), member);
        if let Some(needs) = self.members.borrow().get(&key) {
            return needs.clone();
        }
        let structure = self.declarations.structure(name)?;
        if structure.member(member).is_some()
            || structure.param_decls.iter().any(is_variadic)
            || visiting.iter().any(|seen| seen == name)
        {
            return None;
        }
        visiting.push(name.to_string());
        let parts: Vec<(TyArg, Need)> = structure
            .fields
            .iter()
            .flat_map(|(_, field)| self.member_parts(field, member, visiting))
            .collect();
        visiting.pop();
        let needs = Some(
            structure
                .param_decls
                .iter()
                .map(|decl| {
                    parts
                        .iter()
                        .filter(|(part, _)| names(part, decl))
                        .map(|(part, need)| match (part, need) {
                            (TyArg::Ty(ty), Need::Member)
                                if matches!(ty.kind(), TyKind::Param { binder, .. }
                                    if binder.id == *decl.id()) =>
                            {
                                Need::Member
                            }
                            _ => Need::Whole,
                        })
                        .max()
                })
                .collect(),
        );
        self.members.borrow_mut().insert(key, needs.clone());
        needs
    }

    /// The parts of `ty` running its `member` reads, each with whether the
    /// member runs on a value of it (a type parameter) or takes it whole.
    fn member_parts(
        &self,
        ty: &Ty,
        member: &'static str,
        visiting: &mut Vec<String>,
    ) -> Vec<(TyArg, Need)> {
        match ty.kind() {
            TyKind::Param { .. } => vec![(TyArg::Ty(ty.clone()), Need::Member)],
            TyKind::Struct(_, arguments) if arguments.is_empty() => Vec::new(),
            TyKind::Struct(name, arguments) => match self.member_needs(name, member, visiting) {
                None => arguments
                    .iter()
                    .map(|argument| (argument.clone(), Need::Whole))
                    .collect(),
                Some(needs) => arguments
                    .iter()
                    .enumerate()
                    .flat_map(|(position, argument)| {
                        match (
                            needs.get(position).copied().unwrap_or(Some(Need::Whole)),
                            argument,
                        ) {
                            (None, _) => Vec::new(),
                            (Some(Need::Member), TyArg::Ty(ty)) => {
                                self.member_parts(ty, member, visiting)
                            }
                            (Some(_), argument) => vec![(argument.clone(), Need::Whole)],
                        }
                    })
                    .collect(),
            },
            TyKind::Tuple(items)
            | TyKind::RuntimePack(items)
            | TyKind::Variant(items)
            | TyKind::Overload(items) => items
                .iter()
                .flat_map(|item| self.member_parts(item, member, visiting))
                .collect(),
            TyKind::ComptimeList(item) | TyKind::VariadicPack(item) => {
                self.member_parts(item, member, visiting)
            }
            TyKind::Pointer { .. } | TyKind::Ref(_) | TyKind::Never => Vec::new(),
            _ if has_no_destructor(ty) => Vec::new(),
            _ => vec![(TyArg::Ty(ty.clone()), Need::Whole)],
        }
    }
}

/// What a struct's lifecycle member reads of each of its parameters, by
/// position: `None` when it reads every one whole.
type MemberNeeds = Option<Vec<Option<Need>>>;

/// How a member reads one parameter of its struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Need {
    /// It runs the same member on a value of the type parameter.
    Member,
    /// It takes the argument whole.
    Whole,
}

const fn is_variadic(decl: &ParamDecl) -> bool {
    matches!(
        decl,
        ParamDecl::Type { variadic: true, .. } | ParamDecl::Value { variadic: true, .. }
    )
}

/// Whether the struct argument `part` names the binder `decl` declares.
fn names(part: &TyArg, decl: &ParamDecl) -> bool {
    let binder = decl.binder();
    match part {
        TyArg::Ty(ty) => names_binder(ty, &binder),
        TyArg::Val(_) => names_binder(
            &TyKind::Struct(String::new(), vec![part.clone()]).into(),
            &binder,
        ),
        TyArg::Origin(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::{MirFunctionDeclaration, MirStructDeclaration};
    use mojito_types::ct::CtValue;
    use mojito_types::origin::PointerOrigin;
    use mojito_types::param_expr::{MetaTy, ParamContext, ParamId, ParamRef};
    use mojito_types::types::{SimdDtype, SimdWidth};
    use std::collections::HashSet;

    fn value_decl(owner: &str, name: &str) -> ParamDecl {
        ParamDecl::Value {
            id: ParamId::new(owner, 0),
            name: name.to_string(),
            ty: Ty::int(),
            default: None,
            callable_default: None,
            infer_only: false,
            variadic: false,
            constraints: Vec::new(),
        }
    }

    fn type_decl(owner: &str, name: &str) -> ParamDecl {
        ParamDecl::Type {
            id: ParamId::new(owner, 0),
            name: name.into(),
            bounds: Vec::new(),
            callable_bound: None,
            default: None,
            infer_only: false,
            variadic: false,
            constraints: Vec::new(),
        }
    }

    fn structure(name: &str, decl: ParamDecl, fields: Vec<Ty>) -> MirStructDeclaration {
        MirStructDeclaration {
            name: name.into(),
            fields: fields
                .into_iter()
                .enumerate()
                .map(|(index, ty)| (format!("f{index}"), ty))
                .collect(),
            mut_self_methods: HashSet::new(),
            fieldwise_init: false,
            param_decls: vec![decl],
            explicit_destructors: HashMap::new(),
            explicit_destroy: None,
            conformances: Vec::new(),
            declared: Vec::new(),
            associated_types: Vec::new(),
            parameterized_associated_types: Vec::new(),
            associated_values: Vec::new(),
            members: Vec::new(),
        }
    }

    fn destructor(name: &str) -> MirFunctionDeclaration {
        MirFunctionDeclaration {
            lowered_name: format!("{name}.__deinit__"),
            param_names: Vec::new(),
            param_types: Vec::new(),
            defaults: Vec::new(),
            required: Vec::new(),
            variadic: None,
            variadic_convention: None,
            variadic_index: None,
            kw_variadic: None,
            kw_variadic_convention: None,
            kw_variadic_index: None,
            positional_only: None,
            keyword_only: None,
            param_decls: Vec::new(),
            has_receiver: true,
            receiver_convention: None,
            param_conventions: Vec::new(),
            ret_ty: Ty::none(),
            returns_reference: false,
            raises: false,
            error_ty: None,
            ref_params: Vec::new(),
            param_writes: Vec::new(),
            availability: Vec::new(),
            enclosing: None,
            region_binders: Vec::new(),
            inline: mojito_checked::declaration::InlineLevel::default(),
        }
    }

    /// A value argument naming the binder `name` of `owner`.
    fn value(owner: &str, name: &str) -> TyArg {
        let context = ParamContext::detached();
        TyArg::Val(CtValue::Expr(context.decl_ref(
            ParamId::new(owner, 0),
            name,
            MetaTy::int(),
        )))
    }

    fn param(owner: &str, name: &str) -> Ty {
        TyKind::Param {
            binder: ParamRef {
                id: ParamId::new(owner, 0),
                name: name.into(),
            },
            bounds: Vec::new(),
            callable_bound: None,
        }
        .into()
    }

    fn applied(name: &str, argument: TyArg) -> Ty {
        TyKind::Struct(name.into(), vec![argument]).into()
    }

    fn declarations() -> MirDeclarations {
        let width = ParamContext::detached().decl_ref(ParamId::new("Arr", 0), "n", MetaTy::int());
        MirDeclarations {
            structs: vec![
                structure("Box", value_decl("Box", "n"), vec![Ty::int()]),
                structure(
                    "Arr",
                    value_decl("Arr", "n"),
                    vec![
                        TyKind::Simd {
                            dtype: SimdDtype::Known(mojito_ast::ast::Dtype::Int64),
                            width: SimdWidth::Expr(width),
                        }
                        .into(),
                    ],
                ),
                structure(
                    "Holder",
                    type_decl("Holder", "T"),
                    vec![Ty::int(), param("Holder", "T")],
                ),
                structure("Inner", value_decl("Inner", "k"), vec![Ty::int()]),
                structure(
                    "Outer",
                    value_decl("Outer", "n"),
                    vec![applied("Inner", value("Outer", "n"))],
                ),
                structure(
                    "Outer2",
                    value_decl("Outer2", "n"),
                    vec![applied("Arr", value("Outer2", "n"))],
                ),
                structure(
                    "Link",
                    type_decl("Link", "T"),
                    vec![
                        TyKind::Pointer {
                            element: applied("Link", TyArg::Ty(param("Link", "T"))),
                            origin: PointerOrigin::Static,
                        }
                        .into(),
                        Ty::int(),
                    ],
                ),
                {
                    let mut nbox = structure("NBox", value_decl("NBox", "n"), vec![Ty::int()]);
                    nbox.record_member("__deinit__", "NBox.__deinit__");
                    nbox
                },
            ],
            functions: vec![destructor("NBox")],
            traits: Vec::new(),
            imported: None,
        }
    }

    fn positions(positions: &[usize]) -> LayoutReads {
        LayoutReads::Positions(positions.iter().copied().collect())
    }

    /// A struct's layout reads a parameter its fields spell, through a
    /// nested struct only where that struct's layout reads it, and never
    /// through a pointer field's pointee.
    #[test]
    fn struct_layout_reads_follow_lowered_fields() {
        let declarations = declarations();
        let reads = |name| struct_layout_reads(name, &declarations);
        assert_eq!(reads("Box"), positions(&[]));
        assert_eq!(reads("Arr"), positions(&[0]));
        assert_eq!(reads("Holder"), positions(&[0]));
        assert_eq!(reads("Outer"), positions(&[]));
        assert_eq!(reads("Outer2"), positions(&[0]));
        assert_eq!(reads("Link"), positions(&[]));
        assert_eq!(reads("Undeclared"), LayoutReads::All);
    }

    /// Dropping a value reads the arguments of a struct that declares its
    /// destructor whole, through a struct that does not only where a field
    /// runs one, and nothing of a struct whose destruction runs no code.
    #[test]
    fn destruction_reads_declared_destructors_only() {
        let declarations = declarations();
        let structs = StructReads::new(&declarations);
        let n = value("f", "n");
        let nbox = applied("NBox", n.clone());
        assert_eq!(structs.member_reads(&nbox, "__deinit__"), vec![n.clone()]);
        assert_eq!(
            structs.member_reads(&applied("Holder", TyArg::Ty(nbox)), "__deinit__"),
            vec![n.clone()]
        );
        let boxed = applied("Holder", TyArg::Ty(applied("Box", n)));
        assert!(structs.member_reads(&boxed, "__deinit__").is_empty());
        let generic = applied("Holder", TyArg::Ty(param("f", "T")));
        assert_eq!(
            structs.member_reads(&generic, "__deinit__"),
            vec![TyArg::Ty(param("f", "T"))]
        );
    }
}
