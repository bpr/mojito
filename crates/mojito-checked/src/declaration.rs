//! The checked declaration metadata of a lowered program.
//!
//! Each struct and callable's resolved signature, which MIR carries beside
//! its bodies, a backend consumes, and a package records once per
//! declaration.

use crate::checked::{CheckedConst, ExplicitDestroyInfo, StructConformance};
use mojito_ast::ast::{ArgConvention, CONTEXTUAL_SENTINEL, Decorator, ExprKind};
use mojito_common::error::TypeError;
use mojito_symbol::symbol::{
    OverloadFamily, OverloadSets, SignatureKey, TRAIT_DISPATCH, method_symbol, overload_family,
    split_method_symbol,
};
use mojito_types::origin::OriginParamId;
use mojito_types::param_expr::{ParamExpr, ParamRef};
use mojito_types::types::{GenericConstraint, ParamDecl, Ty};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// A package stores each one as a bytecode record, its sets and maps
/// written sorted, so equal declarations encode equal bytes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StructDeclaration {
    /// A package record leaves it to its declaration's directory row, so
    /// no other encoding may rely on it.
    #[serde(skip)]
    pub name: String,
    pub fields: Vec<(String, Ty)>,
    #[serde(serialize_with = "sorted_names")]
    pub mut_self_methods: HashSet<String>,
    pub fieldwise_init: bool,
    pub param_decls: Vec<ParamDecl>,
    /// The struct's named destructors (`deinit self` methods), beside
    /// whether each raises.
    #[serde(serialize_with = "sorted_map")]
    pub explicit_destructors: HashMap<String, bool>,
    /// The struct's explicit-destruction obligation, when it is not
    /// `Deinitable`.
    pub explicit_destroy: Option<ExplicitDestroyObligation>,
    /// The struct's synthesized answer for each lifecycle trait and
    /// `IsTrivially*` spelling (`mojito_types::conformance::LIFECYCLE_TRAITS`),
    /// sorted by trait: an instance conforms when its arguments satisfy any
    /// of the row's conditions. A concrete instance's rows are decided, so
    /// each holds `Bool(true)` or nothing.
    pub conformances: Vec<StructConformance>,
    /// The struct's declared conformances, in declaration order, each with
    /// its condition (`Bool(true)` when it states none). A conformance to
    /// any other trait is decided from them when a clause asks it, by
    /// `mojito_types::conformance::struct_trait_rule`. A concrete
    /// instance's are decided, likewise.
    pub declared: Vec<StructConformance>,
    /// The struct's unparameterized associated types (`Element` of `List`),
    /// sorted by member, each over the struct's own binders: the elaborator
    /// solves `C.Element` from them once `C` is bound to an instance, so
    /// elaborated MIR carries none. Members with parameters of their own are
    /// [`Self::parameterized_associated_types`].
    pub associated_types: Vec<(String, Ty)>,
    /// The struct's parameterized associated types (`IteratorType[o]` of
    /// `List`), sorted by member: the elaborator answers
    /// `C.IteratorType[origin_of(xs)]` from them once `C` is bound to an
    /// instance, so elaborated MIR carries none.
    pub parameterized_associated_types: Vec<ParameterizedAssociatedType>,
    /// The struct's compile-time value members (`K` of `comptime K = 7`),
    /// sorted by member, each a parameter expression over the struct's own
    /// binders: the elaborator answers `T.K` from them once `T` is bound to
    /// an instance, so elaborated MIR carries none.
    pub associated_values: Vec<(String, ParamExpr)>,
    /// The struct's parameterized compile-time value members (`get` of
    /// `comptime get[idx: Int]: Int = Self.values[idx]`), sorted by member:
    /// the elaborator answers `T.get[0]` from them once `T` is bound to an
    /// instance, so elaborated MIR carries none.
    pub parameterized_associated_values: Vec<ParameterizedAssociatedValue>,
    /// The symbols of the members the struct owns that a backend finds
    /// from a value's type, sorted by key, as upstream's struct nests its
    /// member functions: in a template, the lifecycle members it declares
    /// (its destructor, copy, and move constructors); in a concrete struct,
    /// every instance of a lifecycle or dispatched member
    /// (`mojito_mir::mir::DISPATCHED_MEMBERS`), other special method, or
    /// trait requirement (`MirDeclarations::reached_from_type`) the
    /// elaborator made for it, as upstream's witness tables name a
    /// conformance's members. A drop, copy, move, display, hash, or other
    /// type-reached dispatch of a value finds the member it runs here,
    /// never by composing a name from the value's type, and
    /// `eliminate-duplicate-functions` points an entry at the instance a
    /// merged member became.
    pub members: Vec<StructMember>,
    /// The witness table of each conformance the struct declares to a
    /// trait with method requirements, sorted by trait.
    ///
    /// Upstream's struct conformance carries its witness table: a call
    /// through a bound whose receiver a substitution made this struct names
    /// the member the table records for its requirement. A template carries
    /// them; an elaborated instance carries none, its [`Self::members`]
    /// naming the instances.
    pub witness_tables: Vec<WitnessTable>,
}

