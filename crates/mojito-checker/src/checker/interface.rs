//! A package's declaration interfaces: the checker's resolved table rows
//! for each declaration a library module makes, by linked name.
//!
//! The package build's check opens one record per library declaration
//! after it has checked the whole library (`mojito_checked::package`): its
//! residue holds the declaration's rows, and the package's one directory
//! names every table key each holds. An importer's check attaches the
//! records of the modules its link reached instead of registering their
//! declarations from source, and resolves a residue from its bytecode the
//! first time a lookup its own tables miss asks for one of its keys, as
//! upstream's importer resolves a package declaration on first lookup. A
//! residue holds no binding owner, span-keyed fact, syntax identity, or
//! parameter hole: each is one check's or one process's, and encoding a
//! table that holds one fails.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::CheckedConst;
use mojito_checked::declaration::{FunctionDeclaration, StructDeclaration};
pub(super) use mojito_checked::package::InterfaceTable as Table;
use mojito_checked::package::{
    DeclarationRecord, DeclarationRecords, DeclaringModules, LoweredDeclaration,
    PackageDeclarations, PackagedModule,
};
use mojito_common::literal::IntLiteral;
use mojito_types::types::TyKind;
use mojito_types::types::{FuncTy, GenericFuncTy};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

/// A package's declaration records as the check finds them, read through
/// the package's directory, which indexes them by the checker keys they
/// answer; no residue decodes here.
pub struct DeclarationInterfaces {
    package: Arc<PackageDeclarations>,
}

impl std::fmt::Debug for DeclarationInterfaces {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let directory = self.package.directory();
        write!(
            formatter,
            "DeclarationInterfaces({} modules, {} declarations)",
            directory.module_count(),
            directory.declaration_count()
        )
    }
}

impl DeclarationInterfaces {
    /// The declarations of `package`, as the check looks them up.
    pub const fn new(package: Arc<PackageDeclarations>) -> Self {
        Self { package }
    }
}

/// What a package build's check exports of its library.
///
/// Each declaration's rows, which
/// [`Self::records`] encodes as the residue of the declaration's record
/// once its lowering is known, the overload sets its lowered declarations
/// must imply, and the effect summaries its lowered part holds. The
/// constants its lowered part holds are the check's integer rows, the one
/// fold of each.
pub struct InterfaceExport {
    declarations: BTreeMap<String, (DeclarationInterface, Vec<(Table, String)>)>,
    /// The library's overload sets, as its check scanned them.
    overloads: mojito_symbol::symbol::OverloadSets,
    declaring: DeclaringModules,
    /// The transfer effects of each module-level callable and method whose
    /// body the check summarized, by callable identity.
    transfer_effects: Vec<(String, Vec<mojito_checked::checked::TransferEffect>)>,
    /// Their call-through effects, likewise.
    call_through_effects: Vec<(String, Vec<mojito_checked::checked::CallThroughEffect>)>,
    /// The spelling of the type each library module type alias names,
    /// which its lowered part holds for an importer's symbols to spell.
    type_aliases: Vec<(String, SourceType)>,
}

impl std::fmt::Debug for InterfaceExport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "InterfaceExport({} declarations)",
            self.declarations.len()
        )
    }
}

impl InterfaceExport {
    /// The package's declaration records, under the module table `modules`
    /// the build's link recorded: `structs`, `functions`, and
    /// `traits`, the library's lowered declarations, and the check's
    /// constants and effect summaries, each in the record of the
    /// declaration it belongs to, beside that declaration's residue, which
    /// leaves out what its lowered declarations say.
    ///
    /// # Errors
    ///
    /// A lowered declaration, constant, or summary no library declaration
    /// owns, lowered declarations whose overload sets differ from the
    /// check's, a struct, method, or function whose lowered declaration
    /// says other than the check's rows, or a residue that does not
    /// encode.
    pub fn records(
        self,
        modules: Vec<PackagedModule>,
        structs: Vec<StructDeclaration>,
        functions: Vec<FunctionDeclaration>,
        traits: Vec<mojito_checked::declaration::TraitDeclaration>,
    ) -> Result<DeclarationRecords, String> {
        if let Some(name) =
            mojito_checked::declaration::declared_overloads(&functions, &structs, &traits)
                .differing_name(&self.overloads)
        {
            return Err(format!(
                "the library's lowered declarations say other than its check of the overloads of `{name}`"
            ));
        }
        let constants = self.checked_constants();
        let mut records = DeclarationRecords {
            modules,
            declaring: self.declaring,
            ..DeclarationRecords::default()
        };
        let mut unplaced: Vec<String> = structs
            .into_iter()
            .filter_map(|declaration| records.place_struct(declaration).err())
            .collect();
        unplaced.extend(
            functions
                .into_iter()
                .filter_map(|declaration| records.place_function(declaration).err()),
        );
        unplaced.extend(
            traits
                .into_iter()
                .filter_map(|declaration| records.place_trait(declaration).err()),
        );
        // An effect summary names its function by its index in the record,
        // so every function is placed first.
        unplaced.extend(
            self.transfer_effects
                .into_iter()
                .filter_map(|(callable, effects)| {
                    records.place_transfer_effects(&callable, effects).err()
                }),
        );
        unplaced.extend(
            constants
                .into_iter()
                .filter_map(|(name, value)| records.place_constant(&name, value).err()),
        );
        unplaced.extend(
            self.type_aliases
                .into_iter()
                .filter_map(|(name, spelling)| records.place_type_alias(&name, spelling).err()),
        );
        unplaced.extend(
            self.call_through_effects
                .into_iter()
                .filter_map(|(callable, effects)| {
                    records.place_call_through_effects(&callable, effects).err()
                }),
        );
        if !unplaced.is_empty() {
            return Err(unplaced.join("; "));
        }
        for (name, (mut interface, mut keys)) in self.declarations {
            interface.sort();
            keys.sort();
            keys.dedup();
            let mut methods: Vec<String> = interface
                .structs
                .iter()
                .flat_map(|(_, info)| info.methods.keys().cloned())
                .collect();
            methods.sort();
            methods.dedup();
            let module = records
                .declaring
                .module(&name)
                .ok_or_else(|| format!("`{name}` has no module record"))?
                .to_string();
            let record =
                records
                    .declarations
                    .entry(name.clone())
                    .or_insert_with(|| DeclarationRecord {
                        module,
                        ..DeclarationRecord::default()
                    });
            record.lowered.name.clone_from(&name);
            interface.project(&record.lowered)?;
            interface.refuse_unrestorable(&name, &keys, &record.lowered)?;
            record.residue = postcard::to_stdvec(&interface).map_err(|error| {
                format!("the interface of `{name}` cannot be exported: {error}")
            })?;
            record.keys = keys;
            record.methods = methods;
        }
        Ok(records)
    }

    /// Every library integer constant as the check folded it, sorted.
    fn checked_constants(&self) -> Vec<(String, IntLiteral)> {
        self.declarations
            .values()
            .flat_map(|(interface, _)| &interface.comptimes)
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<BTreeMap<_, _>>()
            .into_iter()
            .collect()
    }
}

/// The declaration interfaces of the package modules a compilation's link
/// reached, attached to every checker pass, whose struct names the
/// elaborator reads too.
///
/// A declaration's record decodes the first time a lookup asks for one of
/// its keys, once per compilation, since the expressions it holds are
/// stamped with the path its importer reached the module by.
#[derive(Clone)]
pub struct InstalledInterfaces(Arc<Installed>);

impl std::fmt::Debug for InstalledInterfaces {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "InstalledInterfaces({} modules)",
            self.0.reached_order.len()
        )
    }
}

impl InstalledInterfaces {
    /// The interfaces of `modules`, positions in the package's module table
    /// in the order the link reached them, from `package`. Nothing decodes
    /// here: a declaration's record decodes the first time a lookup names
    /// one of its keys.
    pub fn reaching(
        package: &Arc<DeclarationInterfaces>,
        modules: impl IntoIterator<Item = u32>,
    ) -> Self {
        let directory = package.package.directory();
        let count = directory.module_count();
        let mut reached = vec![false; count];
        let mut reached_order = Vec::new();
        let mut binding_bases = vec![None; count];
        let mut binding_count = 0u32;
        for module in modules {
            if reached.get(module as usize).is_none_or(|reached| *reached) {
                continue;
            }
            reached[module as usize] = true;
            reached_order.push(module);
            binding_bases[module as usize] = Some(binding_count);
            binding_count += directory.binding_count(module as usize) as u32;
        }
        let decoded = (0..directory.declaration_count())
            .map(|_| OnceLock::new())
            .collect();
        Self(Arc::new(Installed {
            package: Arc::clone(package),
            reached: reached.into(),
            reached_order,
            binding_bases,
            binding_count,
            decoded,
            failures: Mutex::new(Vec::new()),
            reached_failures: mojito_checked::package::ReachedFailures::default(),
        }))
    }

