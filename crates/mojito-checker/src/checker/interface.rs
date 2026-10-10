//! A package module's declaration interface: the checker's resolved tables
//! for the declarations one library module makes, by linked name.
//!
//! The package build's check exports one per module after it has checked
//! the whole library; an importer's check installs those of the modules its
//! link reached at the start of every pass, instead of registering their
//! declarations from source, as upstream's importer takes a package
//! declaration already resolved from its bytecode. An interface holds no
//! binding owner, span-keyed fact, syntax identity, or parameter hole: each
//! is one check's or one process's, and encoding a table that holds one
//! fails.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::{StmtKind, TypeParam};
use std::collections::BTreeMap;

/// The interfaces of the package modules a compilation's link reached,
/// decoded once and installed into every checker pass.
#[derive(Clone, Default)]
pub struct InstalledInterfaces(std::sync::Arc<Vec<ModuleInterface>>);

impl std::fmt::Debug for InstalledInterfaces {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "InstalledInterfaces({} modules)", self.0.len())
    }
}

impl InstalledInterfaces {
    /// Decode each `(module path, interface)`; the expressions an interface
    /// holds are stamped with the path its importer reached the module by.
    /// Any record that does not decode refuses them all.
    ///
    /// # Errors
    ///
    /// The first record that does not decode, by its module.
    pub fn decode<'a>(
        modules: impl IntoIterator<Item = (&'a str, &'a [u8])>,
    ) -> Result<Self, String> {
        modules
            .into_iter()
            .map(|(path, bytes)| {
                mojito_ast::ast::decoding_from(path, || postcard::from_bytes(bytes))
                    .map_err(|error| format!("the interface of `{path}` does not decode: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|interfaces| Self(std::sync::Arc::new(interfaces)))
    }

    /// Every module constant of the installed modules, as the build's
    /// comptime elaboration left it.
    pub fn elaborated(&self) -> impl Iterator<Item = (&str, &ElaboratedConstant)> {
        self.0.iter().flat_map(|interface| {
            interface
                .elaborated
                .iter()
                .map(|(name, constant)| (name.as_str(), constant))
        })
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

/// One library module's declarations, as the build's check resolved them.
/// Every table is sorted by name.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(super) struct ModuleInterface {
    structs: Vec<(String, StructInfo)>,
    traits: Vec<(String, TraitInfo)>,
    aliases: Vec<(String, ComptimeAlias)>,
    /// The module scope's bindings: every `def`, its overloads merged, and
    /// every module constant.
    bindings: Vec<ModuleBinding>,
    comptimes: Vec<(String, mojito_common::literal::IntLiteral)>,
    comptime_literals: Vec<(String, Expr)>,
    module_values: Vec<(String, CtValue)>,
    comptime_applied: Vec<(String, ParamExpr)>,
    comptime_dtypes: Vec<(String, SimdDtype)>,
    comptime_parameters: Vec<(String, (usize, ParamExpr))>,
    comptime_values: Vec<(String, Expr)>,
    type_aliases: Vec<(String, Ty)>,
    applicable_functions: Vec<(String, Vec<comptime_validation::ApplicableFunction>)>,
    def_results: Vec<(String, SourceType)>,
    owned_collectors: Vec<(String, bool)>,
    collector_names: Vec<(String, String)>,
    overloads: mojito_symbol::symbol::OverloadSets,
    declared_structs: Vec<String>,
    applied_module_constants: Vec<String>,
    nested_origins_read_only_functions: Vec<String>,
    simd_length_binders: Vec<ParamId>,
    /// The module's constants as the build's comptime elaboration resolved
    /// them.
    elaborated: Vec<(String, ElaboratedConstant)>,
}

impl ModuleInterface {
    fn sort(&mut self) {
        fn by_name<V>(entries: &mut [(String, V)]) {
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
        }
        by_name(&mut self.structs);
        by_name(&mut self.traits);
        by_name(&mut self.aliases);
        by_name(&mut self.comptimes);
        by_name(&mut self.comptime_literals);
        by_name(&mut self.module_values);
        by_name(&mut self.comptime_applied);
        by_name(&mut self.comptime_dtypes);
        by_name(&mut self.comptime_parameters);
        by_name(&mut self.comptime_values);
        by_name(&mut self.type_aliases);
        by_name(&mut self.applicable_functions);
        by_name(&mut self.def_results);
        by_name(&mut self.owned_collectors);
        by_name(&mut self.collector_names);
        by_name(&mut self.elaborated);
        self.bindings
            .sort_by(|left, right| left.name.cmp(&right.name));
        self.declared_structs.sort();
        self.applied_module_constants.sort();
        self.nested_origins_read_only_functions.sort();
        self.simd_length_binders.sort();
    }
}

impl Checker {
    /// The interface of every library module `program` declares something
    /// in, encoded, by module path, from this checker's tables after it
    /// checked `program` whole, with the module constants `elaborated`
    /// places in each.
    ///
    /// # Errors
    ///
    /// A module-scope fact the interface cannot carry, a name two modules
    /// declare, or a table that holds a binding owner or a hole, naming it.
    pub(super) fn export_interfaces(
        &self,
        program: &[Stmt],
        elaborated: &[(String, String, ElaboratedConstant)],
    ) -> Result<BTreeMap<String, Vec<u8>>, String> {
        let modules = DeclaringModules::of(program)?;
        let builtin = Self::new();
        let mut interfaces: BTreeMap<String, ModuleInterface> = modules
            .names
            .values()
            .map(|module| (module.clone(), ModuleInterface::default()))
            .collect();
        let mut unplaced = Vec::new();
        // The interface of the module declaring `name`; a name no library
        // module declares is a fresh checker's own, or reported.
        let mut place = |name: &str, builtin_has: bool| -> Option<String> {
            let module = modules.module_of(name);
            if module.is_none() && !builtin_has {
                unplaced.push(name.to_string());
            }
            module.map(str::to_string)
        };
        macro_rules! export_map {
            ($source:expr, $builtin:expr, $field:ident) => {
                for (name, value) in $source {
                    if let Some(interface) = place(name, $builtin.contains_key(name))
                        .and_then(|module| interfaces.get_mut(&module))
                    {
                        interface.$field.push((name.clone(), value.clone()));
                    }
                }
            };
        }
        macro_rules! export_set {
            ($source:expr, $builtin:expr, $field:ident) => {
                for name in $source {
                    if let Some(interface) = place(name, $builtin.contains(name))
                        .and_then(|module| interfaces.get_mut(&module))
                    {
                        interface.$field.push(name.clone());
                    }
                }
            };
        }
        export_map!(&self.structs, builtin.structs, structs);
        export_map!(&self.traits, builtin.traits, traits);
        export_map!(&self.comptime_aliases, builtin.comptime_aliases, aliases);
        export_map!(&self.comptimes, builtin.comptimes, comptimes);
        export_map!(
            &self.comptime_literals,
            builtin.comptime_literals,
            comptime_literals
        );
        export_map!(&self.module_values, builtin.module_values, module_values);
        export_map!(
            &self.comptime_applied,
            builtin.comptime_applied,
            comptime_applied
        );
        export_map!(
            &module_scope(&self.comptime_dtypes),
            module_scope(&builtin.comptime_dtypes),
            comptime_dtypes
        );
        export_map!(
            &module_scope(&self.local_comptime_parameters),
            module_scope(&builtin.local_comptime_parameters),
            comptime_parameters
        );
        export_map!(
            &module_scope(&self.local_comptime_values),
            module_scope(&builtin.local_comptime_values),
            comptime_values
        );
        export_map!(
            &module_scope(&self.local_type_aliases),
            module_scope(&builtin.local_type_aliases),
            type_aliases
        );
        export_map!(
            &self.applicable_functions,
            builtin.applicable_functions,
            applicable_functions
        );
        export_map!(
            &self.module_def_results,
            builtin.module_def_results,
            def_results
        );
        export_map!(
            &self.owned_collectors,
            builtin.owned_collectors,
            owned_collectors
        );
        export_map!(
            &self.collector_names,
            builtin.collector_names,
            collector_names
        );
        export_set!(
            &self.declared_structs,
            builtin.declared_structs,
            declared_structs
        );
        export_set!(
            &self.applied_module_constants,
            builtin.applied_module_constants,
            applied_module_constants
        );
        export_set!(
            &self.nested_origins_read_only_functions,
            builtin.nested_origins_read_only_functions,
            nested_origins_read_only_functions
        );
        for binder in &self.simd_length_binders {
            if let Some(interface) =
                place(&binder.owner, builtin.simd_length_binders.contains(binder))
                    .and_then(|module| interfaces.get_mut(&module))
            {
                interface.simd_length_binders.push(binder.clone());
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
            if let Some(interface) = place(name, builtin_scope.contains_key(name))
                .and_then(|module| interfaces.get_mut(&module))
            {
                interface.bindings.push(ModuleBinding {
                    name: name.clone(),
                    ty: ty.clone(),
                    mutable: mutable.get(name).copied().unwrap_or(false),
                    callable_origins: origins.get(name).cloned(),
                    comptime: owners
                        .get(name)
                        .is_some_and(|owner| self.comptime_binding_owners.contains(owner)),
                    compile_time: compile_time.contains(name),
                });
            }
        }
        for (module, name, constant) in elaborated {
            interfaces
                .entry(module.clone())
                .or_default()
                .elaborated
                .push((name.clone(), constant.clone()));
        }
        for (module, interface) in &mut interfaces {
            interface.overloads = self
                .overload_sets
                .restricted(|name| modules.module_of(name) == Some(module.as_str()));
        }
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
        if !unplaced.is_empty() {
            unplaced.sort();
            unplaced.dedup();
            return Err(format!(
                "no library module declares {}",
                unplaced.join(", ")
            ));
        }
        interfaces
            .into_iter()
            .map(|(module, mut interface)| {
                interface.sort();
                postcard::to_stdvec(&interface)
                    .map(|bytes| (module.clone(), bytes))
                    .map_err(|error| {
                        format!("the interface of `{module}` cannot be exported: {error}")
                    })
            })
            .collect()
    }

    /// Install `installed`'s declarations, as checking their modules'
    /// statements would have registered them.
    ///
    /// # Errors
    ///
    /// A name already declared, which an importer's link rules out.
    pub(super) fn install_interfaces(
        &mut self,
        installed: &InstalledInterfaces,
    ) -> Result<(), TypeError> {
        let _install = timing::span("install_interfaces");
        for interface in installed.0.iter() {
            self.install_interface(interface)?;
        }
        Ok(())
    }

    fn install_interface(&mut self, interface: &ModuleInterface) -> Result<(), TypeError> {
        fn extend<V: Clone>(map: &mut HashMap<String, V>, entries: &[(String, V)]) {
            map.extend(entries.iter().cloned());
        }
        fn module_scope_mut<V>(
            stack: &mut [HashMap<String, V>],
        ) -> Result<&mut HashMap<String, V>, TypeError> {
            stack.first_mut().ok_or_else(|| {
                TypeError::InvariantViolation("checker scope stack is empty".to_string())
            })
        }
        extend(&mut self.structs, &interface.structs);
        extend(&mut self.traits, &interface.traits);
        extend(&mut self.comptime_aliases, &interface.aliases);
        extend(&mut self.comptimes, &interface.comptimes);
        extend(&mut self.comptime_literals, &interface.comptime_literals);
        extend(&mut self.module_values, &interface.module_values);
        extend(&mut self.comptime_applied, &interface.comptime_applied);
        extend(
            module_scope_mut(&mut self.comptime_dtypes)?,
            &interface.comptime_dtypes,
        );
        extend(
            module_scope_mut(&mut self.local_comptime_parameters)?,
            &interface.comptime_parameters,
        );
        extend(
            module_scope_mut(&mut self.local_comptime_values)?,
            &interface.comptime_values,
        );
        extend(
            module_scope_mut(&mut self.local_type_aliases)?,
            &interface.type_aliases,
        );
        extend(
            &mut self.applicable_functions,
            &interface.applicable_functions,
        );
        extend(&mut self.module_def_results, &interface.def_results);
        extend(&mut self.owned_collectors, &interface.owned_collectors);
        extend(&mut self.collector_names, &interface.collector_names);
        self.overload_sets.extend(interface.overloads.clone());
        self.declared_structs
            .extend(interface.declared_structs.iter().cloned());
        self.applied_module_constants
            .extend(interface.applied_module_constants.iter().cloned());
        self.nested_origins_read_only_functions
            .extend(interface.nested_origins_read_only_functions.iter().cloned());
        self.simd_length_binders
            .extend(interface.simd_length_binders.iter().cloned());
        for binding in &interface.bindings {
            let owner = self.fresh_owner()?;
            if module_scope_mut(&mut self.scopes)?
                .insert(binding.name.clone(), binding.ty.clone())
                .is_some()
            {
                return Err(TypeError::Redeclaration(binding.name.clone()));
            }
            module_scope_mut(&mut self.mutable_scopes)?
                .insert(binding.name.clone(), binding.mutable);
            module_scope_mut(&mut self.owner_scopes)?.insert(binding.name.clone(), owner);
            if let Some(origins) = &binding.callable_origins {
                module_scope_mut(&mut self.callable_origin_scopes)?
                    .insert(binding.name.clone(), origins.clone());
            }
            if binding.comptime {
                self.comptime_binding_owners.insert(owner);
            }
            if binding.compile_time
                && let Some(bindings) = self.compile_time_bindings.first_mut()
            {
                bindings.insert(binding.name.clone());
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

/// One name bound at module scope.
#[derive(serde::Serialize, serde::Deserialize)]
struct ModuleBinding {
    name: String,
    ty: Ty,
    mutable: bool,
    /// One entry per overload declaration, as `callable_origin_scopes` keeps.
    callable_origins: Option<Vec<CallableOriginSignature>>,
    /// Whether it is a module `comptime` binding a compile-time expression
    /// may read (`comptime_binding_owners`).
    comptime: bool,
    /// Whether it is a compile-time binding (`compile_time_bindings`).
    compile_time: bool,
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

    /// The module declaring the table key `name`: a declaration's own name,
    /// an overload's signature-qualified one, or a member's `Owner.member`.
    fn module_of(&self, name: &str) -> Option<&str> {
        self.names
            .get(name)
            .or_else(|| {
                let (declaration, _) = name.split_once(mojito_symbol::symbol::OV_SEP)?;
                self.names.get(declaration)
            })
            .or_else(|| {
                let (owner, _) = name.split_once('.')?;
                self.names.get(owner)
            })
            .map(String::as_str)
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
