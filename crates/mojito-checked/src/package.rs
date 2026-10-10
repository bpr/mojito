//! A compiled package's declarations: one record per library declaration,
//! under one directory, which every phase of an importer reads.
//!
//! A record holds the declaration's lowered part — the checked struct and
//! function declarations it lowers to, as MIR and the backends consume
//! them, and its callables' effect summaries — followed by its checker
//! residue: what of the checker's table rows for the declaration the
//! lowered part does not say, which only the check reads, and which the
//! check joins with the lowered part when it decodes the residue. The
//! lowered part holds no path-stamped syntax, so it decodes once per
//! process, the first time any phase looks one of its names up; the residue
//! holds expressions stamped with the path an importer reached the module
//! by, so each compilation decodes it itself. As upstream's importer reads
//! each declaration of a package from one op, no phase reads a second copy
//! of a declaration.
//!
//! The lowered part also holds the declaration's overload facts, which MIR
//! and the check both ask by name: an importer's program asks its own sets
//! first and the package's owning declaration on a miss
//! ([`ProgramOverloads`]).
//!
//! The directory also holds the package's one module table: each library
//! module, by dotted name, beside its source file's identity and the record
//! its importer's link reads (its loads, exports, and namespaces), which
//! every declaration names by position. A module's compile-time facts and
//! constants are each its declaration's.

use crate::checked::{CallThroughEffect, TransferEffect};
use crate::declaration::{FunctionDeclaration, StructDeclaration, TraitDeclaration};
use mojito_ast::ast::{Stmt, StmtKind};
use mojito_symbol::symbol::{LiftedOwner, OV_SEP, OverloadLookup, OverloadSets, lifted_owner};
use std::collections::{BTreeMap, HashMap};
use std::ops::{Deref, Range};
use std::sync::{Arc, Mutex, OnceLock};

/// A package file's bytes: mapped from the file, or owned when the package
/// was just built.
#[derive(Debug)]
pub enum PackageBytes {
    Mapped(memmap2::Mmap),
    Owned(Vec<u8>),
}

impl PackageBytes {
    /// Map `file` read-only.
    pub fn map(file: &std::fs::File) -> std::io::Result<Self> {
        // SAFETY: a cached package is written to a private partial file and
        // renamed over its path, and removed only by unlinking, so no
        // process writes or truncates a package file in place: the mapped
        // bytes stay fixed for the mapping's life.
        unsafe { memmap2::Mmap::map(file) }.map(Self::Mapped)
    }
}

impl Deref for PackageBytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match self {
            Self::Mapped(map) => map,
            Self::Owned(bytes) => bytes,
        }
    }
}

/// A package's payload, shared by its declarations and its bodies.
#[derive(Debug, Clone)]
pub struct Payload {
    bytes: Arc<PackageBytes>,
    start: usize,
}

impl Payload {
    /// The payload of `bytes`, which starts at `start`.
    pub fn new(bytes: PackageBytes, start: usize) -> Self {
        Self {
            bytes: Arc::new(bytes),
            start,
        }
    }

    /// The payload's length.
    pub fn len(&self) -> usize {
        self.bytes.len() - self.start
    }

    /// Whether the payload is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The record at `range`, which the directory's validation keeps in
    /// bounds.
    pub fn get(&self, range: &Range<usize>) -> &[u8] {
        &self.bytes[self.start + range.start..self.start + range.end]
    }
}

/// A checker table a declaration's residue holds rows of, whose every key
/// the directory lists.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum InterfaceTable {
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
    /// A module constant as the build's comptime elaboration resolved it.
    Elaborated,
    /// The module scope's compile-time facts, each under its binding's
    /// name: a bound dtype, a lifted constant's application, a kept
    /// compile-time value, and a type alias.
    ComptimeDtypes,
    ComptimeParameters,
    ComptimeValues,
    TypeAliases,
    /// The `SIMDLength` binders a declaration owns, under the owner.
    SimdLengthBinders,
}

