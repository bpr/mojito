//! A package's declaration interfaces: the checker's resolved table rows
//! for each declaration a library module makes, by linked name.
//!
//! The package build's check exports one record per library declaration
//! after it has checked the whole library, beside a directory naming every
//! table key each record holds. An importer's check attaches the records of
//! the modules its link reached instead of registering their declarations
//! from source, and resolves a record from its bytecode the first time a
//! lookup its own tables miss asks for one of its keys, as upstream's
//! importer resolves a package declaration on first lookup. A record holds
//! no binding owner, span-keyed fact, syntax identity, or parameter hole:
//! each is one check's or one process's, and encoding a table that holds
//! one fails.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::{StmtKind, TypeParam};
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock};

/// A package's encoded declaration interfaces and their directory, read
/// once per process; no record decodes here.
pub struct DeclarationInterfaces {
    bytes: Vec<u8>,
    /// Where the records start in `bytes`.
    records: usize,
    directory: InterfaceDirectory,
    /// Per table, each key's declaration.
    keys: Vec<HashMap<String, u32>>,
    /// Each module binding's module and its position among the module's
    /// bindings, which are sorted by name.
    bindings: HashMap<String, (u32, u32)>,
    /// Each module's bindings, sorted by name.
    module_bindings: Vec<Vec<String>>,
    /// Each module's position, by path.
    modules: HashMap<String, u32>,
    /// Per method name, each declaration whose struct declares it.
    methods: HashMap<String, Vec<u32>>,
}

impl std::fmt::Debug for DeclarationInterfaces {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "DeclarationInterfaces({} modules, {} declarations)",
            self.directory.modules.len(),
            self.directory.declarations.len()
        )
    }
}

impl DeclarationInterfaces {
    /// Read the directory of the interfaces a package build's check
    /// exported ([`CheckContext::take_interfaces`]).
    ///
    /// # Errors
    ///
    /// A directory that does not decode or names a record outside `bytes`.
    pub fn read(bytes: Vec<u8>) -> Result<Self, String> {
        let truncated = || "the declaration interfaces are truncated".to_string();
        let (length, rest) = bytes.split_first_chunk::<8>().ok_or_else(truncated)?;
        let length = usize::try_from(u64::from_le_bytes(*length))
            .ok()
            .filter(|length| *length <= rest.len())
            .ok_or_else(truncated)?;
        let directory: InterfaceDirectory = postcard::from_bytes(&rest[..length])
            .map_err(|error| format!("the interface directory does not decode: {error}"))?;
        let records = 8 + length;
        let payload = bytes.len() - records;
        let ranges = directory
            .modules
            .iter()
            .map(|module| &module.record)
            .chain(directory.declarations.iter().map(|entry| &entry.record));
        for range in ranges {
            if range.start > range.end || range.end > payload {
                return Err("an interface record overruns the payload".to_string());
            }
        }
        let mut keys = vec![HashMap::new(); Table::COUNT];
        let mut module_bindings = vec![Vec::new(); directory.modules.len()];
        let mut methods: HashMap<String, Vec<u32>> = HashMap::new();
        for (position, entry) in directory.declarations.iter().enumerate() {
            let module = module_bindings
                .get_mut(entry.module as usize)
                .ok_or_else(|| format!("`{}` names no module", entry.name))?;
            for (table, key) in &entry.keys {
                keys[*table as usize].insert(key.clone(), position as u32);
                if *table == Table::Bindings {
                    module.push(key.clone());
                }
            }
            for method in &entry.methods {
                methods
                    .entry(method.clone())
                    .or_default()
                    .push(position as u32);
            }
        }
        let mut bindings = HashMap::new();
        for (module, names) in module_bindings.iter_mut().enumerate() {
            names.sort();
            for (position, name) in names.iter().enumerate() {
                bindings.insert(name.clone(), (module as u32, position as u32));
            }
        }
        let modules = directory
            .modules
            .iter()
            .enumerate()
            .map(|(position, module)| (module.path.clone(), position as u32))
            .collect();
        Ok(Self {
            bytes,
            records,
            directory,
            keys,
            bindings,
            module_bindings,
            modules,
            methods,
        })
    }

    /// Whether `module` has an interface here.
    pub fn declares(&self, module: &str) -> bool {
        self.modules.contains_key(module)
    }

    fn record(&self, range: &Range<usize>) -> &[u8] {
        &self.bytes[self.records + range.start..self.records + range.end]
    }
}