    /// The module constant `name` of a reached module, as the build's
    /// comptime elaboration left it, decoding its declaration's record on
    /// first use.
    pub fn elaborated(&self, name: &str) -> Option<&ElaboratedConstant> {
        let constants = &self.declaration(Table::Elaborated, name)?.elaborated;
        constants
            .binary_search_by(|(constant, _)| constant.as_str().cmp(name))
            .ok()
            .map(|position| &constants[position].1)
    }

    /// Whether a reached module declares struct `name`, decoding nothing.
    pub fn declares_struct(&self, name: &str) -> bool {
        self.has_key(Table::Structs, name)
    }

    /// The type the reached module's zero-parameter type alias `name`
    /// names, read off the check's `Aliases` row, decoding its
    /// declaration's record on first use.
    pub fn type_alias(&self, name: &str) -> Option<&Ty> {
        let aliases = &self.declaration(Table::Aliases, name)?.aliases;
        let position = aliases
            .binary_search_by(|(alias, _)| alias.as_str().cmp(name))
            .ok()?;
        let alias = &aliases[position].1;
        match &alias.body {
            AliasBody::Type(ty) if alias.decls.is_empty() => Some(ty),
            _ => None,
        }
    }

    /// The parents the reached trait `name` declares, decoding its
    /// declaration's record on first use.
    pub fn trait_parents(&self, name: &str) -> Option<&[String]> {
        self.declaration(Table::Traits, name)?
            .traits
            .iter()
            .find(|(trait_name, _)| trait_name == name)
            .map(|(_, info)| info.refines.as_slice())
    }

    /// The reached part of the package, whose declarations answer a name
    /// the program's own overload sets miss.
    pub fn package_scope(&self) -> mojito_checked::package::PackageScope {
        mojito_checked::package::PackageScope::new(
            Arc::clone(&self.0.package.package),
            Arc::clone(&self.0.reached),
            self.0.reached_failures.clone(),
        )
    }

    /// How many declaration records have decoded.
    pub fn decoded(&self) -> usize {
        self.0
            .decoded
            .iter()
            .filter(|record| record.get().is_some())
            .count()
    }

    /// Why each declaration record a lookup asked for, here or through
    /// [`Self::package_scope`], and that did not decode failed.
    pub fn failures(&self) -> Vec<String> {
        let mut failures = self
            .0
            .failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        failures.extend(self.0.reached_failures.messages(&self.0.package.package));
        failures
    }

    /// The record of the declaration `table`'s `key` belongs to, decoded on
    /// first use; `None` when no reached module declares the key, or its
    /// record does not decode (a failure [`Self::failures`] reports).
    pub(super) fn declaration(&self, table: Table, key: &str) -> Option<&DeclarationInterface> {
        let position = self.position(table, key)?;
        self.0.decoded[position]
            .get_or_init(|| {
                let package = &self.0.package.package;
                let directory = package.directory();
                let entry = directory.declaration(position)?;
                let path = directory.module(entry.module as usize)?.path;
                package
                    .decoding(|| {
                        mojito_ast::ast::decoding_from(path, || {
                            postcard::from_bytes::<DeclarationInterface>(package.residue(position))
                        })
                    })
                    .map_err(|error| error.to_string())
                    .and_then(|mut record| {
                        let lowered = package
                            .lowered(position)
                            .ok_or_else(|| "its lowered declarations do not decode".to_string())?;
                        let keys: Vec<_> = directory.declaration_keys(position).collect();
                        record.join(&keys, lowered)?;
                        Ok(Box::new(record))
                    })
                    .map_err(|error| {
                        self.0
                            .failures
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push(format!(
                                "the interface of `{}` does not decode: {error}",
                                entry.name
                            ));
                    })
                    .ok()
            })
            .as_deref()
    }

    /// The transfer effects the package recorded for its callable `name`,
    /// decoding the lowered part of the declaration owning it.
    pub(super) fn transfer_effects(
        &self,
        name: &str,
    ) -> Option<&Vec<mojito_checked::checked::TransferEffect>> {
        self.lowered_owning(name)?.transfer_effects(name)
    }

    /// The call-through effects the package recorded for its callable
    /// `name`.
    pub(super) fn call_through_effects(
        &self,
        name: &str,
    ) -> Option<&Vec<mojito_checked::checked::CallThroughEffect>> {
        self.lowered_owning(name)?.call_through_effects(name)
    }

    /// The lowered part of the declaration owning `name`, noting a record
    /// that does not decode.
    fn lowered_owning(&self, name: &str) -> Option<&LoweredDeclaration> {
        let package = &self.0.package.package;
        let lowered = package.lowered_owning(name);
        if lowered.is_none() {
            self.0.reached_failures.note(package, package.owning(name));
        }
        lowered
    }

    /// The dotted name of the reached module declaring `table`'s `key`
    /// (`std.builtin.comparable`), decoding nothing.
    pub(super) fn declaring_module(&self, table: Table, key: &str) -> Option<&str> {
        let directory = self.0.package.package.directory();
        let entry = directory.declaration(self.position(table, key)?)?;
        Some(directory.module(entry.module as usize)?.name)
    }

    /// Whether a reached module's declaration holds `table`'s `key`,
    /// decoding nothing.
    pub(super) fn has_key(&self, table: Table, key: &str) -> bool {
        self.position(table, key).is_some()
    }

    /// Every key of `table` a reached module's declaration holds, decoding
    /// nothing.
    pub(super) fn keys(&self, table: Table) -> impl Iterator<Item = &str> {
        self.0
            .package
            .package
            .directory()
            .table_keys(table)
            .filter(|(_, position)| self.reached_declaration(*position))
            .map(|(key, _)| key)
    }

    /// The struct keys of every reached declaration whose struct declares
    /// `method`, decoding nothing.
    pub(super) fn structs_declaring<'a>(
        &'a self,
        method: &'a str,
    ) -> impl Iterator<Item = &'a str> + 'a {
        let directory = self.0.package.package.directory();
        directory
            .methods_declaring(method)
            .filter(|position| self.reached_declaration(*position))
            .flat_map(|position| directory.declaration_keys(position as usize))
            .filter(|(table, _)| *table == Table::Structs)
            .map(|(_, key)| key)
    }

    /// The module binding `name` and its ordinal among the reached
    /// modules' bindings: by module in the order the link reached them,
    /// then by name.
    pub(super) fn binding(&self, name: &str) -> Option<(u32, &ModuleBinding)> {
        let directory = self.0.package.package.directory();
        let declaration = directory.key_declaration(Table::Bindings, name)?;
        let module = directory.declaration(declaration as usize)?.module as usize;
        let position = directory.binding_position(module, name)? as u32;
        let base = self.0.binding_bases.get(module).copied().flatten()?;
        let bindings = &self.declaration(Table::Bindings, name)?.bindings;
        let found = bindings
            .binary_search_by(|binding| binding.name.as_str().cmp(name))
            .ok()?;
        Some((base + position, &bindings[found]))
    }

    /// The name of the module binding numbered `ordinal`.
    pub(super) fn binding_name(&self, ordinal: u32) -> Option<&str> {
        self.0.reached_order.iter().find_map(|module| {
            let base = self.0.binding_bases[*module as usize]?;
            self.0
                .package
                .package
                .directory()
                .binding(*module as usize, ordinal.checked_sub(base)? as usize)
        })
    }

    /// How many module bindings the reached modules make.
    pub(super) fn binding_count(&self) -> u32 {
        self.0.binding_count
    }

    fn position(&self, table: Table, key: &str) -> Option<usize> {
        let position = self
            .0
            .package
            .package
            .directory()
            .key_declaration(table, key)?;
        self.reached_declaration(position)
            .then_some(position as usize)
    }

    fn reached_declaration(&self, position: u32) -> bool {
        self.0
            .package
            .package
            .directory()
            .declaration(position as usize)
            .and_then(|entry| self.0.reached.get(entry.module as usize).copied())
            .unwrap_or(false)
    }
}

/// A module `comptime` value constant as the build's comptime elaboration
/// folded it, which an importer's elaboration reads instead of folding it.
///
/// It is a closed constant's folded value, or the marker of one the check
/// binds. A module alias, generic or not, travels only as the check's
/// `Aliases` row.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum ElaboratedConstant {
    Value(CtValue),
}

/// The elaborated module constants of a package's program, each beside its
/// name and the module declaring it.
pub type ElaboratedConstants = Vec<(String, String, ElaboratedConstant)>;