/// An associated type with explicit parameters of its own (`comptime
/// IteratorType[mut: Bool, //, o: Origin[mut=mut]] = _ListIter[Self.T, o]`):
/// its template over the struct's binders and its own.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ParameterizedAssociatedType {
    pub name: String,
    /// The member's explicit parameters, in application order.
    pub params: Vec<AssociatedParameter>,
    pub template: Ty,
}

/// A compile-time value member with parameters of its own, upstream's
/// generator.
///
/// `comptime get[idx: Int]: Int = Self.values[idx]` is its body over the
/// struct's binders and its own `params`, every one of them in declaration
/// order, which an application binds by position.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ParameterizedAssociatedValue {
    pub name: String,
    pub params: Vec<ParamDecl>,
    pub body: ParamExpr,
}

/// One explicit parameter of a [`ParameterizedAssociatedType`], keyed as its
/// template names it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AssociatedParameter {
    /// A type parameter, by its binder.
    Type(ParamRef),
    /// A value parameter, by its spelling.
    Value(String),
    /// An origin parameter, by the id its template's `Origin::Param` holds.
    Origin(OriginParamId),
}

/// One entry of [`StructDeclaration::members`]: a member's key and the
/// symbol of the function that is it.
///
/// The key is the template's method spelling with any overload qualifier
/// (`__deinit__`, `write_to`, `__eq__$ov$Self`); a generic member has one
/// entry per instance under one key. The symbol is a definition the struct
/// owns, not a use of one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StructMember {
    pub member: String,
    pub symbol: String,
}

impl StructDeclaration {
    /// The symbol of the struct's member keyed `member`, when it owns one:
    /// the first, for a generic member with several instances.
    pub fn member(&self, member: &str) -> Option<&str> {
        self.members
            .iter()
            .find(|entry| entry.member == member)
            .map(|entry| entry.symbol.as_str())
    }

    /// The entries of the member `base` (`__eq__`), each overload and
    /// instance of it: those whose key's spelling before any `$` qualifier
    /// is `base`.
    pub fn members_of<'s>(&'s self, base: &str) -> impl Iterator<Item = &'s StructMember> {
        self.members
            .iter()
            .filter(move |entry| entry.member.split('$').next().unwrap_or(&entry.member) == base)
    }

    /// Record `symbol` under the key `member`, keeping the table sorted by
    /// key, then symbol; an entry already there stays.
    pub fn record_member(&mut self, member: &str, symbol: &str) {
        if let Err(position) = self.members.binary_search_by(|entry| {
            (entry.member.as_str(), entry.symbol.as_str()).cmp(&(member, symbol))
        }) {
            self.members.insert(
                position,
                StructMember {
                    member: member.to_string(),
                    symbol: symbol.to_string(),
                },
            );
        }
    }

    /// The parameter names of the struct's synthesized `@fieldwise_init`
    /// constructor, its field names in order; `None` when it has none.
    pub fn fieldwise_parameters(&self) -> Option<Vec<String>> {
        self.fieldwise_init
            .then(|| self.fields.iter().map(|(name, _)| name.clone()).collect())
    }

    /// The struct's explicit-destruction obligation, its destructors
    /// joined, as the check analyzes one.
    pub fn explicit_destroy_info(&self) -> Option<ExplicitDestroyInfo> {
        self.explicit_destroy
            .as_ref()
            .map(|obligation| ExplicitDestroyInfo {
                message: obligation.message.clone(),
                declared: obligation.declared,
                destructors: self.explicit_destructors.clone(),
                fields: obligation.fields.clone(),
            })
    }

    /// The member witnessing the requirement whose dispatch symbol is
    /// `requirement` (`__trait_dispatch.__len__$ov$SelfRead`).
    ///
    /// `None` when no declared conformance's table records it. Every table
    /// that records the requirement names the same member, the dispatch
    /// symbol spelling its signature.
    pub fn witness(&self, requirement: &str) -> Option<&str> {
        self.witness_tables
            .iter()
            .flat_map(|table| &table.witnesses)
            .find(|witness| witness.requirement == requirement)
            .map(|witness| witness.member.as_str())
    }
}