/// The declaration interfaces of the package modules a compilation's link
/// reached, attached to every checker pass and to the elaborator's
/// conformance oracle.
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
    /// The interfaces of `modules`, in the order the link reached them,
    /// from `package`: each module's own record (its overload sets, its
    /// elaborated constants, and its module-scope compile-time facts)
    /// decodes now, and no declaration's.
    ///
    /// # Errors
    ///
    /// The first module record that does not decode.
    pub fn reaching<'a>(
        package: &Arc<DeclarationInterfaces>,
        modules: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, String> {
        let count = package.directory.modules.len();
        let mut reached = vec![false; count];
        let mut reached_order = Vec::new();
        let mut binding_bases = vec![None; count];
        let mut binding_count = 0u32;
        let mut records = Vec::new();
        let mut overloads = mojito_symbol::symbol::OverloadSets::default();
        for path in modules {
            let Some(&module) = package.modules.get(path) else {
                continue;
            };
            if reached[module as usize] {
                continue;
            }
            reached[module as usize] = true;
            reached_order.push(module);
            binding_bases[module as usize] = Some(binding_count);
            binding_count += package.module_bindings[module as usize].len() as u32;
            let entry = &package.directory.modules[module as usize];
            let record: ModuleRecord = mojito_ast::ast::decoding_from(path, || {
                postcard::from_bytes(package.record(&entry.record))
            })
            .map_err(|error| format!("the interface of `{path}` does not decode: {error}"))?;
            overloads.extend(record.overloads.clone());
            records.push(record);
        }
        let decoded = package
            .directory
            .declarations
            .iter()
            .map(|_| OnceLock::new())
            .collect();
        Ok(Self(Arc::new(Installed {
            package: Arc::clone(package),
            reached,
            reached_order,
            binding_bases,
            binding_count,
            records,
            overloads,
            decoded,
            failures: Mutex::new(Vec::new()),
        })))
    }

    /// Every module constant of the reached modules, as the build's
    /// comptime elaboration left it.
    pub fn elaborated(&self) -> impl Iterator<Item = (&str, &ElaboratedConstant)> {
        self.0.records.iter().flat_map(|record| {
            record
                .elaborated
                .iter()
                .map(|(name, constant)| (name.as_str(), constant))
        })
    }

    /// How many declaration records have decoded.
    pub fn decoded(&self) -> usize {
        self.0
            .decoded
            .iter()
            .filter(|record| record.get().is_some())
            .count()
    }

    /// Why each declaration record a lookup asked for and that did not
    /// decode failed.
    pub fn failures(&self) -> Vec<String> {
        self.0
            .failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The record of the declaration `table`'s `key` belongs to, decoded on
    /// first use; `None` when no reached module declares the key, or its
    /// record does not decode (a failure [`Self::failures`] reports).
    pub(super) fn declaration(&self, table: Table, key: &str) -> Option<&DeclarationInterface> {
        let position = self.position(table, key)?;
        self.0.decoded[position]
            .get_or_init(|| {
                let package = &self.0.package;
                let entry = &package.directory.declarations[position];
                let path = &package.directory.modules[entry.module as usize].path;
                mojito_ast::ast::decoding_from(path, || {
                    postcard::from_bytes(package.record(&entry.record))
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
            .as_ref()
    }

    /// Whether a reached module's declaration holds `table`'s `key`,
    /// decoding nothing.
    pub(super) fn has_key(&self, table: Table, key: &str) -> bool {
        self.position(table, key).is_some()
    }

    /// Every key of `table` a reached module's declaration holds, decoding
    /// nothing.
    pub(super) fn keys(&self, table: Table) -> impl Iterator<Item = &String> {
        self.0.package.keys[table as usize]
            .iter()
            .filter(|(_, position)| self.reached_declaration(**position))
            .map(|(key, _)| key)
    }

    /// The struct keys of every reached declaration whose struct declares
    /// `method`, decoding nothing.
    pub(super) fn structs_declaring(&self, method: &str) -> impl Iterator<Item = &String> {
        let package = &self.0.package;
        package
            .methods
            .get(method)
            .into_iter()
            .flatten()
            .filter(|position| self.reached_declaration(**position))
            .flat_map(|position| &package.directory.declarations[*position as usize].keys)
            .filter(|(table, _)| *table == Table::Structs)
            .map(|(_, key)| key)
    }

    /// The module binding `name` and its ordinal among the reached
    /// modules' bindings: by module in the order the link reached them,
    /// then by name.
    pub(super) fn binding(&self, name: &str) -> Option<(u32, &ModuleBinding)> {
        let (module, position) = *self.0.package.bindings.get(name)?;
        let base = self.0.binding_bases[module as usize]?;
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
            let names = &self.0.package.module_bindings[*module as usize];
            names
                .get(ordinal.checked_sub(base)? as usize)
                .map(String::as_str)
        })
    }

    /// How many module bindings the reached modules make.
    pub(super) fn binding_count(&self) -> u32 {
        self.0.binding_count
    }

    fn position(&self, table: Table, key: &str) -> Option<usize> {
        let position = *self.0.package.keys[table as usize].get(key)?;
        self.reached_declaration(position)
            .then_some(position as usize)
    }

    fn reached_declaration(&self, position: u32) -> bool {
        let module = self.0.package.directory.declarations[position as usize].module;
        self.0.reached[module as usize]
    }
}

/// A module `comptime` declaration as the build's comptime elaboration
/// resolved it, which an importer's elaboration reads instead of folding the
/// declaration again.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum ElaboratedConstant {
    /// A closed constant's folded value, or the marker of one the check
    /// binds.
    Value(CtValue),
    /// A generic alias: its parameters and body.
    Alias(Vec<TypeParam>, Expr),
}

/// The elaborated module constants of a package's program, each beside its
/// name and the module declaring it.
pub type ElaboratedConstants = Vec<(String, String, ElaboratedConstant)>;

/// A checker table a declaration record holds rows of, whose every key the
/// directory lists.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub(super) enum Table {
    Structs,
    Traits,
    Aliases,
    Comptimes,
    ComptimeLiterals,
    ModuleValues,
    ComptimeApplied,
    ApplicableFunctions,
    DefResults,
    OwnedCollectors,
    CollectorNames,
    DeclaredStructs,
    AppliedModuleConstants,
    NestedOriginsReadOnlyFunctions,
    ExplicitDestroy,
    Bindings,
}