/// One library declaration's rows of the checker's tables, as the build's
/// check resolved them.
///
/// Every table is sorted by key; a name-set table (`declared_structs` and
/// the like) is only keys in the directory. Encoded, it is its record's
/// residue: a fact the record's lowered part says is left out
/// ([`Self::project`]) — a projected struct's or function's declared facts,
/// and a module integer constant's value, which only its lowered part
/// holds — and so is every key and projected name, which the
/// directory's key span for the declaration holds once; both are restored
/// when the residue decodes ([`Self::join`]).
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(super) struct DeclarationInterface {
    #[serde(with = "keyless")]
    structs: Vec<(String, StructInfo)>,
    #[serde(with = "keyless")]
    traits: Vec<(String, TraitInfo)>,
    #[serde(with = "keyless")]
    aliases: Vec<(String, ComptimeAlias)>,
    #[serde(with = "keyless")]
    comptimes: Vec<(String, mojito_common::literal::IntLiteral)>,
    #[serde(with = "keyless")]
    comptime_literals: Vec<(String, Expr)>,
    #[serde(with = "keyless")]
    module_values: Vec<(String, CtValue)>,
    #[serde(with = "keyless")]
    comptime_applied: Vec<(String, ParamExpr)>,
    #[serde(with = "keyless")]
    applicable_functions: Vec<(String, Vec<comptime_validation::ApplicableFunction>)>,
    #[serde(with = "keyless")]
    def_results: Vec<(String, SourceType)>,
    #[serde(with = "keyless")]
    owned_collectors: Vec<(String, bool)>,
    #[serde(with = "keyless")]
    collector_names: Vec<(String, String)>,
    /// The explicit-destruction obligation of a struct whose self-type is
    /// not `Deinitable`; a projected struct's is its lowered declaration's.
    #[serde(with = "keyless")]
    explicit_destroy: Vec<(String, mojito_checked::checked::ExplicitDestroyInfo)>,
    /// The declaration's module-scope bindings: a `def`, its overloads
    /// merged, or a module constant.
    bindings: Vec<ModuleBinding>,
    /// The module constant it declares, as the build's comptime elaboration
    /// resolved it.
    #[serde(with = "keyless")]
    elaborated: Vec<(String, ElaboratedConstant)>,
    /// Its module-scope compile-time facts, as the scope-0 entries of the
    /// checker's per-scope tables.
    #[serde(with = "keyless")]
    comptime_dtypes: Vec<(String, SimdDtype)>,
    #[serde(with = "keyless")]
    comptime_parameters: Vec<(String, (usize, ParamExpr))>,
    #[serde(with = "keyless")]
    comptime_values: Vec<(String, Expr)>,
    /// The `SIMDLength` binders it owns.
    simd_length_binders: Vec<ParamId>,
    /// The structs whose declared parameters, fields, fieldwise
    /// initializer, and explicit-destruction facts the record leaves to
    /// their lowered declarations, sorted by name.
    projected_structs: Vec<ProjectedStruct>,
    /// The module-scope `def`s whose type the record leaves to their
    /// lowered declarations, sorted by name.
    projected_bindings: Vec<ProjectedBinding>,
}

/// What a module `def`'s type says that its lowered declaration does not.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct ProjectedBinding {
    /// Restored from the declaration's bindings that have a lowered
    /// function of their name.
    #[serde(skip)]
    name: String,
    /// Whether the type is a `GenericFunc`, not a `Func`.
    generic: bool,
    environment: mojito_types::origin::CallableEnvironment,
    ref_params: Vec<Option<mojito_types::origin::RefSig>>,
    ref_return: Option<Box<mojito_types::origin::RefSig>>,
    /// Its transfers, unless they are the transfer effects its record
    /// summarizes for it.
    transfers: Option<mojito_types::types::TransferSet>,
}

/// A struct whose row leaves its declared facts to its lowered declaration.
#[derive(serde::Serialize, serde::Deserialize)]
struct ProjectedStruct {
    /// Restored from the declaration's structs that have a lowered
    /// declaration.
    #[serde(skip)]
    name: String,
    /// When a function lowers each of its methods, every method's residue,
    /// by method name, each method's overloads in the row's order; its row
    /// then holds no method, each signature being its function's
    /// declaration and the residue.
    methods: Option<Vec<MethodResidue>>,
}

/// What a method's signature says that its lowered declaration does not.
///
/// It names its method by its function's index in the record, whose
/// lowered name spells the method's name and overload qualifier.
#[derive(serde::Serialize, serde::Deserialize)]
struct MethodResidue {
    function: u32,
    variadic_name: Option<String>,
    ret_mlir_index: bool,
    ref_params: Vec<Option<mojito_types::origin::RefSig>>,
    ref_return: Option<mojito_types::origin::RefSig>,
    view_return_interior: Vec<String>,
    view_return: Vec<ViewReturnOrigin>,
    implicit: bool,
    parametric_origin_writes: Vec<mojito_types::origin::OriginParamId>,
    origin_binders: Vec<Option<mojito_types::origin::PointerOrigin>>,
    synthesized_default: bool,
    nested_origins: NestedOrigins,
}

/// One name bound at module scope.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct ModuleBinding {
    /// Restored from the directory's key span for its declaration.
    #[serde(skip)]
    pub(super) name: String,
    pub(super) ty: Ty,
    pub(super) mutable: bool,
    /// One entry per overload declaration, as `callable_origin_scopes` keeps.
    pub(super) callable_origins: Option<Vec<CallableOriginSignature>>,
    /// Whether it is a module `comptime` binding a compile-time expression
    /// may read (`comptime_binding_owners`).
    pub(super) comptime: bool,
    /// Whether it is a compile-time binding (`compile_time_bindings`).
    pub(super) compile_time: bool,
}

impl Checker {
    /// The interface of every library declaration `program` makes, from
    /// this checker's tables after it checked `program` whole, with each
    /// module constant of `elaborated` in its own declaration's record.
    ///
    /// # Errors
    ///
    /// A module-scope fact the interface cannot carry, a name two modules
    /// declare, or a table that holds a binding owner or a hole, naming it.
    pub(super) fn export_interfaces(
        &self,
        program: &[Stmt],
        elaborated: &[(String, String, ElaboratedConstant)],
    ) -> Result<InterfaceExport, String> {
        let modules = declaring_modules(program, elaborated)?;
        let builtin = Self::new();
        let mut declarations: BTreeMap<String, (DeclarationInterface, Vec<(Table, String)>)> =
            BTreeMap::new();
        let mut unplaced = Vec::new();
        // The declaration `name` belongs to; a name no library module
        // declares is a fresh checker's own, or reported.
        let mut place = |name: &str, builtin_has: bool| -> Option<String> {
            let declaration = modules.declaration_of(name);
            if declaration.is_none() && !builtin_has {
                unplaced.push(name.to_string());
            }
            declaration.map(str::to_string)
        };
        macro_rules! export_map {
            ($source:expr, $builtin:expr, $table:expr, $field:ident) => {
                for (name, value) in $source.iter() {
                    if let Some(declaration) = place(name, $builtin.contains_key(name)) {
                        let (record, keys) = declarations.entry(declaration).or_default();
                        record.$field.push((name.clone(), value.clone()));
                        keys.push(($table, name.clone()));
                    }
                }
            };
        }
        macro_rules! export_set {
            ($source:expr, $builtin:expr, $table:expr) => {
                for name in $source.iter() {
                    if let Some(declaration) = place(name, $builtin.contains(name)) {
                        declarations
                            .entry(declaration)
                            .or_default()
                            .1
                            .push(($table, name.to_string()));
                    }
                }
            };
        }
        macro_rules! export_module_scope {
            ($source:expr, $builtin:expr, $table:expr, $field:ident) => {
                let source = module_scope(&$source);
                let builtin_scope = module_scope(&$builtin);
                export_map!(source, builtin_scope, $table, $field);
            };
        }
        export_map!(self.structs, builtin.structs, Table::Structs, structs);
        export_map!(self.traits, builtin.traits, Table::Traits, traits);
        export_map!(
            self.comptime_aliases,
            builtin.comptime_aliases,
            Table::Aliases,
            aliases
        );
        export_map!(
            self.comptimes,
            builtin.comptimes,
            Table::Comptimes,
            comptimes
        );
        export_map!(
            self.comptime_literals,
            builtin.comptime_literals,
            Table::ComptimeLiterals,
            comptime_literals
        );
        export_map!(
            self.module_values,
            builtin.module_values,
            Table::ModuleValues,
            module_values
        );
        export_map!(
            self.comptime_applied,
            builtin.comptime_applied,
            Table::ComptimeApplied,
            comptime_applied
        );
        export_map!(
            self.applicable_functions,
            builtin.applicable_functions,
            Table::ApplicableFunctions,
            applicable_functions
        );
        export_map!(
            self.module_def_results,
            builtin.module_def_results,
            Table::DefResults,
            def_results
        );
        export_map!(
            self.owned_collectors,
            builtin.owned_collectors,
            Table::OwnedCollectors,
            owned_collectors
        );
        export_map!(
            self.collector_names,
            builtin.collector_names,
            Table::CollectorNames,
            collector_names
        );
        export_map!(
            super::explicit_destroy_types(self),
            builtin.structs,
            Table::ExplicitDestroy,
            explicit_destroy
        );
        export_set!(
            self.declared_structs,
            builtin.declared_structs,
            Table::DeclaredStructs
        );
        export_set!(
            self.applied_module_constants,
            builtin.applied_module_constants,
            Table::AppliedModuleConstants
        );
        export_set!(
            self.nested_origins_read_only_functions,
            builtin.nested_origins_read_only_functions,
            Table::NestedOriginsReadOnlyFunctions
        );
        export_module_scope!(
            self.comptime_dtypes,
            builtin.comptime_dtypes,
            Table::ComptimeDtypes,
            comptime_dtypes
        );
        export_module_scope!(
            self.local_comptime_parameters,
            builtin.local_comptime_parameters,
            Table::ComptimeParameters,
            comptime_parameters
        );
        export_module_scope!(
            self.local_comptime_values,
            builtin.local_comptime_values,
            Table::ComptimeValues,
            comptime_values
        );
        for binder in &self.simd_length_binders {
            if let Some(declaration) =
                place(&binder.owner, builtin.simd_length_binders.contains(binder))
            {
                let (record, keys) = declarations.entry(declaration).or_default();
                record.simd_length_binders.push(binder.clone());
                keys.push((Table::SimdLengthBinders, binder.owner.to_string()));
            }
        }
        let scope = module_scope(&self.scopes);
        let mutable = module_scope(&self.mutable_scopes);
        let owners = module_scope(&self.owner_scopes);
        let origins = module_scope(&self.callable_origin_scopes);
        let compile_time = self
            .compile_time_bindings
            .first()
            .cloned()
            .unwrap_or_default();
        let builtin_scope = module_scope(&builtin.scopes);
        for (name, ty) in &scope {
            if let Some(declaration) = place(name, builtin_scope.contains_key(name)) {
                let (record, keys) = declarations.entry(declaration).or_default();
                record.bindings.push(ModuleBinding {
                    name: name.clone(),
                    ty: ty.clone(),
                    mutable: mutable.get(name).copied().unwrap_or(false),
                    callable_origins: origins.get(name).cloned(),
                    comptime: owners
                        .get(name)
                        .is_some_and(|owner| self.comptime_binding_owners.contains(owner)),
                    compile_time: compile_time.contains(name),
                });
                keys.push((Table::Bindings, name.clone()));
            }
        }
        for (_, name, constant) in elaborated {
            if let Some(declaration) = place(name, false) {
                let (record, keys) = declarations.entry(declaration).or_default();
                record.elaborated.push((name.clone(), constant.clone()));
                keys.push((Table::Elaborated, name.clone()));
            }
        }
        let overloads = self.overload_sets.own().clone();
        self.refuse_uncarried(&modules)?;
        refuse_unplaced(unplaced)?;
        let callables = self.top_level_callables.borrow();
        let transfer_effects = summarized(&self.transfer_effects.borrow(), &callables);
        let call_through_effects = summarized(&self.call_through_effects.borrow(), &callables);
        Ok(InterfaceExport {
            declarations,
            overloads,
            declaring: modules,
            transfer_effects,
            call_through_effects,
            type_aliases: self.program_constants.own_type_aliases(),
        })
    }