impl InterfaceTable {
    pub const COUNT: usize = Self::SimdLengthBinders as usize + 1;
}

/// The lowered part of a declaration's record.
///
/// Every struct and function declaration it lowers to — its own, its
/// methods, and the functions nested in or lifted from it — and the effect
/// summaries of its callables. [`DeclarationRecords`] keeps every list
/// sorted.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct LoweredDeclaration {
    /// Sorted by name.
    pub structs: Vec<StructDeclaration>,
    /// Sorted by lowered name.
    pub functions: Vec<FunctionDeclaration>,
    /// Its trait, with the traits it refines; sorted by name.
    pub traits: Vec<TraitDeclaration>,
    /// The transfer effects the package's bodies record for each callable
    /// it declares, by callable identity, sorted.
    pub transfer_effects: Vec<(String, Vec<TransferEffect>)>,
    /// The call-through effects of each callable it declares, likewise.
    pub call_through_effects: Vec<(String, Vec<CallThroughEffect>)>,
    /// The overload facts of the names it owns: its functions' arities, its
    /// methods' and its trait's requirements', its struct's fieldwise
    /// initializer, and its constant's integer value.
    pub overloads: OverloadSets,
}

impl LoweredDeclaration {
    /// The transfer effects of the callable `name`.
    pub fn transfer_effects(&self, name: &str) -> Option<&Vec<TransferEffect>> {
        sorted_get(&self.transfer_effects, name)
    }

    /// The call-through effects of the callable `name`.
    pub fn call_through_effects(&self, name: &str) -> Option<&Vec<CallThroughEffect>> {
        sorted_get(&self.call_through_effects, name)
    }
}

/// Where every declaration record of a package lies in its payload.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct DeclarationDirectory {
    /// Each library module, sorted by dotted name: the package's module
    /// table.
    pub modules: Vec<ModuleEntry>,
    /// Each library declaration's record, sorted by name.
    pub declarations: Vec<DeclarationEntry>,
    /// Every struct declaration, as its declaration's position and its
    /// position in that record, sorted by struct name: the package order.
    pub structs: Vec<(u32, u32)>,
    /// Every function declaration, likewise, sorted by lowered name.
    pub functions: Vec<(u32, u32)>,
}

impl DeclarationDirectory {
    /// Refuse a directory whose records leave a payload of `length` bytes,
    /// whose names are not strictly sorted, which the lookups by name rely
    /// on, or whose positions name nothing.
    fn validate(&self, length: usize) -> Result<(), String> {
        let ranges = self
            .declarations
            .iter()
            .map(|entry| &entry.record)
            .chain(self.modules.iter().map(|module| &module.link));
        for range in ranges {
            if range.start > range.end || range.end > length {
                return Err("a declaration or module record overruns the payload".to_string());
            }
        }
        if !self
            .modules
            .windows(2)
            .all(|pair| pair[0].name < pair[1].name)
        {
            return Err("the modules are not strictly sorted".to_string());
        }
        for entry in &self.declarations {
            if entry.residue > entry.record.len() || entry.module as usize >= self.modules.len() {
                return Err(format!("the record of `{}` is malformed", entry.name));
            }
        }
        if !self
            .declarations
            .windows(2)
            .all(|pair| pair[0].name < pair[1].name)
        {
            return Err("the declarations are not strictly sorted".to_string());
        }
        for (order, names, kind) in [
            (
                &self.structs,
                (|entry: &DeclarationEntry| &entry.structs)
                    as fn(&DeclarationEntry) -> &Vec<String>,
                "struct",
            ),
            (&self.functions, |entry| &entry.functions, "function"),
        ] {
            let name = |(entry, index): &(u32, u32)| {
                self.declarations
                    .get(*entry as usize)
                    .and_then(|entry| names(entry).get(*index as usize))
            };
            let total: usize = self
                .declarations
                .iter()
                .map(|entry| names(entry).len())
                .sum();
            let sorted = order.len() == total
                && order.iter().all(|pair| name(pair).is_some())
                && order.windows(2).all(|pair| name(&pair[0]) < name(&pair[1]));
            if !sorted {
                return Err(format!("the {kind} declarations are not a strict sort"));
            }
        }
        Ok(())
    }
}