/// One entry of [`StructDeclaration::witness_tables`].
///
/// It is the struct's conformance to `trait_name` and the member satisfying
/// each of its method requirements, its parents' included, sorted by
/// requirement.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WitnessTable {
    pub trait_name: String,
    pub witnesses: Vec<Witness>,
}

/// One requirement of a [`WitnessTable`] and the member satisfying it.
///
/// The requirement is the dispatch symbol a call through a bound selects it
/// by ([`TraitRequirement::dispatch`]); the member is the symbol of the
/// struct's written method or synthesized trait default.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Witness {
    pub requirement: String,
    pub member: String,
}

/// A trait declaration as a conformance decision and an overload lookup
/// read it: its name, the traits it refines, and its own requirements.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TraitDeclaration {
    /// A package record leaves it to its declaration's directory row, so
    /// no other encoding may rely on it.
    #[serde(skip)]
    pub name: String,
    /// Its parents, in declaration order.
    pub refines: Vec<String>,
    /// Its own requirements, defaulted or not, in declaration order, one
    /// per overload: a lookup of `Trait.method` finds every one, as
    /// upstream's trait declares each, and a defaulted one's member
    /// function is its body, not another declaration of the name.
    pub requirements: Vec<TraitRequirement>,
}

/// The declaration of the compiler-known trait `name` a witness table
/// records the requirements of.
///
/// They are `Defaultable`, as upstream's `std/builtin/value.mojo` declares
/// it, whose `__init__(out self)` takes no parameter, and the comparison
/// traits `Equatable` and `Comparable`, as its `std/builtin/comparable.mojo`
/// declares them, whose each requirement's parameter is `Self`.
///
/// Until the bundled library declares them, this is their declaration. A
/// program or package that declares a trait of the name shadows it.
pub fn compiler_trait_declaration(name: &str) -> Option<&'static TraitDeclaration> {
    static DECLARATIONS: LazyLock<[TraitDeclaration; 3]> = LazyLock::new(|| {
        let declaration =
            |name: &str, refines: &[&str], parameter: Option<&str>, methods: &[&str]| {
                let param_types: Vec<Ty> = parameter.iter().map(|_| Ty::self_type()).collect();
                TraitDeclaration {
                    name: name.to_string(),
                    refines: refines.iter().map(ToString::to_string).collect(),
                    requirements: methods
                        .iter()
                        .map(|method| TraitRequirement {
                            name: (*method).to_string(),
                            arity: param_types.len(),
                            dispatch: method_symbol(
                                TRAIT_DISPATCH,
                                method,
                                &SignatureKey::from_tys_with_self(&param_types, None),
                            ),
                            param_names: parameter.iter().map(ToString::to_string).collect(),
                            param_types: param_types.clone(),
                            param_conventions: vec![None; param_types.len()],
                            variadic_index: None,
                            required: vec![true; param_types.len()],
                            positional_only: None,
                            keyword_only: None,
                            variadic: None,
                            kw_variadic: None,
                            variadic_convention: None,
                        })
                        .collect(),
                }
            };
        [
            declaration("Defaultable", &[], None, &["__init__"]),
            declaration("Equatable", &[], Some("other"), &["__eq__", "__ne__"]),
            declaration(
                "Comparable",
                &["Equatable"],
                Some("rhs"),
                &["__lt__", "__gt__", "__le__", "__ge__"],
            ),
        ]
    });
    DECLARATIONS
        .iter()
        .find(|declaration| declaration.name == name)
}