    /// Resolve every table lookup the program's own declarations miss from
    /// `installed`, and bind the reached modules' bindings below the module
    /// scope, with a block of owners reserved for them in their install
    /// order. Nothing is copied in: a module-scope compile-time fact, and a
    /// name the program's own overload sets miss, are answered from the
    /// package declaration owning the name.
    ///
    /// # Errors
    ///
    /// The checker's binding identities run out.
    pub(super) fn attach_interfaces(
        &mut self,
        installed: &InstalledInterfaces,
    ) -> Result<(), TypeError> {
        self.structs
            .attach(installed, Table::Structs, |record| &record.structs);
        self.traits
            .attach(installed, Table::Traits, |record| &record.traits);
        self.comptime_aliases
            .attach(installed, Table::Aliases, |record| &record.aliases);
        self.comptimes
            .attach(installed, Table::Comptimes, |record| &record.comptimes);
        self.comptime_literals
            .attach(installed, Table::ComptimeLiterals, |record| {
                &record.comptime_literals
            });
        self.module_values
            .attach(installed, Table::ModuleValues, |record| {
                &record.module_values
            });
        self.comptime_applied
            .attach(installed, Table::ComptimeApplied, |record| {
                &record.comptime_applied
            });
        self.applicable_functions
            .attach(installed, Table::ApplicableFunctions, |record| {
                &record.applicable_functions
            });
        self.module_def_results
            .attach(installed, Table::DefResults, |record| &record.def_results);
        self.owned_collectors
            .attach(installed, Table::OwnedCollectors, |record| {
                &record.owned_collectors
            });
        self.collector_names
            .attach(installed, Table::CollectorNames, |record| {
                &record.collector_names
            });
        self.declared_structs
            .attach(installed, Table::DeclaredStructs);
        self.applied_module_constants
            .attach(installed, Table::AppliedModuleConstants);
        self.nested_origins_read_only_functions
            .attach(installed, Table::NestedOriginsReadOnlyFunctions);
        self.overload_sets = Arc::new(mojito_checked::package::ProgramOverloads::importing(Some(
            installed.package_scope(),
        )));
        self.program_constants = Arc::new(mojito_checked::package::ProgramConstants::importing(
            Some(installed.package_scope()),
        ));
        let base = self.reserve_owners(installed.binding_count())?;
        self.installed = Some((installed.clone(), base));
        Ok(())
    }

    /// Every struct declaring `method`: the own ones, and each installed one
    /// whose record the directory says declares it.
    pub(super) fn structs_declaring<'a>(
        &'a self,
        method: &'a str,
    ) -> impl Iterator<Item = (&'a String, &'a StructInfo)> {
        self.structs
            .iter_selected(|installed| installed.structs_declaring(method))
            .filter(move |(_, info)| info.methods.contains_key(method))
    }

    /// The dtype the installed module `comptime` binding `name` holds.
    pub(super) fn installed_comptime_dtype(&self, name: &str) -> Option<&SimdDtype> {
        self.installed_fact(Table::ComptimeDtypes, name, |record| {
            &record.comptime_dtypes
        })
    }

    /// The application an installed module constant the check lifted
    /// denotes, beside its scope level.
    pub(super) fn installed_comptime_parameter(&self, name: &str) -> Option<&(usize, ParamExpr)> {
        self.installed_fact(Table::ComptimeParameters, name, |record| {
            &record.comptime_parameters
        })
    }

    /// The defining expression of an installed module compile-time value.
    pub(super) fn installed_comptime_value(&self, name: &str) -> Option<&Expr> {
        self.installed_fact(Table::ComptimeValues, name, |record| {
            &record.comptime_values
        })
    }

    /// Whether an installed module holds `table`'s `name`, decoding
    /// nothing.
    pub(super) fn installs(&self, table: Table, name: &str) -> bool {
        self.installed
            .as_ref()
            .is_some_and(|(installed, _)| installed.has_key(table, name))
    }

    /// Whether any installed module holds a key of `table`, decoding
    /// nothing.
    pub(super) fn installs_any(&self, table: Table) -> bool {
        self.installed
            .as_ref()
            .is_some_and(|(installed, _)| installed.keys(table).next().is_some())
    }

    /// Whether `binder` is a `SIMDLength` binder: one the check declared,
    /// or one an installed declaration owns.
    pub(super) fn is_simd_length_binder(&self, binder: &ParamId) -> bool {
        self.simd_length_binders.contains(binder)
            || self.installed.as_ref().is_some_and(|(installed, _)| {
                installed
                    .declaration(Table::SimdLengthBinders, &binder.owner)
                    .is_some_and(|record| record.simd_length_binders.contains(binder))
            })
    }

    /// The explicit-destruction obligations `own` gives the program's own
    /// structs, over the installed structs' as the package build's check
    /// resolved them.
    pub(super) fn explicit_destroy_table(
        &self,
        own: HashMap<String, mojito_checked::checked::ExplicitDestroyInfo>,
    ) -> DeclarationTable<mojito_checked::checked::ExplicitDestroyInfo> {
        let mut table = DeclarationTable::default();
        table.extend(own);
        if let Some((installed, _)) = &self.installed {
            table.attach(installed, Table::ExplicitDestroy, |record| {
                &record.explicit_destroy
            });
        }
        table
    }
    /// The module-scope fact `table` holds for `name` in the installed
    /// declaration owning it, decoded on first lookup.
    fn installed_fact<V>(
        &self,
        table: Table,
        name: &str,
        select: impl FnOnce(&DeclarationInterface) -> &Vec<(String, V)>,
    ) -> Option<&V> {
        let (installed, _) = self.installed.as_ref()?;
        let entries = select(installed.declaration(table, name)?);
        entries
            .binary_search_by(|(entry, _)| entry.as_str().cmp(name))
            .ok()
            .map(|position| &entries[position].1)
    }

    /// Refuse a module-scope fact of a library declaration no interface
    /// carries.
    fn refuse_uncarried(&self, modules: &DeclaringModules) -> Result<(), String> {
        let uncarried = [
            (
                "aggregate origins",
                scope_names(&self.aggregate_origin_scopes),
            ),
            (
                "field origins",
                scope_names(&self.aggregate_field_origin_scopes),
            ),
            (
                "storage origin demands",
                scope_names(&self.storage_origin_demand_scopes),
            ),
            (
                "immutable origin binders",
                scope_names(&self.immutable_origin_binder_scopes),
            ),
            (
                "reference parameters",
                scope_names(&self.reference_parameter_scopes),
            ),
            (
                "compile-time displays",
                scope_names(&self.local_comptime_displays),
            ),
        ];
        for (table, names) in uncarried {
            if let Some(name) = names.iter().find(|name| modules.module_of(name).is_some()) {
                return Err(format!(
                    "the module-scope {table} of `{name}` cannot be exported"
                ));
            }
        }
        Ok(())
    }
}