/// One library module: its identity, and where its link record lies.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ModuleEntry {
    /// Its dotted name (`std.io.file`).
    pub name: String,
    /// The canonical path of its source file, its identity in a link.
    pub canonical: String,
    /// The path its declarations' locations name.
    pub path: String,
    /// Its link record.
    pub link: Range<usize>,
}

/// One library module as a package build writes it: its identity and its
/// encoded link record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagedModule {
    pub name: String,
    pub canonical: String,
    pub path: String,
    pub link: Vec<u8>,
}

/// One library declaration: what its record answers, and where it lies.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct DeclarationEntry {
    pub name: String,
    /// Its module's position in the directory.
    pub module: u32,
    /// Every checker table key its residue holds, sorted.
    pub keys: Vec<(InterfaceTable, String)>,
    /// Every method its struct declares, sorted.
    pub methods: Vec<String>,
    /// The name of each struct declaration its record holds, in record
    /// order.
    pub structs: Vec<String>,
    /// The lowered name of each function declaration its record holds, in
    /// record order.
    pub functions: Vec<String>,
    pub record: Range<usize>,
    /// Where the residue starts, from the record's start.
    pub residue: usize,
}

/// A package's declarations: each record's lowered part decoded the first
/// time one of its names is looked up, once per process.
///
/// A package is shared by every compilation in a process, so nothing an
/// importer adds to a declaration lives here.
#[derive(Debug)]
pub struct PackageDeclarations {
    payload: Payload,
    directory: DeclarationDirectory,
    /// Every trait the package declares, sorted.
    traits: Vec<String>,
    decoded: Vec<OnceLock<Option<LoweredDeclaration>>>,
    failures: Mutex<Vec<String>>,
}

impl PackageDeclarations {
    /// The declarations `directory` places in `payload`.
    ///
    /// # Errors
    ///
    /// A record outside the payload, a name list that is not strictly
    /// sorted, or a position naming no declaration.
    pub fn read(payload: Payload, directory: DeclarationDirectory) -> Result<Self, String> {
        directory.validate(payload.len())?;
        let mut traits: Vec<String> = directory
            .declarations
            .iter()
            .flat_map(|entry| &entry.keys)
            .filter(|(table, _)| *table == InterfaceTable::Traits)
            .map(|(_, key)| key.clone())
            .collect();
        traits.sort();
        let decoded = directory
            .declarations
            .iter()
            .map(|_| OnceLock::new())
            .collect();
        Ok(Self {
            payload,
            directory,
            traits,
            decoded,
            failures: Mutex::new(Vec::new()),
        })
    }

    /// `records` encoded on their own, as a package holding nothing else
    /// would hold them.
    ///
    /// # Errors
    ///
    /// A record that does not encode.
    pub fn of_records(records: DeclarationRecords) -> Result<Self, String> {
        let mut payload = Vec::new();
        let directory = records.encode(&mut |bytes| {
            let start = payload.len();
            payload.extend_from_slice(bytes);
            start..payload.len()
        })?;
        Self::read(Payload::new(PackageBytes::Owned(payload), 0), directory)
    }

    /// The struct declaration `name`, decoded on first use; `None` when the
    /// package declares no such struct or its record does not decode (a
    /// failure [`Self::failures`] reports).
    pub fn structure(&self, name: &str) -> Option<&StructDeclaration> {
        self.structure_at(self.struct_position(name)?)
    }

    /// The function declaration `name`, decoded on first use.
    pub fn function(&self, name: &str) -> Option<&FunctionDeclaration> {
        self.function_at(self.function_position(name)?)
    }

    /// Whether the package declares a function `name`, decoding nothing.
    pub fn contains_function(&self, name: &str) -> bool {
        self.function_position(name).is_some()
    }