/// One requirement of a trait declaration, a bodyless method declaration.
///
/// Upstream's trait declares each, so a call through a bound passes its
/// arguments by the requirement's conventions as a concrete call does by
/// its callee's.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TraitRequirement {
    pub name: String,
    /// How many parameters it spells, its receiver aside, as the overload
    /// scan counts them.
    pub arity: usize,
    /// The symbol a call through a bound selects it by
    /// (`__trait_dispatch.sh$ov$Int`).
    pub dispatch: String,
    pub param_names: Vec<String>,
    /// Its parameter types, `Self` spelled `TyKind::SelfType`.
    pub param_types: Vec<Ty>,
    pub param_conventions: Vec<Option<ArgConvention>>,
    pub variadic_index: Option<usize>,
    /// Its call shape beside the parameters: which of them a call must
    /// pass, where positional and keyword-only ones start, and its
    /// positional and keyword collectors' element types, by which a call
    /// through a bound binds its arguments and builds its collector.
    pub required: Vec<bool>,
    pub positional_only: Option<usize>,
    pub keyword_only: Option<usize>,
    pub variadic: Option<Ty>,
    pub kw_variadic: Option<Ty>,
    /// The positional collector's convention, which makes it an owned
    /// `VariadicList` when `var`.
    pub variadic_convention: Option<ArgConvention>,
}

/// A struct's explicit-destruction obligation, apart from its destructors,
/// which every struct may declare.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExplicitDestroyObligation {
    /// The `@explicit_destroy` message, or
    /// [`ExplicitDestroyInfo::default_message`].
    pub message: String,
    /// Whether `@explicit_destroy` declared `message`.
    pub declared: bool,
    /// Each direct field whose type carries its own obligation, beside that
    /// type's struct.
    #[serde(serialize_with = "sorted_map")]
    pub fields: HashMap<String, String>,
}