impl Table {
    const COUNT: usize = Self::Bindings as usize + 1;
}

/// One library declaration's rows of the checker's tables, as the build's
/// check resolved them. Every table is sorted by key; a name-set table
/// (`declared_structs` and the like) is only keys in the directory.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(super) struct DeclarationInterface {
    structs: Vec<(String, StructInfo)>,
    traits: Vec<(String, TraitInfo)>,
    aliases: Vec<(String, ComptimeAlias)>,
    comptimes: Vec<(String, mojito_common::literal::IntLiteral)>,
    comptime_literals: Vec<(String, Expr)>,
    module_values: Vec<(String, CtValue)>,
    comptime_applied: Vec<(String, ParamExpr)>,
    applicable_functions: Vec<(String, Vec<comptime_validation::ApplicableFunction>)>,
    def_results: Vec<(String, SourceType)>,
    owned_collectors: Vec<(String, bool)>,
    collector_names: Vec<(String, String)>,
    /// The explicit-destruction obligation of a struct whose self-type is
    /// not `Deinitable`.
    explicit_destroy: Vec<(String, mojito_checked::checked::ExplicitDestroyInfo)>,
    /// The declaration's module-scope bindings: a `def`, its overloads
    /// merged, or a module constant.
    bindings: Vec<ModuleBinding>,
}