    /// The position of the struct `name` in package order, decoding
    /// nothing.
    pub fn struct_position(&self, name: &str) -> Option<usize> {
        let directory = &self.directory;
        position(&directory.structs, name, |entry| &entry.structs, directory)
    }

    /// The struct declaration at `position` in package order.
    pub fn structure_at(&self, position: usize) -> Option<&StructDeclaration> {
        let (entry, index) = *self.directory.structs.get(position)?;
        self.lowered(entry as usize)?.structs.get(index as usize)
    }

    /// The function declaration at `position` in package order.
    pub fn function_at(&self, position: usize) -> Option<&FunctionDeclaration> {
        let (entry, index) = *self.directory.functions.get(position)?;
        self.lowered(entry as usize)?.functions.get(index as usize)
    }

    /// Every struct name, in package order, decoding nothing.
    pub fn struct_names(&self) -> impl Iterator<Item = &str> {
        self.directory.structs.iter().map(|(entry, index)| {
            self.directory.declarations[*entry as usize].structs[*index as usize].as_str()
        })
    }

    /// Every function name, in package order, decoding nothing.
    pub fn function_names(&self) -> impl Iterator<Item = &str> {
        self.directory.functions.iter().map(|(entry, index)| {
            self.directory.declarations[*entry as usize].functions[*index as usize].as_str()
        })
    }

    /// Every trait the package declares, sorted.
    pub fn traits(&self) -> &[String] {
        &self.traits
    }

    /// The trait declaration `name`, decoded on first use; `None` when the
    /// package declares no such trait.
    pub fn trait_declaration(&self, name: &str) -> Option<&TraitDeclaration> {
        self.traits
            .binary_search_by(|known| known.as_str().cmp(name))
            .ok()?;
        self.lowered_owning(name)?
            .traits
            .iter()
            .find(|declaration| declaration.name == name)
    }

    /// The directory, which names every module and declaration record.
    pub const fn directory(&self) -> &DeclarationDirectory {
        &self.directory
    }

    /// The package's module table, sorted by dotted name.
    pub fn modules(&self) -> &[ModuleEntry] {
        &self.directory.modules
    }

    /// The encoded link record of the module at `position` in the module
    /// table.
    pub fn link_record(&self, position: usize) -> Option<&[u8]> {
        let module = self.directory.modules.get(position)?;
        Some(self.payload.get(&module.link))
    }

    /// Whether a declaration stamped with `module` comes from one of the
    /// package's modules, so its body is in the package.
    pub fn compiles(&self, module: Option<&str>) -> bool {
        module.is_some_and(|module| {
            self.directory
                .modules
                .iter()
                .any(|entry| entry.path == module)
        })
    }

    /// The lowered part of the declaration at `entry`, decoded on first
    /// use.
    pub fn lowered(&self, entry: usize) -> Option<&LoweredDeclaration> {
        self.decoded
            .get(entry)?
            .get_or_init(|| {
                let declaration = &self.directory.declarations[entry];
                let record = self.payload.get(&declaration.record);
                postcard::from_bytes(&record[..declaration.residue])
                    .map_err(|error| {
                        self.fail(format!(
                            "package declaration `{}` does not decode: {error}",
                            declaration.name
                        ));
                    })
                    .ok()
            })
            .as_ref()
    }

    /// The lowered part of the declaration the name `name` belongs to
    /// ([`DeclaringModules::declaration_of`]), decoded on first use.
    pub fn lowered_owning(&self, name: &str) -> Option<&LoweredDeclaration> {
        self.lowered(self.owning(name)?)
    }

    /// The checker residue of the declaration at `entry`, undecoded.
    pub fn residue(&self, entry: usize) -> &[u8] {
        let declaration = &self.directory.declarations[entry];
        &self.payload.get(&declaration.record)[declaration.residue..]
    }

    /// How many declaration records' lowered parts have been decoded so
    /// far.
    pub fn decoded(&self) -> usize {
        self.decoded
            .iter()
            .filter(|cell| cell.get().is_some())
            .count()
    }