/// Write `map` sorted by key, so an interface encodes the same bytes in
/// every build.
pub(super) fn sorted<K, V, S>(map: &HashMap<K, V>, serializer: S) -> Result<S::Ok, S::Error>
where
    K: Ord + serde::Serialize,
    V: serde::Serialize,
    S: serde::Serializer,
{
    serde::Serialize::serialize(&map.iter().collect::<BTreeMap<_, _>>(), serializer)
}

/// A keyed table written as its values alone, its keys being the
/// directory's, which [`DeclarationInterface::join`] restores.
mod keyless {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(super) fn serialize<V: Serialize, S: Serializer>(
        entries: &[(String, V)],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(entries.iter().map(|(_, value)| value))
    }

    pub(super) fn deserialize<'de, V: Deserialize<'de>, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<(String, V)>, D::Error> {
        Ok(Vec::<V>::deserialize(deserializer)?
            .into_iter()
            .map(|value| (String::new(), value))
            .collect())
    }
}

/// What a compilation reached of a package's declaration interfaces.
struct Installed {
    package: Arc<DeclarationInterfaces>,
    /// Per module, whether the link reached it.
    reached: Arc<[bool]>,
    /// The reached modules, in the order the link reached them.
    reached_order: Vec<u32>,
    /// Per reached module, the ordinal of its first binding.
    binding_bases: Vec<Option<u32>>,
    binding_count: u32,
    /// Per declaration, its record once a lookup asked for it.
    decoded: Vec<OnceLock<Option<Box<DeclarationInterface>>>>,
    failures: Mutex<Vec<String>>,
    /// The package's lowered declarations its lookups and its package
    /// scope's reached that do not decode.
    reached_failures: mojito_checked::package::ReachedFailures,
}

impl DeclarationInterface {
    /// Leave out of each struct's row, and each of its methods' signature,
    /// what its declaration in `lowered` says, once the guard finds it says
    /// exactly that: a method's declaration is the function named by its
    /// struct, its name, and its overload qualifier.
    fn project(&mut self, lowered: &LoweredDeclaration) -> Result<(), String> {
        self.project_constants(lowered)?;
        for (name, info) in &mut self.structs {
            let Some(declaration) = lowered_struct(lowered, name) else {
                continue;
            };
            let row = self
                .explicit_destroy
                .iter()
                .find(|(owner, _)| owner == name)
                .map(|(_, row)| row);
            if let Some(fact) = struct_projection_differs(info, row, declaration) {
                return Err(format!(
                    "the lowered declaration of struct `{name}` says other than its check of \
                     its {fact}"
                ));
            }
            let methods = project_methods(name, info, lowered)?;
            if methods.is_some() {
                info.methods = HashMap::new();
            }
            info.decls = Vec::new();
            info.fields = Vec::new();
            info.fieldwise_init = false;
            info.explicit_destroy_message = None;
            info.explicit_destructors = HashMap::new();
            self.projected_structs.push(ProjectedStruct {
                name: name.clone(),
                methods,
            });
        }
        self.explicit_destroy.retain(|(owner, _)| {
            !self
                .projected_structs
                .iter()
                .any(|projected| projected.name == *owner)
        });
        for binding in &mut self.bindings {
            let (Some(projected), Some(declaration)) = (
                ProjectedBinding::of(&binding.name, &binding.ty),
                lowered_function(lowered, &binding.name),
            ) else {
                continue;
            };
            let summarized = ProjectedBinding {
                transfers: None,
                ..projected.clone()
            };
            let projected = [summarized, projected]
                .into_iter()
                .find(|projected| {
                    same_bytes(&projected.clone().ty(declaration, lowered), &binding.ty)
                })
                .ok_or_else(|| {
                    format!(
                        "the lowered declaration of `{}` says other than its check of its type",
                        binding.name
                    )
                })?;
            binding.ty = Ty::none();
            self.projected_bindings.push(projected);
        }
        Ok(())
    }

    /// Leave out each integer row and elaborated value of a module constant
    /// whose one value `lowered` holds, once the guard finds each says
    /// exactly that value.
    fn project_constants(&mut self, lowered: &LoweredDeclaration) -> Result<(), String> {
        for (name, value) in &self.comptimes {
            if !lowered
                .constant(name)
                .is_some_and(|constant| same_bytes(value, constant))
            {
                return Err(format!(
                    "the lowered constant `{name}` says other than its check"
                ));
            }
        }
        for (name, constant) in &self.elaborated {
            if let Some(value) = lowered.constant(name)
                && !same_bytes(constant, &elaborated_constant(value))
            {
                return Err(format!(
                    "the elaboration of `{name}` says other than its check"
                ));
            }
        }
        self.comptimes.clear();
        self.elaborated
            .retain(|(name, _)| lowered.constant(name).is_none());
        Ok(())
    }

    /// Restore what [`Self::project`] left out from `lowered`, and every
    /// key and projected name from the directory's `keys` for the
    /// declaration.
    fn join(&mut self, keys: &[(Table, &str)], lowered: &LoweredDeclaration) -> Result<(), String> {
        for (column, names) in self.key_columns() {
            let implied = implied_keys(column, keys, lowered);
            if names.len() != implied.len() {
                return Err(format!("its {column:?} rows are not its directory's keys"));
            }
            for (name, key) in names.into_iter().zip(implied) {
                *name = key.to_string();
            }
        }
        for projected in std::mem::take(&mut self.projected_structs) {
            let name = projected.name;
            let declaration = lowered_struct(lowered, &name)
                .ok_or_else(|| format!("struct `{name}` has no lowered declaration"))?;
            let (_, info) = self
                .structs
                .iter_mut()
                .find(|(owner, _)| *owner == name)
                .ok_or_else(|| format!("struct `{name}` has no row"))?;
            info.decls.clone_from(&declaration.param_decls);
            info.fields.clone_from(&declaration.fields);
            info.fieldwise_init = declaration.fieldwise_init;
            info.explicit_destructors
                .clone_from(&declaration.explicit_destructors);
            if let Some(obligation) = declaration.explicit_destroy_info() {
                info.explicit_destroy_message =
                    obligation.declared.then(|| obligation.message.clone());
                self.explicit_destroy.push((name.clone(), obligation));
            }
            for residue in projected.methods.into_iter().flatten() {
                let function = lowered
                    .functions
                    .get(residue.function as usize)
                    .ok_or_else(|| format!("a method residue of `{name}` names no function"))?;
                let (method, qualifier) =
                    mojito_symbol::symbol::method_of_symbol(&name, &function.lowered_name)
                        .ok_or_else(|| {
                            format!(
                                "function `{}` is no method of `{name}`",
                                function.lowered_name
                            )
                        })?;
                let sig = residue.sig(method, qualifier, info.decls.len(), function);
                info.methods
                    .entry(method.to_string())
                    .or_default()
                    .push(sig);
            }
        }
        by_name(&mut self.explicit_destroy);
        for (table, key) in keys {
            let Some(value) = lowered.constant(key) else {
                continue;
            };
            match table {
                Table::Comptimes => self.comptimes.push((key.to_string(), value.clone())),
                Table::Elaborated => self
                    .elaborated
                    .push((key.to_string(), elaborated_constant(value))),
                _ => {}
            }
        }
        by_name(&mut self.comptimes);
        by_name(&mut self.elaborated);
        for projected in std::mem::take(&mut self.projected_bindings) {
            let declaration = lowered_function(lowered, &projected.name).ok_or_else(|| {
                format!("function `{}` has no lowered declaration", projected.name)
            })?;
            let binding = self
                .bindings
                .binary_search_by(|binding| binding.name.cmp(&projected.name))
                .map(|position| &mut self.bindings[position])
                .map_err(|_| format!("function `{}` has no binding", projected.name))?;
            binding.ty = projected.ty(declaration, lowered);
        }
        Ok(())
    }

    /// Refuse the residue of the declaration `name` whose keys and
    /// projected names are not those its directory `keys` and `lowered`
    /// part imply, so that [`Self::join`] restores them.
    fn refuse_unrestorable(
        &mut self,
        name: &str,
        keys: &[(Table, String)],
        lowered: &LoweredDeclaration,
    ) -> Result<(), String> {
        let keys: Vec<(Table, &str)> = keys
            .iter()
            .map(|(table, key)| (*table, key.as_str()))
            .collect();
        for (column, names) in self.key_columns() {
            let implied = implied_keys(column, &keys, lowered);
            if !names.iter().map(|name| name.as_str()).eq(implied) {
                return Err(format!(
                    "the residue of `{name}` holds other {column:?} keys than its directory"
                ));
            }
        }
        Ok(())
    }