/// One name bound at module scope.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct ModuleBinding {
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
    /// The declaration interfaces of every library module `program`
    /// declares something in, encoded with their directory, from this
    /// checker's tables after it checked `program` whole, with the module
    /// constants `elaborated` places in each module.
    ///
    /// # Errors
    ///
    /// A module-scope fact the interface cannot carry, a name two modules
    /// declare, or a table that holds a binding owner or a hole, naming it.
    pub(super) fn export_interfaces(
        &self,
        program: &[Stmt],
        elaborated: &[(String, String, ElaboratedConstant)],
    ) -> Result<Vec<u8>, String> {
        let modules = DeclaringModules::of(program)?;
        let builtin = Self::new();
        let mut records: BTreeMap<String, ModuleRecord> = modules
            .names
            .values()
            .map(|module| (module.clone(), ModuleRecord::default()))
            .collect();
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
                            .push(($table, name.clone()));
                    }
                }
            };
        }
        macro_rules! export_module_scope {
            ($source:expr, $builtin:expr, $field:ident) => {
                let builtin_scope = module_scope(&$builtin);
                for (name, value) in module_scope(&$source) {
                    if place(&name, builtin_scope.contains_key(&name)).is_some()
                        && let Some(record) = modules
                            .module_of(&name)
                            .and_then(|module| records.get_mut(module))
                    {
                        record.$field.push((name, value));
                    }
                }
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
            comptime_dtypes
        );
        export_module_scope!(
            self.local_comptime_parameters,
            builtin.local_comptime_parameters,
            comptime_parameters
        );
        export_module_scope!(
            self.local_comptime_values,
            builtin.local_comptime_values,
            comptime_values
        );
        export_module_scope!(
            self.local_type_aliases,
            builtin.local_type_aliases,
            type_aliases
        );
        for binder in &self.simd_length_binders {
            if place(&binder.owner, builtin.simd_length_binders.contains(binder)).is_some()
                && let Some(record) = modules
                    .module_of(&binder.owner)
                    .and_then(|module| records.get_mut(module))
            {
                record.simd_length_binders.push(binder.clone());
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
        for (module, name, constant) in elaborated {
            records
                .entry(module.clone())
                .or_default()
                .elaborated
                .push((name.clone(), constant.clone()));
        }
        for (module, record) in &mut records {
            record.overloads = self
                .overload_sets
                .restricted(|name| modules.module_of(name) == Some(module.as_str()));
        }
        self.refuse_uncarried(&modules)?;
        if !unplaced.is_empty() {
            unplaced.sort();
            unplaced.dedup();
            return Err(format!(
                "no library module declares {}",
                unplaced.join(", ")
            ));
        }
        encode_interfaces(&modules, records, declarations)
    }

    /// Resolve every table lookup the program's own declarations miss from
    /// `installed`, and bind the reached modules' bindings below the module
    /// scope, with a block of owners reserved for them in their install
    /// order. Only each module's own record is copied in: its overload sets
    /// and its module-scope compile-time facts.
    ///
    /// # Errors
    ///
    /// The checker's binding identities run out.
    pub(super) fn attach_interfaces(
        &mut self,
        installed: &InstalledInterfaces,
    ) -> Result<(), TypeError> {
        fn extend<V: Clone>(
            stack: &mut [HashMap<String, V>],
            entries: &[(String, V)],
        ) -> Result<(), TypeError> {
            stack
                .first_mut()
                .ok_or_else(|| {
                    TypeError::InvariantViolation("checker scope stack is empty".to_string())
                })?
                .extend(entries.iter().cloned());
            Ok(())
        }
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
        self.overload_sets.extend(installed.0.overloads.clone());
        for record in &installed.0.records {
            extend(&mut self.comptime_dtypes, &record.comptime_dtypes)?;
            extend(
                &mut self.local_comptime_parameters,
                &record.comptime_parameters,
            )?;
            extend(&mut self.local_comptime_values, &record.comptime_values)?;
            extend(&mut self.local_type_aliases, &record.type_aliases)?;
            self.simd_length_binders
                .extend(record.simd_length_binders.iter().cloned());
        }
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

/// What a compilation reached of a package's declaration interfaces.
struct Installed {
    package: Arc<DeclarationInterfaces>,
    /// Per module, whether the link reached it.
    reached: Vec<bool>,
    /// The reached modules, in the order the link reached them.
    reached_order: Vec<u32>,
    /// Per reached module, the ordinal of its first binding.
    binding_bases: Vec<Option<u32>>,
    binding_count: u32,
    /// The reached modules' own records, in reach order.
    records: Vec<ModuleRecord>,
    /// The reached modules' overload sets, merged.
    overloads: mojito_symbol::symbol::OverloadSets,
    /// Per declaration, its record once a lookup asked for it.
    decoded: Vec<OnceLock<Option<DeclarationInterface>>>,
    failures: Mutex<Vec<String>>,
}

/// Where each record of a package's declaration interfaces lies.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct InterfaceDirectory {
    /// Each library module's own record, by path.
    modules: Vec<ModuleEntry>,
    /// Each library declaration's record, sorted by name.
    declarations: Vec<DeclarationEntry>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ModuleEntry {
    path: String,
    record: Range<usize>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct DeclarationEntry {
    name: String,
    /// Its module's position in the directory.
    module: u32,
    record: Range<usize>,
    /// Every table key its record holds, sorted.
    keys: Vec<(Table, String)>,
    /// Every method its struct declares, sorted.
    methods: Vec<String>,
}

/// One library module's facts an importer reads whole: the overload sets
/// MIR lowering asks about by name, the constants the importer's
/// elaboration reads, and the module scope's compile-time facts.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct ModuleRecord {
    overloads: mojito_symbol::symbol::OverloadSets,
    /// The module's constants as the build's comptime elaboration resolved
    /// them.
    elaborated: Vec<(String, ElaboratedConstant)>,
    comptime_dtypes: Vec<(String, SimdDtype)>,
    comptime_parameters: Vec<(String, (usize, ParamExpr))>,
    comptime_values: Vec<(String, Expr)>,
    type_aliases: Vec<(String, Ty)>,
    simd_length_binders: Vec<ParamId>,
}

impl ModuleRecord {
    fn sort(&mut self) {
        by_name(&mut self.elaborated);
        by_name(&mut self.comptime_dtypes);
        by_name(&mut self.comptime_parameters);
        by_name(&mut self.comptime_values);
        by_name(&mut self.type_aliases);
        self.simd_length_binders.sort();
    }
}

impl DeclarationInterface {
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
        self.bindings
            .sort_by(|left, right| left.name.cmp(&right.name));
    }
}

/// The module each module-level name of a program is declared in.
struct DeclaringModules {
    names: HashMap<String, String>,
}

impl DeclaringModules {
    /// The declaring module of every name `program` declares at module
    /// level. A name two modules declare (overloads split across modules)
    /// has no one module to install it from, and is refused.
    fn of(program: &[Stmt]) -> Result<Self, String> {
        let mut names: HashMap<String, String> = HashMap::new();
        for statement in program {
            let (StmtKind::Def { name, .. }
            | StmtKind::Struct { name, .. }
            | StmtKind::Trait { name, .. }
            | StmtKind::Comptime { name, .. }) = &statement.kind
            else {
                continue;
            };
            let Some(module) = &statement.module else {
                continue;
            };
            match names.get(name) {
                Some(declared) if declared != module => {
                    return Err(format!(
                        "`{name}` is declared in both `{declared}` and `{module}`"
                    ));
                }
                Some(_) => {}
                None => {
                    names.insert(name.clone(), module.clone());
                }
            }
        }
        Ok(Self { names })
    }

    /// The module declaring the table key `name` ([`Self::declaration_of`]).
    fn module_of(&self, name: &str) -> Option<&str> {
        self.declaration_of(name)
            .and_then(|declaration| self.names.get(declaration))
            .map(String::as_str)
    }

    /// The declaration the table key `name` belongs to: a declaration's own
    /// name, an overload's signature-qualified one, or a member's
    /// `Owner.member`.
    fn declaration_of(&self, name: &str) -> Option<&str> {
        [
            Some(name),
            name.split_once(mojito_symbol::symbol::OV_SEP)
                .map(|(declaration, _)| declaration),
            name.split_once('.').map(|(owner, _)| owner),
        ]
        .into_iter()
        .flatten()
        .find_map(|declaration| self.names.get_key_value(declaration))
        .map(|(declaration, _)| declaration.as_str())
    }
}

/// Encode the module `records` and the declaration records with their
/// directory: its length, eight bytes little-endian, then the directory,
/// then every record.
fn encode_interfaces(
    modules: &DeclaringModules,
    records: BTreeMap<String, ModuleRecord>,
    declarations: BTreeMap<String, (DeclarationInterface, Vec<(Table, String)>)>,
) -> Result<Vec<u8>, String> {
    let mut payload = Vec::new();
    let mut append = |bytes: Vec<u8>| {
        let start = payload.len();
        payload.extend(bytes);
        start..payload.len()
    };
    let positions: HashMap<String, u32> = records
        .keys()
        .enumerate()
        .map(|(position, module)| (module.clone(), position as u32))
        .collect();
    let mut directory = InterfaceDirectory::default();
    for (module, mut record) in records {
        record.sort();
        let bytes = postcard::to_stdvec(&record)
            .map_err(|error| format!("the interface of `{module}` cannot be exported: {error}"))?;
        directory.modules.push(ModuleEntry {
            path: module,
            record: append(bytes),
        });
    }
    for (name, (mut record, mut keys)) in declarations {
        record.sort();
        keys.sort();
        let mut methods: Vec<String> = record
            .structs
            .iter()
            .flat_map(|(_, info)| info.methods.keys().cloned())
            .collect();
        methods.sort();
        methods.dedup();
        let bytes = postcard::to_stdvec(&record)
            .map_err(|error| format!("the interface of `{name}` cannot be exported: {error}"))?;
        let module = modules
            .names
            .get(&name)
            .and_then(|module| positions.get(module))
            .copied()
            .ok_or_else(|| format!("`{name}` has no module record"))?;
        directory.declarations.push(DeclarationEntry {
            name,
            module,
            record: append(bytes),
            keys,
            methods,
        });
    }
    let directory = postcard::to_stdvec(&directory)
        .map_err(|error| format!("the interface directory cannot be exported: {error}"))?;
    let mut bytes = (directory.len() as u64).to_le_bytes().to_vec();
    bytes.extend(directory);
    bytes.extend(payload);
    Ok(bytes)
}

/// Sort `entries` by name.
fn by_name<V>(entries: &mut [(String, V)]) {
    entries.sort_by(|(left, _), (right, _)| left.cmp(right));
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