    /// The declarations that failed to decode so far.
    pub fn failures(&self) -> Vec<String> {
        self.failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The position of the declaration the name `name` belongs to,
    /// decoding nothing.
    fn owning(&self, name: &str) -> Option<usize> {
        let declarations = &self.directory.declarations;
        declaration_candidates(name).find_map(|candidate| {
            declarations
                .binary_search_by(|entry| entry.name.as_str().cmp(candidate))
                .ok()
        })
    }

    fn function_position(&self, name: &str) -> Option<usize> {
        let directory = &self.directory;
        position(
            &directory.functions,
            name,
            |entry| &entry.functions,
            directory,
        )
    }

    fn fail(&self, message: String) {
        self.failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(message);
    }
}

/// The part of a package an importer's link reached, which its lookups
/// answer from.
#[derive(Debug, Clone)]
pub struct PackageScope {
    package: Arc<PackageDeclarations>,
    /// Per module, whether the link reached it.
    reached: Arc<[bool]>,
}

impl PackageScope {
    /// The declarations of `package` in the modules `reached` marks.
    pub const fn new(package: Arc<PackageDeclarations>, reached: Arc<[bool]>) -> Self {
        Self { package, reached }
    }

    /// The overload facts of the declaration the name `name` belongs to,
    /// decoded on first use; `None` when no reached module declares it.
    pub fn overloads_of(&self, name: &str) -> Option<&OverloadSets> {
        let entry = self.package.owning(name)?;
        let module = self.package.directory.declarations[entry].module;
        if !self.reached.get(module as usize).copied().unwrap_or(false) {
            return None;
        }
        self.package
            .lowered(entry)
            .map(|lowered| &lowered.overloads)
    }
}

/// A program's overload sets: its own, and the package it imports, whose
/// declarations answer a name the own sets miss.
///
/// An importer declares none of its package's names, so the two never
/// answer the same name.
#[derive(Debug, Clone, Default)]
pub struct ProgramOverloads {
    own: OverloadSets,
    package: Option<PackageScope>,
}

impl ProgramOverloads {
    /// The sets of a program importing `package`, before its own
    /// statements are scanned.
    pub fn importing(package: Option<PackageScope>) -> Self {
        Self {
            own: OverloadSets::default(),
            package,
        }
    }

    /// The program's own sets.
    pub const fn own(&self) -> &OverloadSets {
        &self.own
    }

    /// Add the declarations of `program`, the program's own, whose
    /// constants may read the package's.
    pub fn scan_more(&mut self, program: &[Stmt]) {
        let package = self.package.as_ref();
        self.own
            .scan_more(program, |name| package?.overloads_of(name)?.comptime(name));
    }

    /// Record the own method `source_name`'s declarations by arity
    /// ([`OverloadSets::declare_method`]).
    pub fn declare_method(&mut self, source_name: &str, arities: Vec<usize>) {
        self.own.declare_method(source_name, arities);
    }

    /// The overload facts of the package declaration owning `name`.
    fn package(&self, name: &str) -> Option<&OverloadSets> {
        self.package.as_ref()?.overloads_of(name)
    }
}

impl OverloadLookup for ProgramOverloads {
    fn function_is_overloaded(&self, name: &str, arity: usize) -> bool {
        self.own.function_is_overloaded(name, arity)
            || self
                .package(name)
                .is_some_and(|sets| sets.function_is_overloaded(name, arity))
    }

    fn is_function(&self, name: &str) -> bool {
        self.own.is_function(name)
            || self
                .package(name)
                .is_some_and(|sets| sets.is_function(name))
    }

    fn method_is_overloaded(&self, source_name: &str, arity: usize) -> bool {
        self.own.method_is_overloaded(source_name, arity)
            || self
                .package(source_name)
                .is_some_and(|sets| sets.method_is_overloaded(source_name, arity))
    }

    fn fieldwise_parameters(&self, name: &str) -> Option<&[String]> {
        self.own
            .fieldwise_parameters(name)
            .or_else(|| self.package(name)?.fieldwise_parameters(name))
    }