    /// Each column of names the encoded residue leaves to the directory.
    fn key_columns(&mut self) -> [(KeyColumn, Vec<&mut String>); 19] {
        fn keys<V>(entries: &mut [(String, V)]) -> Vec<&mut String> {
            entries.iter_mut().map(|(key, _)| key).collect()
        }
        [
            (KeyColumn::Table(Table::Structs), keys(&mut self.structs)),
            (KeyColumn::Table(Table::Traits), keys(&mut self.traits)),
            (KeyColumn::Table(Table::Aliases), keys(&mut self.aliases)),
            (
                KeyColumn::Table(Table::Comptimes),
                keys(&mut self.comptimes),
            ),
            (
                KeyColumn::Table(Table::ComptimeLiterals),
                keys(&mut self.comptime_literals),
            ),
            (
                KeyColumn::Table(Table::ModuleValues),
                keys(&mut self.module_values),
            ),
            (
                KeyColumn::Table(Table::ComptimeApplied),
                keys(&mut self.comptime_applied),
            ),
            (
                KeyColumn::Table(Table::ApplicableFunctions),
                keys(&mut self.applicable_functions),
            ),
            (
                KeyColumn::Table(Table::DefResults),
                keys(&mut self.def_results),
            ),
            (
                KeyColumn::Table(Table::OwnedCollectors),
                keys(&mut self.owned_collectors),
            ),
            (
                KeyColumn::Table(Table::CollectorNames),
                keys(&mut self.collector_names),
            ),
            (
                KeyColumn::Table(Table::ExplicitDestroy),
                keys(&mut self.explicit_destroy),
            ),
            (
                KeyColumn::Table(Table::Bindings),
                self.bindings
                    .iter_mut()
                    .map(|binding| &mut binding.name)
                    .collect(),
            ),
            (
                KeyColumn::Table(Table::Elaborated),
                keys(&mut self.elaborated),
            ),
            (
                KeyColumn::Table(Table::ComptimeDtypes),
                keys(&mut self.comptime_dtypes),
            ),
            (
                KeyColumn::Table(Table::ComptimeParameters),
                keys(&mut self.comptime_parameters),
            ),
            (
                KeyColumn::Table(Table::ComptimeValues),
                keys(&mut self.comptime_values),
            ),
            (
                KeyColumn::ProjectedStructs,
                self.projected_structs
                    .iter_mut()
                    .map(|projected| &mut projected.name)
                    .collect(),
            ),
            (
                KeyColumn::ProjectedBindings,
                self.projected_bindings
                    .iter_mut()
                    .map(|projected| &mut projected.name)
                    .collect(),
            ),
        ]
    }

    fn sort(&mut self) {
        by_name(&mut self.structs);
        by_name(&mut self.traits);
        by_name(&mut self.aliases);
        by_name(&mut self.comptimes);
        by_name(&mut self.comptime_literals);
        by_name(&mut self.module_values);
        by_name(&mut self.comptime_applied);
        by_name(&mut self.applicable_functions);
        by_name(&mut self.def_results);
        by_name(&mut self.owned_collectors);
        by_name(&mut self.collector_names);
        by_name(&mut self.explicit_destroy);
        by_name(&mut self.elaborated);
        by_name(&mut self.comptime_dtypes);
        by_name(&mut self.comptime_parameters);
        by_name(&mut self.comptime_values);
        self.simd_length_binders.sort();
        self.bindings
            .sort_by(|left, right| left.name.cmp(&right.name));
        self.projected_structs
            .sort_by(|left, right| left.name.cmp(&right.name));
        self.projected_bindings
            .sort_by(|left, right| left.name.cmp(&right.name));
    }
}

/// The first fact of a struct's row (`info`, and its explicit-destruction
/// `row`) its lowered `declaration` does not say byte for byte, or `None`.
fn struct_projection_differs(
    info: &StructInfo,
    row: Option<&mojito_checked::checked::ExplicitDestroyInfo>,
    declaration: &StructDeclaration,
) -> Option<&'static str> {
    fn sorted(map: &HashMap<String, bool>) -> BTreeMap<&String, &bool> {
        map.iter().collect()
    }
    let obligation = declaration.explicit_destroy.as_ref();
    [
        (
            "parameters",
            same_bytes(&info.decls, &declaration.param_decls),
        ),
        ("fields", same_bytes(&info.fields, &declaration.fields)),
        (
            "fieldwise initializer",
            info.fieldwise_init == declaration.fieldwise_init,
        ),
        (
            "explicit-destroy message",
            info.explicit_destroy_message.as_ref()
                == obligation
                    .filter(|obligation| obligation.declared)
                    .map(|obligation| &obligation.message),
        ),
        (
            "destructors",
            sorted(&info.explicit_destructors) == sorted(&declaration.explicit_destructors),
        ),
        (
            "explicit-destruction obligation",
            row == declaration.explicit_destroy_info().as_ref(),
        ),
    ]
    .into_iter()
    .find_map(|(fact, same)| (!same).then_some(fact))
}

/// The first fact of a method's signature its lowered `declaration` does
/// not say byte for byte, or `None`.
fn method_projection_differs(
    method: &str,
    owner_decls: &[ParamDecl],
    sig: &MethodSig,
    declaration: &FunctionDeclaration,
) -> Option<&'static str> {
    [
        (
            "parameters",
            own_decls(method, owner_decls.len(), declaration)
                .is_some_and(|own| same_bytes(&sig.decls, &own))
                && (!mojito_symbol::symbol::is_initializer_member(method)
                    || declaration
                        .param_decls
                        .get(..owner_decls.len())
                        .is_some_and(|prefix| same_bytes(&owner_decls, &prefix))),
        ),
        (
            "availability",
            same_bytes(&sig.availability, &declaration.availability),
        ),
        ("receiver", sig.has_self == declaration.has_receiver),
        (
            "parameter types",
            same_bytes(&sig.params, &declaration.param_types),
        ),
        ("parameter names", sig.names == declaration.param_names),
        ("required", sig.required == declaration.required),
        (
            "defaults",
            sig.defaults.len() == declaration.defaults.len()
                && sig
                    .defaults
                    .iter()
                    .zip(&declaration.defaults)
                    .all(|(default, declared)| {
                        default_declared_alike(default.as_ref(), declared.as_ref())
                    }),
        ),
        ("variadic", same_bytes(&sig.variadic, &declaration.variadic)),
        (
            "variadic index",
            sig.variadic_index == declaration.variadic_index,
        ),
        (
            "variadic convention",
            sig.variadic_convention == declaration.variadic_convention,
        ),
        (
            "keyword variadic",
            same_bytes(&sig.kw_variadic, &declaration.kw_variadic),
        ),
        (
            "keyword variadic index",
            sig.kw_variadic_index == declaration.kw_variadic_index,
        ),
        (
            "positional-only",
            sig.positional_only == declaration.positional_only,
        ),
        ("keyword-only", sig.keyword_only == declaration.keyword_only),
        (
            "conventions",
            sig.conventions == declaration.param_conventions,
        ),
        ("return", same_bytes(&sig.ret, &declaration.ret_ty)),
        ("raises", sig.raises == declaration.raises),
        ("error", same_bytes(&sig.error, &declaration.error_ty)),
        (
            "receiver convention",
            sig.self_convention == declaration.receiver_convention,
        ),
    ]
    .into_iter()
    .find_map(|(fact, same)| (!same).then_some(fact))
}

/// Whether a signature's default and its lowered declaration's hold the
/// same default: a literal as its constant or a conversion of it, any other
/// as an evaluated default.
fn default_declared_alike(
    default: Option<&SignatureDefault>,
    declared: Option<&CheckedConst>,
) -> bool {
    match (default, declared) {
        (None, None) => true,
        (Some(default), Some(declared)) => default == &SignatureDefault::of_declared(declared),
        _ => false,
    }
}

/// The residue of every method of the struct `owner` (`info`), when a
/// function in `lowered` lowers each, or `None`.
///
/// # Errors
///
/// A method whose lowered declaration says other than its signature, or
/// whose function's name would not split back into its name and overload
/// qualifier.
fn project_methods(
    owner: &str,
    info: &StructInfo,
    lowered: &LoweredDeclaration,
) -> Result<Option<Vec<MethodResidue>>, String> {
    let mut methods: Vec<(&String, &Vec<MethodSig>)> = info.methods.iter().collect();
    methods.sort_by_key(|(method, _)| *method);
    let mut residues = Vec::new();
    for (method, overloads) in methods {
        for sig in overloads {
            let lowered_name = mojito_symbol::symbol::qualified_method_symbol(
                owner,
                method,
                sig.overload.as_deref(),
            );
            let Some(function) = lowered.function_index(&lowered_name) else {
                return Ok(None);
            };
            let declaration = &lowered.functions[function];
            if let Some(fact) = method_projection_differs(method, &info.decls, sig, declaration) {
                return Err(format!(
                    "the lowered declaration of method `{lowered_name}` says other than its \
                     check of its {fact}"
                ));
            }
            if mojito_symbol::symbol::method_of_symbol(owner, &lowered_name)
                != Some((method.as_str(), sig.overload.as_deref()))
            {
                return Err(format!(
                    "method `{lowered_name}` would not join back from its function"
                ));
            }
            let function = u32::try_from(function)
                .map_err(|_| format!("method `{lowered_name}` has no function index"))?;
            residues.push(MethodResidue::of(sig, function));
        }
    }
    Ok(Some(residues))
}