impl ExplicitDestroyObligation {
    /// `info` without its destructors.
    pub fn of(info: &ExplicitDestroyInfo) -> Self {
        Self {
            message: info.message.clone(),
            declared: info.declared,
            fields: info.fields.clone(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FunctionDeclaration {
    /// A package record leaves it to the function's directory row, so no
    /// other encoding may rely on it.
    #[serde(skip)]
    pub lowered_name: String,
    pub param_names: Vec<String>,
    pub param_types: Vec<Ty>,
    pub defaults: Vec<Option<CheckedConst>>,
    pub required: Vec<bool>,
    pub variadic: Option<Ty>,
    /// Declared convention of the positional variadic collector. Kept
    /// independently because it is not represented in `param_conventions`.
    pub variadic_convention: Option<ArgConvention>,
    pub variadic_index: Option<usize>,
    pub kw_variadic: Option<Ty>,
    /// Declared convention of the keyword variadic collector. Kept
    /// independently because it is not represented in `param_conventions`.
    pub kw_variadic_convention: Option<ArgConvention>,
    pub kw_variadic_index: Option<usize>,
    pub positional_only: Option<usize>,
    pub keyword_only: Option<usize>,
    pub param_decls: Vec<ParamDecl>,
    /// Whether this declaration has an implicit method receiver. This keeps a
    /// plain `self` (whose convention is `None`) distinct from a static/free
    /// callable with no receiver.
    pub has_receiver: bool,
    /// Declared convention of that implicit method receiver.
    pub receiver_convention: Option<ArgConvention>,
    /// Declared conventions of the explicit runtime parameters, aligned with
    /// `param_types`. Effective per-call `ref` access may narrow to `Read`.
    pub param_conventions: Vec<Option<ArgConvention>>,
    /// Checked return type of the callable.
    pub ret_ty: Ty,
    /// Whether the callable returns a reference handle to `ret_ty` rather than
    /// an owned value. The selected call carries the instantiated origin and
    /// mutability; this declaration fact verifies the ABI family.
    pub returns_reference: bool,
    /// Checked raising contract and its declared error type.
    pub raises: bool,
    pub error_ty: Option<Ty>,
    /// Whether each runtime parameter is a `mut`/`ref` reference whose final
    /// value writes back through a caller place. Same order as `param_types`.
    /// A package record derives it from `param_conventions`
    /// ([`Self::conventions_mask`]), so no other encoding may rely on it.
    #[serde(skip)]
    pub ref_params: Vec<bool>,
    /// Whether each runtime parameter writes through that retained caller
    /// place (`mut`/`var`/`deinit`, or a `ref` under a statically mutable
    /// origin contract); `ref_params` says only that it needs one. The loan
    /// analysis classifies a retained argument place by this mask. Same
    /// order as `param_types`.
    pub param_writes: Vec<bool>,
    /// A struct member's `where` clauses, one entry per clause, over the
    /// owner's and the member's binders. An instance whose arguments
    /// disprove one does not have the member. Empty on a concrete instance,
    /// whose clauses the elaborator decided.
    pub availability: Vec<GenericConstraint>,
    /// The lowered name of the function or method a nested function is
    /// declared in, whose binders (and its own enclosing declaration's) the
    /// nested body may name. `None` at top level and on a concrete instance.
    pub enclosing: Option<String>,
    /// The binders of the compile-time regions of the enclosing body that a
    /// nested function is declared in — the indices of its enclosing
    /// `comptime for` loops, outermost first — which its body and signature
    /// may name. The elaborator instantiates such a body once per unrolled
    /// copy of the region. Empty at top level and on a concrete instance.
    pub region_binders: Vec<ParamDecl>,
    /// The level its inline decorator names.
    pub inline: InlineLevel,
    /// Whether the source declares the function or the compiler
    /// synthesized it.
    pub provenance: FunctionProvenance,
}

impl FunctionDeclaration {
    /// Whether the compiler synthesized the function for a struct that
    /// declares none ([`FunctionProvenance::Synthesized`]).
    pub fn is_synthesized(&self) -> bool {
        self.provenance == FunctionProvenance::Synthesized
    }

    /// Whether the function is a closure's body, the method of its
    /// storage struct ([`mojito_symbol::symbol::closure_method_name`]).
    pub fn is_closure_method(&self) -> bool {
        self.has_receiver
            && mojito_symbol::symbol::split_method_symbol(&self.lowered_name)
                .is_some_and(|(owner, _)| mojito_symbol::symbol::is_closure_storage_name(owner))
    }

    /// How many parameters the function's source spells, its receiver
    /// aside: the runtime ones and each variadic collector.
    pub fn source_arity(&self) -> usize {
        self.param_types.len()
            + usize::from(self.variadic.is_some())
            + usize::from(self.kw_variadic.is_some())
    }

    /// Whether each runtime parameter is a `mut`/`ref` reference, as its
    /// declared convention says.
    pub fn conventions_mask(&self) -> Vec<bool> {
        self.param_conventions
            .iter()
            .map(|convention| ArgConvention::is_reference(*convention))
            .collect()
    }

    /// The parameter a body slot of the function binds, in the layout the
    /// call binding builds: the receiver first, then the regular
    /// parameters with the positional collector at `variadic_index` among
    /// them and the keyword collector at `kw_variadic_index` among both.
    pub fn slot_parameter(&self, slot: usize) -> SlotParameter {
        let Some(mut index) = slot.checked_sub(usize::from(self.has_receiver)) else {
            return SlotParameter::Receiver;
        };
        let positional = self
            .variadic
            .as_ref()
            .map(|_| self.variadic_index.unwrap_or(self.param_names.len()));
        if self.kw_variadic.is_some() {
            let keyword = self
                .kw_variadic_index
                .unwrap_or_else(|| self.param_names.len() + usize::from(positional.is_some()));
            if index == keyword {
                return SlotParameter::KwVariadic;
            }
            index -= usize::from(index > keyword);
        }
        if let Some(positional) = positional {
            if index == positional {
                return SlotParameter::Variadic;
            }
            index -= usize::from(index > positional);
        }
        SlotParameter::Position(index)
    }

    /// The overload set the function belongs to, beside its source arity;
    /// `None` for a nested or lifted function, which no lookup by name
    /// finds, and for a synthesized one, which the check declared in none.
    pub fn overload_member(&self) -> Option<(OverloadFamily<'_>, usize)> {
        if self.enclosing.is_some() || self.is_synthesized() {
            return None;
        }
        overload_family(&self.lowered_name).map(|family| (family, self.source_arity()))
    }
}

/// Where a function declaration comes from.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum FunctionProvenance {
    /// Written in source.
    #[default]
    Source,
    /// Synthesized by the compiler for a struct that declares none, as
    /// upstream synthesizes a `__del__` destroying the fields of a struct
    /// without one: it has no source body, no overload set holds it, and a
    /// backend traces no lifecycle event of it.
    Synthesized,
}

/// The parameter a body slot binds ([`FunctionDeclaration::slot_parameter`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotParameter {
    /// A method's receiver.
    Receiver,
    /// The regular parameter at this declaration position.
    Position(usize),
    /// The positional collector, `*args`.
    Variadic,
    /// The keyword collector, `**kwargs`.
    KwVariadic,
}

/// How a callable asks to be inlined, as upstream's function op records the
/// level its inline decorator names.
///
/// `@always_inline` is [`Self::Always`], `@always_inline("nodebug")`
/// [`Self::NoDebug`], `@always_inline("builtin")` [`Self::Builtin`], and
/// `@no_inline` [`Self::Never`]; `@inline(level)` names one of the
/// prelude `InlineLevel`'s members, spelled `.member` or
/// `InlineLevel.member`. A callable with none is [`Self::Automatic`].
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum InlineLevel {
    #[default]
    Automatic,
    Always,
    NoDebug,
    Builtin,
    Never,
}

impl InlineLevel {
    /// Whether `decorator` is one that names an inline level.
    pub fn names(decorator: &Decorator) -> bool {
        matches!(decorator.path.as_slice(),
            [name] if matches!(name.as_str(), "always_inline" | "inline" | "no_inline"))
    }

    /// The level `decorators` name, or the pin's diagnostic for an operand
    /// it rejects or for two decorators that name different levels.
    /// Decorators apply innermost first, so the later of two conflicting
    /// ones is reported against the earlier. An `@inline` operand other
    /// than a spelled member is a compile-time expression this level
    /// cannot hold, and is unsupported.
    pub fn of(decorators: &[Decorator]) -> Result<Self, TypeError> {
        let mut declared: Option<(Self, &str)> = None;
        for decorator in decorators.iter().rev() {
            let [name] = decorator.path.as_slice() else {
                continue;
            };
            let level = match name.as_str() {
                "always_inline" => {
                    Self::always_inline(decorator).map_err(TypeError::InvalidDecorator)?
                }
                "inline" => Self::inline(decorator)?,
                "no_inline" if decorator.args.is_empty() && decorator.kwargs.is_empty() => {
                    Self::Never
                }
                "no_inline" => {
                    return Err(TypeError::InvalidDecorator(
                        "'@no_inline' cannot have arguments".to_string(),
                    ));
                }
                _ => continue,
            };
            match declared {
                Some((previous, spelled)) if previous != level => {
                    return Err(TypeError::InvalidDecorator(format!(
                        "function has conflicting inline level from a previous '@{spelled}' \
                         decorator"
                    )));
                }
                Some(_) => {}
                None => declared = Some((level, name)),
            }
        }
        Ok(declared.map_or(Self::Automatic, |(level, _)| level))
    }

    /// The spelling MIR text gives it.
    pub const fn spelling(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Always => "always",
            Self::NoDebug => "nodebug",
            Self::Builtin => "builtin",
            Self::Never => "never",
        }
    }

    /// The level MIR text spells `spelling`.
    pub fn from_spelling(spelling: &str) -> Option<Self> {
        [
            Self::Automatic,
            Self::Always,
            Self::NoDebug,
            Self::Builtin,
            Self::Never,
        ]
        .into_iter()
        .find(|level| level.spelling() == spelling)
    }

    /// The level `@inline(level)` names: one positional operand, a member
    /// of `InlineLevel` spelled `.member` or `InlineLevel.member`.
    fn inline(decorator: &Decorator) -> Result<Self, TypeError> {
        let operands = decorator.args.len() + decorator.kwargs.len();
        if operands != 1 {
            return Err(TypeError::InvalidDecorator(format!(
                "'@inline' decorator takes exactly 1 argument, found {operands}"
            )));
        }
        let [operand] = decorator.args.as_slice() else {
            return Err(TypeError::InvalidDecorator(
                "'@inline' argument must be positional".to_string(),
            ));
        };
        let ExprKind::Member { object, field } = &operand.kind else {
            return Err(Self::unsupported_inline_operand());
        };
        if !matches!(&object.kind, ExprKind::Identifier(name)
            if name == CONTEXTUAL_SENTINEL || name == "InlineLevel")
        {
            return Err(Self::unsupported_inline_operand());
        }
        [Self::Automatic, Self::Always, Self::NoDebug, Self::Never]
            .into_iter()
            .find(|level| level.spelling() == field)
            .ok_or_else(|| {
                TypeError::InvalidDecorator(format!(
                    "'InlineLevel' value has no attribute '{field}'"
                ))
            })
    }

    fn unsupported_inline_operand() -> TypeError {
        TypeError::Unsupported(
            "an '@inline' level other than a spelled 'InlineLevel' member".to_string(),
        )
    }

    fn always_inline(decorator: &Decorator) -> Result<Self, String> {
        const OPERAND: &str = "'@always_inline' operand must be \"nodebug\" or \"builtin\"";
        if !decorator.kwargs.is_empty() {
            return Err(OPERAND.to_string());
        }
        match decorator.args.as_slice() {
            [] => Ok(Self::Always),
            [operand] => match &operand.kind {
                ExprKind::Str(level) if level == "nodebug" => Ok(Self::NoDebug),
                ExprKind::Str(level) if level == "builtin" => Ok(Self::Builtin),
                _ => Err(OPERAND.to_string()),
            },
            operands => Err(format!(
                "'@always_inline' decorator takes 0 or 1 arguments, found {}",
                operands.len()
            )),
        }
    }
}

/// The overload sets the declarations `functions`, `structs`, and
/// `traits` imply, as a lookup by name finds them.
///
/// A trait's requirement set is its declaration's, which its member
/// functions do not restate ([`OverloadSets::declared`]).
pub fn declared_overloads(
    functions: &[FunctionDeclaration],
    structs: &[StructDeclaration],
    traits: &[TraitDeclaration],
) -> OverloadSets {
    let requirements: Vec<(String, usize)> = traits
        .iter()
        .flat_map(|declaration| {
            declaration.requirements.iter().map(|requirement| {
                (
                    format!("{}.{}", declaration.name, requirement.name),
                    requirement.arity,
                )
            })
        })
        .collect();
    let is_trait = |owner: &str| traits.iter().any(|declaration| declaration.name == owner);
    OverloadSets::declared(
        functions
            .iter()
            .filter_map(FunctionDeclaration::overload_member)
            .filter(|(family, _)| match family {
                OverloadFamily::Method(name) => {
                    split_method_symbol(name).is_none_or(|(owner, _)| !is_trait(owner))
                }
                OverloadFamily::Free(_) => true,
            })
            .chain(
                requirements
                    .iter()
                    .map(|(name, arity)| (OverloadFamily::Method(name), *arity)),
            ),
        structs.iter().filter_map(|declaration| {
            declaration
                .fieldwise_parameters()
                .map(|names| (declaration.name.clone(), names))
        }),
    )
}

fn sorted_names<S: serde::Serializer>(
    names: &HashSet<String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serde::Serialize::serialize(
        &names.iter().collect::<std::collections::BTreeSet<_>>(),
        serializer,
    )
}

fn sorted_map<K: Ord + serde::Serialize, V: serde::Serialize, S: serde::Serializer>(
    map: &HashMap<K, V>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serde::Serialize::serialize(
        &map.iter().collect::<std::collections::BTreeMap<_, _>>(),
        serializer,
    )
}