    fn comptime(&self, name: &str) -> Option<i64> {
        self.own
            .comptime(name)
            .or_else(|| self.package(name)?.comptime(name))
    }
}

/// The declarations a package build writes.
///
/// The module table, and each library declaration's record: its lowered
/// part, which the build places name by name ([`Self::place_struct`] and
/// its siblings), and its checker residue.
#[derive(Debug, Default)]
pub struct DeclarationRecords {
    /// Each library module, as the build's link recorded it.
    pub modules: Vec<PackagedModule>,
    /// Each declaration's record, by name.
    pub declarations: BTreeMap<String, DeclarationRecord>,
    /// The module declaring each library declaration.
    pub declaring: DeclaringModules,
}

/// One library declaration's record as a package build writes it.
#[derive(Debug, Default)]
pub struct DeclarationRecord {
    /// Its module's path.
    pub module: String,
    pub keys: Vec<(InterfaceTable, String)>,
    pub methods: Vec<String>,
    pub lowered: LoweredDeclaration,
    /// Its encoded checker rows.
    pub residue: Vec<u8>,
}

impl DeclarationRecords {
    /// Put `declaration` in the record of the declaration it belongs to.
    ///
    /// # Errors
    ///
    /// No library declaration owns its name.
    pub fn place_struct(&mut self, declaration: StructDeclaration) -> Result<(), String> {
        let structs = &mut self.record_of(&declaration.name)?.lowered.structs;
        let position = structs
            .binary_search_by(|placed| placed.name.as_bytes().cmp(declaration.name.as_bytes()))
            .err()
            .ok_or_else(|| format!("struct `{}` is placed twice", declaration.name))?;
        structs.insert(position, declaration);
        Ok(())
    }

    /// Put `declaration` in the record of the declaration it belongs to.
    ///
    /// # Errors
    ///
    /// No library declaration owns its lowered name.
    pub fn place_function(&mut self, declaration: FunctionDeclaration) -> Result<(), String> {
        let functions = &mut self.record_of(&declaration.lowered_name)?.lowered.functions;
        let position = functions
            .binary_search_by(|placed| {
                placed
                    .lowered_name
                    .as_bytes()
                    .cmp(declaration.lowered_name.as_bytes())
            })
            .err()
            .ok_or_else(|| format!("function `{}` is placed twice", declaration.lowered_name))?;
        functions.insert(position, declaration);
        Ok(())
    }

    /// Put `declaration` in the record of the trait it declares.
    ///
    /// # Errors
    ///
    /// No library declaration owns its name.
    pub fn place_trait(&mut self, declaration: TraitDeclaration) -> Result<(), String> {
        let traits = &mut self.record_of(&declaration.name)?.lowered.traits;
        let position = traits
            .binary_search_by(|placed| placed.name.as_bytes().cmp(declaration.name.as_bytes()))
            .err()
            .ok_or_else(|| format!("trait `{}` is placed twice", declaration.name))?;
        traits.insert(position, declaration);
        Ok(())
    }

    /// Put the transfer effects of the callable `name` in the record of the
    /// declaration it belongs to.
    ///
    /// # Errors
    ///
    /// No library declaration owns `name`.
    pub fn place_transfer_effects(
        &mut self,
        name: String,
        effects: Vec<TransferEffect>,
    ) -> Result<(), String> {
        let record = self.record_of(&name)?;
        sorted_insert(&mut record.lowered.transfer_effects, name, effects);
        Ok(())
    }

    /// Put the call-through effects of the callable `name` in the record of
    /// the declaration it belongs to.
    ///
    /// # Errors
    ///
    /// No library declaration owns `name`.
    pub fn place_call_through_effects(
        &mut self,
        name: String,
        effects: Vec<CallThroughEffect>,
    ) -> Result<(), String> {
        let record = self.record_of(&name)?;
        sorted_insert(&mut record.lowered.call_through_effects, name, effects);
        Ok(())
    }