impl MethodResidue {
    fn of(sig: &MethodSig, function: u32) -> Self {
        Self {
            function,
            variadic_name: sig.variadic_name.clone(),
            ret_mlir_index: sig.ret_mlir_index,
            ref_params: sig.ref_params.clone(),
            ref_return: sig.ref_return.clone(),
            view_return_interior: sig.view_return_interior.clone(),
            view_return: sig.view_return.clone(),
            implicit: sig.implicit,
            parametric_origin_writes: sig.parametric_origin_writes.clone(),
            origin_binders: sig.origin_binders.clone(),
            synthesized_default: sig.synthesized_default,
            nested_origins: sig.nested_origins,
        }
    }

    /// The signature of `method`, whose overload qualifier is `qualifier`,
    /// on a struct of `owner_decls` parameters, whose lowered declaration is
    /// `declaration`.
    fn sig(
        self,
        method: &str,
        qualifier: Option<&str>,
        owner_decls: usize,
        declaration: &FunctionDeclaration,
    ) -> MethodSig {
        MethodSig {
            decls: own_decls(method, owner_decls, declaration)
                .unwrap_or_default()
                .to_vec(),
            availability: declaration.availability.clone(),
            has_self: declaration.has_receiver,
            params: declaration.param_types.clone(),
            names: declaration.param_names.clone(),
            required: declaration.required.clone(),
            defaults: declaration
                .defaults
                .iter()
                .map(|default| default.as_ref().map(SignatureDefault::of_declared))
                .collect(),
            variadic: declaration.variadic.clone(),
            variadic_index: declaration.variadic_index,
            variadic_convention: declaration.variadic_convention,
            variadic_name: self.variadic_name,
            kw_variadic: declaration.kw_variadic.clone(),
            kw_variadic_index: declaration.kw_variadic_index,
            positional_only: declaration.positional_only,
            keyword_only: declaration.keyword_only,
            conventions: declaration.param_conventions.clone(),
            ret: declaration.ret_ty.clone(),
            ret_mlir_index: self.ret_mlir_index,
            raises: declaration.raises,
            error: declaration.error_ty.clone(),
            self_convention: declaration.receiver_convention,
            ref_params: self.ref_params,
            ref_return: self.ref_return,
            view_return_interior: self.view_return_interior,
            view_return: self.view_return,
            implicit: self.implicit,
            parametric_origin_writes: self.parametric_origin_writes,
            origin_binders: self.origin_binders,
            synthesized_default: self.synthesized_default,
            nested_origins: self.nested_origins,
            overload: qualifier.map(str::to_string),
        }
    }
}

impl ProjectedBinding {
    /// What the `def` type `ty` of the binding `name` says beyond its
    /// lowered declaration; `None` for any other type.
    fn of(name: &str, ty: &Ty) -> Option<Self> {
        let (generic, environment, ref_params, ref_return, transfers) = match ty.kind() {
            TyKind::Func(func) => {
                let FuncTy {
                    environment,
                    ref_params,
                    ref_return,
                    transfers,
                    ..
                } = &**func;
                (false, environment, ref_params, ref_return, transfers)
            }
            TyKind::GenericFunc(generic) => {
                let GenericFuncTy {
                    func:
                        FuncTy {
                            environment,
                            ref_params,
                            ref_return,
                            transfers,
                            ..
                        },
                    ..
                } = &**generic;
                (true, environment, ref_params, ref_return, transfers)
            }
            _ => return None,
        };
        Some(Self {
            name: name.to_string(),
            generic,
            environment: environment.clone(),
            ref_params: ref_params.clone(),
            ref_return: ref_return.clone(),
            transfers: Some(transfers.clone()),
        })
    }

    /// The binding's type, whose lowered declaration is `declaration` in
    /// `lowered`.
    fn ty(self, declaration: &FunctionDeclaration, lowered: &LoweredDeclaration) -> Ty {
        let transfers = self.transfers.unwrap_or_else(|| {
            mojito_types::types::TransferSet(
                lowered
                    .transfer_effects(&self.name)
                    .cloned()
                    .unwrap_or_default(),
            )
        });
        let params = declaration.param_types.clone();
        let names = declaration.param_names.clone();
        let ret = declaration.ret_ty.clone();
        let required = declaration.required.clone();
        let variadic = declaration.variadic.clone();
        let kw_variadic = declaration.kw_variadic.clone();
        let error = declaration.error_ty.clone();
        let conventions = declaration.param_conventions.clone();
        if self.generic {
            TyKind::GenericFunc(Box::new(GenericFuncTy {
                decls: declaration.param_decls.clone(),
                func: FuncTy {
                    environment: self.environment,
                    params,
                    names,
                    ret,
                    required,
                    variadic,
                    kw_variadic,
                    positional_only: declaration.positional_only,
                    keyword_only: declaration.keyword_only,
                    raises: declaration.raises,
                    error,
                    conventions,
                    ref_params: self.ref_params,
                    ref_return: self.ref_return,
                    transfers,
                },
            }))
            .into()
        } else {
            TyKind::Func(Box::new(FuncTy {
                environment: self.environment,
                params,
                names,
                ret,
                required,
                variadic,
                kw_variadic,
                positional_only: declaration.positional_only,
                keyword_only: declaration.keyword_only,
                raises: declaration.raises,
                error,
                conventions,
                ref_params: self.ref_params,
                ref_return: self.ref_return,
                transfers,
            }))
            .into()
        }
    }
}

/// The struct declaration `name` among `lowered`'s, which are sorted.
fn lowered_struct<'a>(
    lowered: &'a LoweredDeclaration,
    name: &str,
) -> Option<&'a StructDeclaration> {
    lowered
        .structs
        .binary_search_by(|declaration| declaration.name.as_bytes().cmp(name.as_bytes()))
        .ok()
        .map(|position| &lowered.structs[position])
}

/// The parameters `method`'s lowered `declaration` declares itself: an
/// initializer's declaration declares its struct's `owner_decls` first, as
/// MIR lowers one.
fn own_decls<'a>(
    method: &str,
    owner_decls: usize,
    declaration: &'a FunctionDeclaration,
) -> Option<&'a [ParamDecl]> {
    if mojito_symbol::symbol::is_initializer_member(method) {
        declaration.param_decls.get(owner_decls..)
    } else {
        Some(&declaration.param_decls)
    }
}

/// The function declaration `name` among `lowered`'s.
fn lowered_function<'a>(
    lowered: &'a LoweredDeclaration,
    name: &str,
) -> Option<&'a FunctionDeclaration> {
    lowered
        .function_index(name)
        .map(|position| &lowered.functions[position])
}

/// The entries of `effects` whose callable is one of `callables`, whose
/// bodies the check summarized.
fn summarized<V: Clone>(
    effects: &HashMap<String, V>,
    callables: &HashSet<String>,
) -> Vec<(String, V)> {
    effects
        .iter()
        .filter(|(callable, _)| callables.contains(*callable))
        .map(|(callable, effects)| (callable.clone(), effects.clone()))
        .collect()
}

/// The elaborated value of the module integer constant whose one value is
/// `value`.
fn elaborated_constant(value: &IntLiteral) -> ElaboratedConstant {
    ElaboratedConstant::Value(CtValue::IntLiteral(value.clone()))
}

/// Whether `left` and `right` encode the same bytes.
fn same_bytes<L: serde::Serialize, R: serde::Serialize>(left: &L, right: &R) -> bool {
    matches!(
        (postcard::to_stdvec(left), postcard::to_stdvec(right)),
        (Ok(left), Ok(right)) if left == right
    )
}

/// Sort `entries` by name.
fn by_name<V>(entries: &mut [(String, V)]) {
    entries.sort_by(|(left, _), (right, _)| left.cmp(right));
}

/// A column of names an encoded residue leaves to the directory.
#[derive(Debug, Clone, Copy)]
enum KeyColumn {
    /// A table's keys.
    Table(Table),
    /// The names of the projected structs: the structs with a lowered
    /// declaration.
    ProjectedStructs,
    /// The names of the projected bindings: the bindings with a lowered
    /// function of their name.
    ProjectedBindings,
}

/// The names `column` holds in the residue of a declaration whose
/// directory keys are `keys` and whose lowered part is `lowered`: a table's
/// keys, but for a projected struct's explicit-destruction row, which its
/// lowered declaration holds, and a module integer constant's integer row
/// and elaborated value, which its lowered constant holds.
fn implied_keys<'k>(
    column: KeyColumn,
    keys: &[(Table, &'k str)],
    lowered: &LoweredDeclaration,
) -> Vec<&'k str> {
    let of = |table| {
        keys.iter()
            .filter(move |(key_table, _)| *key_table == table)
            .map(|(_, key)| *key)
    };
    match column {
        KeyColumn::Table(Table::ExplicitDestroy) => of(Table::ExplicitDestroy)
            .filter(|key| lowered_struct(lowered, key).is_none())
            .collect(),
        KeyColumn::Table(table @ (Table::Comptimes | Table::Elaborated)) => of(table)
            .filter(|key| lowered.constant(key).is_none())
            .collect(),
        KeyColumn::Table(table) => of(table).collect(),
        KeyColumn::ProjectedStructs => of(Table::Structs)
            .filter(|key| lowered_struct(lowered, key).is_some())
            .collect(),
        KeyColumn::ProjectedBindings => of(Table::Bindings)
            .filter(|key| lowered_function(lowered, key).is_some())
            .collect(),
    }
}