    /// Put the overload facts of the names the declaration `declaration`
    /// owns in its record.
    ///
    /// # Errors
    ///
    /// No library declaration is named `declaration`.
    pub fn place_overloads(
        &mut self,
        declaration: &str,
        overloads: OverloadSets,
    ) -> Result<(), String> {
        self.record_of(declaration)?.lowered.overloads = overloads;
        Ok(())
    }

    /// Encode every record with `append`, which stores its bytes and
    /// answers their range, and answer the directory placing them.
    ///
    /// # Errors
    ///
    /// A record that does not encode, a module recorded twice, or a
    /// declaration of no recorded module.
    pub fn encode(
        mut self,
        append: &mut impl FnMut(&[u8]) -> Range<usize>,
    ) -> Result<DeclarationDirectory, String> {
        self.modules
            .sort_by(|left, right| left.name.cmp(&right.name));
        if let Some(pair) = self
            .modules
            .windows(2)
            .find(|pair| pair[0].name == pair[1].name)
        {
            return Err(format!("module `{}` is recorded twice", pair[0].name));
        }
        let positions: HashMap<&str, u32> = self
            .modules
            .iter()
            .enumerate()
            .map(|(position, module)| (module.path.as_str(), position as u32))
            .collect();
        let mut directory = DeclarationDirectory::default();
        let mut structs = Vec::new();
        let mut functions = Vec::new();
        for (position, (name, record)) in self.declarations.into_iter().enumerate() {
            let module = positions
                .get(record.module.as_str())
                .copied()
                .ok_or_else(|| format!("`{name}` has no module record"))?;
            let mut bytes = postcard::to_stdvec(&record.lowered)
                .map_err(|error| format!("the declarations of `{name}` do not encode: {error}"))?;
            let residue = bytes.len();
            bytes.extend_from_slice(&record.residue);
            let entry = DeclarationEntry {
                structs: record
                    .lowered
                    .structs
                    .iter()
                    .map(|declaration| declaration.name.clone())
                    .collect(),
                functions: record
                    .lowered
                    .functions
                    .iter()
                    .map(|declaration| declaration.lowered_name.clone())
                    .collect(),
                name,
                module,
                keys: record.keys,
                methods: record.methods,
                record: append(&bytes),
                residue,
            };
            structs.extend((0..entry.structs.len()).map(|index| (position as u32, index as u32)));
            functions
                .extend((0..entry.functions.len()).map(|index| (position as u32, index as u32)));
            directory.declarations.push(entry);
        }
        let declarations = &directory.declarations;
        structs.sort_by(|left, right| {
            let name = |(entry, index): &(u32, u32)| {
                declarations[*entry as usize].structs[*index as usize].as_bytes()
            };
            name(left).cmp(name(right))
        });
        functions.sort_by(|left, right| {
            let name = |(entry, index): &(u32, u32)| {
                declarations[*entry as usize].functions[*index as usize].as_bytes()
            };
            name(left).cmp(name(right))
        });
        directory.structs = structs;
        directory.functions = functions;
        directory.modules = self
            .modules
            .into_iter()
            .map(|module| ModuleEntry {
                link: append(&module.link),
                name: module.name,
                canonical: module.canonical,
                path: module.path,
            })
            .collect();
        Ok(directory)
    }

    /// The record of the declaration `name` belongs to, opened empty when
    /// the check left the declaration no row.
    fn record_of(&mut self, name: &str) -> Result<&mut DeclarationRecord, String> {
        let declaration = self
            .declaring
            .declaration_of(name)
            .ok_or_else(|| format!("no library declaration owns `{name}`"))?
            .to_string();
        let module = self.declaring.names[&declaration].clone();
        Ok(self
            .declarations
            .entry(declaration)
            .or_insert_with(|| DeclarationRecord {
                module,
                ..DeclarationRecord::default()
            }))
    }
}

/// The module each module-level name of a library is declared in.
#[derive(Debug, Default)]
pub struct DeclaringModules {
    names: HashMap<String, String>,
}

impl DeclaringModules {
    /// The declaring module of every name `program` declares at module
    /// level. A name two modules declare (overloads split across modules)
    /// has no one module to install it from, and is refused.
    ///
    /// # Errors
    ///
    /// A name two modules declare.
    pub fn of(program: &[Stmt]) -> Result<Self, String> {
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

    /// Record `name` as a declaration of `module`, one the program no
    /// longer spells (a module constant its elaboration folded away).
    ///
    /// # Errors
    ///
    /// Another module declares `name`.
    pub fn declare(&mut self, name: &str, module: &str) -> Result<(), String> {
        match self.names.get(name) {
            Some(declared) if declared != module => Err(format!(
                "`{name}` is declared in both `{declared}` and `{module}`"
            )),
            Some(_) => Ok(()),
            None => {
                self.names.insert(name.to_string(), module.to_string());
                Ok(())
            }
        }
    }

    /// Every declaring module.
    pub fn modules(&self) -> impl Iterator<Item = &String> {
        self.names.values()
    }

    /// The module declaring the declaration `declaration`.
    pub fn module(&self, declaration: &str) -> Option<&str> {
        self.names.get(declaration).map(String::as_str)
    }

    /// The module declaring the name `name` belongs to
    /// ([`Self::declaration_of`]).
    pub fn module_of(&self, name: &str) -> Option<&str> {
        self.declaration_of(name)
            .and_then(|declaration| self.module(declaration))
    }

    /// The declaration the name `name` belongs to: a declaration's own
    /// name, an overload's signature-qualified one, a member's
    /// `Owner.member`, or a function lifted from one of those
    /// ([`lifted_owner`]).
    pub fn declaration_of(&self, name: &str) -> Option<&str> {
        declaration_candidates(name)
            .find_map(|declaration| self.names.get_key_value(declaration))
            .map(|(declaration, _)| declaration.as_str())
    }
}

/// The names the declaration owning `name` may have, most specific first:
/// the name itself, an overload's base, or a member's owner, of `name` or,
/// for a lifted function, of the name it is lifted from ([`lifted_owner`]);
/// none for an anonymous lifted function.
fn declaration_candidates(name: &str) -> impl Iterator<Item = &str> {
    let owner = match lifted_owner(name) {
        Some(LiftedOwner::Constant(owner) | LiftedOwner::Numbered(owner)) => Some(owner),
        Some(LiftedOwner::Default { owner, .. }) => Some(owner),
        Some(LiftedOwner::Anonymous) => None,
        None => Some(name),
    };
    owner.into_iter().flat_map(|owner| {
        [
            Some(owner),
            owner.split_once(OV_SEP).map(|(declaration, _)| declaration),
            owner.split_once('.').map(|(declaration, _)| declaration),
        ]
        .into_iter()
        .flatten()
    })
}

/// The value of `name` in `entries`, which are sorted by name.
fn sorted_get<'a, V>(entries: &'a [(String, V)], name: &str) -> Option<&'a V> {
    entries
        .binary_search_by(|(entry, _)| entry.as_str().cmp(name))
        .ok()
        .map(|position| &entries[position].1)
}

/// Insert `value` under `name` in `entries`, kept sorted by name.
fn sorted_insert<V>(entries: &mut Vec<(String, V)>, name: String, value: V) {
    match entries.binary_search_by(|(entry, _)| entry.as_str().cmp(&name)) {
        Ok(position) => entries[position].1 = value,
        Err(position) => entries.insert(position, (name, value)),
    }
}

/// The position in `order` of the declaration `name`, whose names an entry
/// lists at `names`.
fn position(
    order: &[(u32, u32)],
    name: &str,
    names: impl Fn(&DeclarationEntry) -> &Vec<String>,
    directory: &DeclarationDirectory,
) -> Option<usize> {
    order
        .binary_search_by(|(entry, index)| {
            names(&directory.declarations[*entry as usize])[*index as usize]
                .as_bytes()
                .cmp(name.as_bytes())
        })
        .ok()
}