/// The module scope of a per-scope table.
fn module_scope<V: Clone>(stack: &[HashMap<String, V>]) -> HashMap<String, V> {
    stack.first().cloned().unwrap_or_default()
}

/// The names bound in the module scope of a per-scope table.
fn scope_names<V>(stack: &[HashMap<String, V>]) -> Vec<String> {
    stack
        .first()
        .map(|scope| scope.keys().cloned().collect())
        .unwrap_or_default()
}

/// The module declaring each library declaration of `program`, and each
/// constant of `elaborated`: one the elaboration folded away (a type
/// handle) is still a declaration of its module.
fn declaring_modules(
    program: &[Stmt],
    elaborated: &[(String, String, ElaboratedConstant)],
) -> Result<DeclaringModules, String> {
    let mut modules = DeclaringModules::of(program)?;
    for (module, name, _) in elaborated {
        modules.declare(name, module)?;
    }
    Ok(modules)
}

/// Refuse a library fact no library declaration owns, naming each.
fn refuse_unplaced(mut unplaced: Vec<String>) -> Result<(), String> {
    if unplaced.is_empty() {
        return Ok(());
    }
    unplaced.sort();
    unplaced.dedup();
    Err(format!(
        "no library module declares {}",
        unplaced.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constant_n(value: i64) -> LoweredDeclaration {
        LoweredDeclaration {
            constant: Some(IntLiteral::from(value)),
            name: "N".to_string(),
            ..LoweredDeclaration::default()
        }
    }

    fn residue_of_n(value: i64) -> DeclarationInterface {
        DeclarationInterface {
            comptimes: vec![("N".to_string(), IntLiteral::from(value))],
            elaborated: vec![(
                "N".to_string(),
                elaborated_constant(&IntLiteral::from(value)),
            )],
            ..DeclarationInterface::default()
        }
    }

    const KEYS: [(Table, &str); 2] = [(Table::Comptimes, "N"), (Table::Elaborated, "N")];

    fn method_function(lowered_name: &str) -> FunctionDeclaration {
        FunctionDeclaration {
            lowered_name: lowered_name.to_string(),
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
            provenance: mojito_checked::declaration::FunctionProvenance::default(),
        }
    }

    /// The signature of `S.{method}{qualifier}` whose lowered declaration
    /// is `function`.
    fn method_sig(
        method: &str,
        qualifier: Option<&str>,
        function: &FunctionDeclaration,
    ) -> MethodSig {
        MethodResidue {
            function: 0,
            variadic_name: None,
            ret_mlir_index: false,
            ref_params: Vec::new(),
            ref_return: None,
            view_return_interior: Vec::new(),
            view_return: Vec::new(),
            implicit: false,
            parametric_origin_writes: Vec::new(),
            origin_binders: Vec::new(),
            synthesized_default: false,
            nested_origins: NestedOrigins::AsDeclared,
        }
        .sig(method, qualifier, 0, function)
    }

    /// Struct `S`'s row, with `methods`, and its lowered part, with a
    /// function of each method's lowered name.
    fn struct_s(methods: &[(&str, Option<&str>)]) -> (DeclarationInterface, LoweredDeclaration) {
        let mut functions: Vec<FunctionDeclaration> = methods
            .iter()
            .map(|(method, qualifier)| {
                method_function(&mojito_symbol::symbol::qualified_method_symbol(
                    "S", method, *qualifier,
                ))
            })
            .collect();
        let mut rows: HashMap<String, Vec<MethodSig>> = HashMap::new();
        for ((method, qualifier), function) in methods.iter().zip(&functions) {
            rows.entry((*method).to_string())
                .or_default()
                .push(method_sig(method, *qualifier, function));
        }
        functions.sort_by(|left, right| left.lowered_name.cmp(&right.lowered_name));
        let info = StructInfo {
            decls: Vec::new(),
            source_params: Vec::new(),
            conforms: Vec::new(),
            callable_conformance: None,
            callable_target: None,
            witness_tables: Vec::new(),
            conformance_conditions: HashMap::new(),
            fields: Vec::new(),
            declared_field_names: Vec::new(),
            field_origin_arguments: HashMap::new(),
            associated: HashMap::new(),
            associated_value_types: HashMap::new(),
            associated_constraints: HashMap::new(),
            parameterized_associated: HashMap::new(),
            methods: rows,
            fieldwise_init: false,
            explicit_destroy_message: None,
            explicit_destructors: HashMap::new(),
        };
        let declaration = StructDeclaration {
            name: "S".to_string(),
            fields: Vec::new(),
            mut_self_methods: HashSet::new(),
            fieldwise_init: false,
            param_decls: Vec::new(),
            explicit_destructors: HashMap::new(),
            explicit_destroy: None,
            conformances: Vec::new(),
            declared: Vec::new(),
            associated_types: Vec::new(),
            parameterized_associated_types: Vec::new(),
            associated_values: Vec::new(),
            parameterized_associated_values: Vec::new(),
            witness_tables: Vec::new(),
            members: Vec::new(),
        };
        (
            DeclarationInterface {
                structs: vec![("S".to_string(), info)],
                ..DeclarationInterface::default()
            },
            LoweredDeclaration {
                structs: vec![declaration],
                functions,
                name: "S".to_string(),
                ..LoweredDeclaration::default()
            },
        )
    }

    /// The overload qualifiers of each of `interface`'s struct's methods,
    /// by name, in row order.
    fn qualifiers(interface: &DeclarationInterface) -> BTreeMap<String, Vec<Option<String>>> {
        interface.structs[0]
            .1
            .methods
            .iter()
            .map(|(method, sigs)| {
                let qualifiers = sigs.iter().map(|sig| sig.overload.clone()).collect();
                (method.clone(), qualifiers)
            })
            .collect()
    }

    #[test]
    fn a_method_residue_names_its_function_and_joins_back_its_name_and_qualifier() {
        use mojito_symbol::symbol::OV_SEP;
        let (b, a) = (format!("{OV_SEP}b"), format!("{OV_SEP}a"));
        let methods = [
            ("f", Some(b.as_str())),
            ("f", Some(a.as_str())),
            ("pop", Some(OV_SEP)),
            ("g", None),
        ];
        let (mut residue, lowered) = struct_s(&methods);
        let expected = qualifiers(&residue);
        residue.project(&lowered).expect("the rows agree");
        assert!(residue.structs[0].1.methods.is_empty());
        let bytes = postcard::to_stdvec(&residue).expect("encodes");
        let mut decoded: DeclarationInterface = postcard::from_bytes(&bytes).expect("decodes");
        decoded
            .join(&[(Table::Structs, "S")], &lowered)
            .expect("joins");
        assert_eq!(qualifiers(&decoded), expected);
    }

    #[test]
    fn a_method_whose_function_would_not_split_back_is_refused() {
        let (mut residue, lowered) = struct_s(&[("f", Some("x"))]);
        let error = residue
            .project(&lowered)
            .expect_err("`S.fx` splits as method `fx`");
        assert!(error.contains("would not join back"), "{error}");
    }

    #[test]
    fn a_constant_value_is_held_once_and_joined_back() {
        let lowered = constant_n(3);
        let mut residue = residue_of_n(3);
        residue.project(&lowered).expect("the rows agree");
        assert!(residue.comptimes.is_empty() && residue.elaborated.is_empty());
        let bytes = postcard::to_stdvec(&residue).expect("encodes");
        let mut decoded: DeclarationInterface = postcard::from_bytes(&bytes).expect("decodes");
        decoded.join(&KEYS, &lowered).expect("joins");
        assert!(same_bytes(&decoded.comptimes, &residue_of_n(3).comptimes));
        assert!(same_bytes(&decoded.elaborated, &residue_of_n(3).elaborated));
    }

    #[test]
    fn a_lowered_constant_other_than_the_check_is_refused() {
        let error = residue_of_n(3)
            .project(&constant_n(4))
            .expect_err("the values differ");
        assert!(error.contains("the lowered constant `N`"), "{error}");
    }

    #[test]
    fn an_elaborated_value_other_than_the_check_is_refused() {
        let mut residue = residue_of_n(3);
        residue.elaborated[0].1 = elaborated_constant(&IntLiteral::from(4_i64));
        let error = residue
            .project(&constant_n(3))
            .expect_err("the elaboration differs");
        assert!(error.contains("the elaboration of `N`"), "{error}");
    }

    #[test]
    fn a_constant_key_with_neither_row_nor_lowered_value_is_refused() {
        let error = DeclarationInterface::default()
            .join(&KEYS, &LoweredDeclaration::default())
            .expect_err("the key has no row");
        assert!(error.contains("Comptimes"), "{error}");
    }
}
