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
//! The lowered part also holds the integer value of the declaration's
//! constant. It holds no overload set: as upstream does, an importer finds
//! a name's overload set by looking up the declarations under the name,
//! after its program's own sets miss ([`ProgramOverloads`]), and the build
//! refuses a library whose lowered declarations say other than its check's
//! sets.
//!
//! The directory also holds the package's one module table: each library
//! module, by dotted name, beside its source file's identity and the record
//! its importer's link reads (its loads, exports, and namespaces), which
//! every declaration names by position. A module's compile-time facts and
//! constants are each its declaration's. It holds one row per library
//! function, which names the function once and places its body, and the
//! indexes every phase looks a name up in; an importer reads it in place
//! ([`DeclarationDirectory`]).

use crate::checked::{CallThroughEffect, TransferEffect};
use crate::declaration::{FunctionDeclaration, StructDeclaration, TraitDeclaration};
use mojito_ast::ast::{Stmt, StmtKind};
use mojito_symbol::symbol::{
    LiftedOwner, OV_SEP, OverloadFamily, OverloadLookup, OverloadSets, lifted_owner,
};
use std::borrow::Cow;
use std::cmp::Ordering;
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

    /// The record at `range`; empty when `range` leaves the payload.
    pub fn get(&self, range: &Range<usize>) -> &[u8] {
        self.bytes
            .get(self.start + range.start..self.start + range.end)
            .unwrap_or_default()
    }

    /// The little-endian word at `at`; `u32::MAX` past the payload's end.
    fn word(&self, at: usize) -> u32 {
        word_at(&self.bytes[self.start..], at)
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
    const ALL: [Self; Self::COUNT] = [
        Self::Structs,
        Self::Traits,
        Self::Aliases,
        Self::Comptimes,
        Self::ComptimeLiterals,
        Self::ModuleValues,
        Self::ComptimeApplied,
        Self::ApplicableFunctions,
        Self::DefResults,
        Self::OwnedCollectors,
        Self::CollectorNames,
        Self::DeclaredStructs,
        Self::AppliedModuleConstants,
        Self::NestedOriginsReadOnlyFunctions,
        Self::ExplicitDestroy,
        Self::Bindings,
        Self::Elaborated,
        Self::ComptimeDtypes,
        Self::ComptimeParameters,
        Self::ComptimeValues,
        Self::TypeAliases,
        Self::SimdLengthBinders,
    ];

    /// The table numbered `index`.
    fn from_index(index: u32) -> Option<Self> {
        Self::ALL.get(index as usize).copied()
    }
}

/// The lowered part of a declaration's record.
///
/// Every struct and function declaration it lowers to — its own, its
/// methods, and the functions nested in or lifted from it — and the effect
/// summaries of its callables. [`DeclarationRecords`] keeps every list
/// sorted.
///
/// Encoded, it names nothing the directory holds, as an MLIR bytecode op
/// names its symbol by an index into the one string section: each function
/// by its row in the function table, its struct and trait by the
/// declaration's own row, and each reference mask is its conventions'
/// ([`FunctionDeclaration::conventions_mask`]). Decoding joins them back.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct LoweredDeclaration {
    /// Its own struct, when it declares one.
    pub structs: Vec<StructDeclaration>,
    /// Sorted by lowered name.
    pub functions: Vec<FunctionDeclaration>,
    /// Each function's row in the directory's function table, aligned with
    /// `functions`; filled when the record is encoded.
    pub function_rows: Vec<u32>,
    /// Its own trait, with the traits it refines, when it declares one.
    pub traits: Vec<TraitDeclaration>,
    /// The transfer effects the package's bodies record for each callable
    /// it declares, by callable identity, sorted.
    pub transfer_effects: Vec<(String, Vec<TransferEffect>)>,
    /// The call-through effects of each callable it declares, likewise.
    pub call_through_effects: Vec<(String, Vec<CallThroughEffect>)>,
    /// The integer value of its module constant, by name, sorted.
    pub constants: Vec<(String, i64)>,
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

    /// The integer value of the module constant `name`.
    pub fn constant(&self, name: &str) -> Option<i64> {
        sorted_get(&self.constants, name).copied()
    }

    /// Refuse a record of the declaration `name` whose names and masks
    /// would not join back from the directory as encoded: a struct or trait
    /// not the declaration's own, or a reference mask other than its
    /// function's conventions.
    fn refuse_unjoinable(&self, name: &str) -> Result<(), String> {
        if let Some(function) = self
            .functions
            .iter()
            .find(|function| function.ref_params != function.conventions_mask())
        {
            return Err(format!(
                "the reference mask of `{}` says other than its conventions",
                function.lowered_name
            ));
        }
        let own = |names: Vec<&str>| names.len() <= 1 && names.iter().all(|own| *own == name);
        if !own(self
            .structs
            .iter()
            .map(|declaration| declaration.name.as_str())
            .collect())
        {
            return Err(format!("the record of `{name}` holds a struct not its own"));
        }
        if !own(self
            .traits
            .iter()
            .map(|declaration| declaration.name.as_str())
            .collect())
        {
            return Err(format!("the record of `{name}` holds a trait not its own"));
        }
        Ok(())
    }

    /// Restore what the encoded record of the declaration at `entry` (named
    /// `name`) leaves to `directory`: each function's name from its row,
    /// which must place it back at its index in this record, its reference
    /// mask from its conventions, and its struct's and trait's names.
    fn join(
        &mut self,
        directory: &DeclarationDirectory,
        entry: usize,
        name: &str,
    ) -> Result<(), String> {
        if self.function_rows.len() != self.functions.len() {
            return Err("its function rows are not one per function".to_string());
        }
        for (index, (function, row)) in self
            .functions
            .iter_mut()
            .zip(&self.function_rows)
            .enumerate()
        {
            let word = |column| directory.word(Section::Functions, *row as usize, column);
            if (word(1) as usize, word(2) as usize) != (entry, index) {
                return Err(format!("function row {row} does not place its function"));
            }
            function.lowered_name = directory.string(word(0)).to_string();
            function.ref_params = function.conventions_mask();
        }
        for declaration in &mut self.structs {
            declaration.name = name.to_string();
        }
        for declaration in &mut self.traits {
            declaration.name = name.to_string();
        }
        Ok(())
    }
}

/// A package's declaration directory, read in place from its payload.
///
/// It holds the module table, where each declaration's record lies, the
/// struct and function tables, and the indexes the check and the elaborator
/// look a name up in.
///
/// Every name it holds is written once, in one string section — a LEB128
/// count, each string's LEB128 length in reverse order, then the strings'
/// bytes — as an MLIR bytecode string section is. Its tables follow: each
/// table's row count, then each table's rows, every row a fixed number of
/// little-endian `u32` words, a name being its string's index.
/// [`DeclarationRecords::encode`] writes every table sorted as its lookups
/// need. Reading the directory builds only its strings' offsets, and a
/// lookup reads only the rows its binary search touches; a malformed row
/// makes a lookup miss, never read outside the payload.
#[derive(Debug)]
pub struct DeclarationDirectory {
    payload: Payload,
    /// Each string's start in the payload, then the last one's end.
    strings: Vec<usize>,
    /// Each table's first row's offset in the payload and its row count.
    tables: [(usize, usize); Section::COUNT],
}

impl DeclarationDirectory {
    /// How many modules the module table holds.
    pub const fn module_count(&self) -> usize {
        self.rows(Section::Modules)
    }

    /// The module at `position` in the module table, which is sorted by
    /// dotted name.
    pub fn module(&self, position: usize) -> Option<ModuleEntry<'_>> {
        (position < self.module_count()).then(|| self.module_row(position))
    }

    /// The module table, sorted by dotted name.
    pub fn modules(&self) -> impl ExactSizeIterator<Item = ModuleEntry<'_>> + '_ {
        (0..self.module_count()).map(|position| self.module_row(position))
    }

    /// How many declaration records the directory places.
    pub const fn declaration_count(&self) -> usize {
        self.rows(Section::Declarations)
    }

    /// The declaration at `position`, the declarations being sorted by
    /// name.
    pub fn declaration(&self, position: usize) -> Option<DeclarationEntry<'_>> {
        (position < self.declaration_count()).then(|| {
            let word = |column| self.word(Section::Declarations, position, column);
            DeclarationEntry {
                name: self.string(word(0)),
                module: word(1),
                record: word(2) as usize..word(3) as usize,
                residue: word(4) as usize,
            }
        })
    }

    /// Every checker table key the residue of the declaration at
    /// `position` holds, sorted.
    pub fn declaration_keys(
        &self,
        position: usize,
    ) -> impl Iterator<Item = (InterfaceTable, &str)> + '_ {
        let word = |column| self.word(Section::Declarations, position, column);
        self.span(Section::DeclarationKeys, word(5), word(6))
            .filter_map(|row| {
                let table =
                    InterfaceTable::from_index(self.word(Section::DeclarationKeys, row, 0))?;
                Some((
                    table,
                    self.string(self.word(Section::DeclarationKeys, row, 1)),
                ))
            })
    }

    /// The position of the declaration whose residue holds `table`'s
    /// `key`.
    pub fn key_declaration(&self, table: InterfaceTable, key: &str) -> Option<u32> {
        let row = self.find(Section::Keys, |row| {
            self.word(Section::Keys, row, 0)
                .cmp(&(table as u32))
                .then_with(|| {
                    self.text(self.word(Section::Keys, row, 1))
                        .cmp(key.as_bytes())
                })
        })?;
        Some(self.word(Section::Keys, row, 2))
    }

    /// Every key of `table`, sorted, beside the position of the declaration
    /// holding it.
    pub fn table_keys(&self, table: InterfaceTable) -> impl Iterator<Item = (&str, u32)> + '_ {
        let rows = 0..self.rows(Section::Keys);
        let start = partition_point(rows.clone(), |row| {
            self.word(Section::Keys, row, 0) < table as u32
        });
        let end = partition_point(rows, |row| self.word(Section::Keys, row, 0) <= table as u32);
        (start..end).map(|row| {
            (
                self.string(self.word(Section::Keys, row, 1)),
                self.word(Section::Keys, row, 2),
            )
        })
    }

    /// The position of each declaration whose struct declares `method`.
    pub fn methods_declaring(&self, method: &str) -> impl Iterator<Item = u32> + '_ {
        let rows = 0..self.rows(Section::Methods);
        let name = |row| self.text(self.word(Section::Methods, row, 0));
        let start = partition_point(rows.clone(), |row| name(row) < method.as_bytes());
        let end = partition_point(rows, |row| name(row) <= method.as_bytes());
        (start..end).map(|row| self.word(Section::Methods, row, 1))
    }

    /// How many module bindings the module at `module` makes.
    pub fn binding_count(&self, module: usize) -> usize {
        self.module_bindings(module).len()
    }

    /// The module binding at `position` among the module `module`'s, which
    /// are sorted by name.
    pub fn binding(&self, module: usize, position: usize) -> Option<&str> {
        let row = self.module_bindings(module).nth(position)?;
        Some(self.string(self.word(Section::Bindings, row, 0)))
    }

    /// The position of the binding `name` among the module `module`'s.
    pub fn binding_position(&self, module: usize, name: &str) -> Option<usize> {
        let rows = self.module_bindings(module);
        let start = rows.start;
        let text = |row| self.text(self.word(Section::Bindings, row, 0));
        let found = partition_point(rows.clone(), |row| text(row) < name.as_bytes());
        (rows.contains(&found) && text(found) == name.as_bytes()).then(|| found - start)
    }

    /// The directory `range` of `payload` holds, of which only the string
    /// section's lengths are read.
    fn read(payload: Payload, range: &Range<usize>) -> Result<Self, String> {
        let bytes = payload.get(range);
        if bytes.len() != range.len() {
            return Err("the declaration directory overruns the payload".to_string());
        }
        let mut cursor = 0;
        let count = leb128(bytes, &mut cursor)? as usize;
        let mut lengths = Vec::with_capacity(count.min(bytes.len()));
        for _ in 0..count {
            lengths.push(leb128(bytes, &mut cursor)? as usize);
        }
        let mut strings = Vec::with_capacity(lengths.len() + 1);
        let mut offset = cursor;
        strings.push(range.start + offset);
        for length in lengths.into_iter().rev() {
            offset = offset.saturating_add(length);
            strings.push(range.start + offset);
        }
        let counts = offset
            .checked_add(Section::COUNT * 4)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| "the declaration directory's strings overrun it".to_string())?;
        let mut tables = [(0, 0); Section::COUNT];
        let mut start = counts;
        for (index, section) in Section::ALL.into_iter().enumerate() {
            let rows = word_at(bytes, offset + index * 4) as usize;
            tables[index] = (range.start + start, rows);
            start = rows
                .checked_mul(section.width() * 4)
                .and_then(|length| start.checked_add(length))
                .ok_or_else(|| "a declaration directory table overruns it".to_string())?;
        }
        if start != bytes.len() {
            return Err("the declaration directory's tables do not fill it".to_string());
        }
        Ok(Self {
            payload,
            strings,
            tables,
        })
    }

    fn word(&self, section: Section, row: usize, column: usize) -> u32 {
        let (start, rows) = self.tables[section as usize];
        if row >= rows {
            return u32::MAX;
        }
        self.payload
            .word(start + (row * section.width() + column) * 4)
    }

    /// The string at `index`, empty when the section holds no such string
    /// or it is not UTF-8.
    fn string(&self, index: u32) -> &str {
        std::str::from_utf8(self.text(index)).unwrap_or_default()
    }

    /// The bytes of the string at `index`.
    fn text(&self, index: u32) -> &[u8] {
        let index = index as usize;
        match (self.strings.get(index), self.strings.get(index + 1)) {
            (Some(start), Some(end)) => self.payload.get(&(*start..*end)),
            _ => &[],
        }
    }

    const fn rows(&self, section: Section) -> usize {
        self.tables[section as usize].1
    }

    /// The row `compare` answers `Equal` for, `section` being sorted by
    /// it.
    fn find(&self, section: Section, compare: impl Fn(usize) -> Ordering) -> Option<usize> {
        let rows = 0..self.rows(section);
        let found = partition_point(rows.clone(), |row| compare(row) == Ordering::Less);
        (rows.contains(&found) && compare(found) == Ordering::Equal).then_some(found)
    }

    /// The `count` rows of `section` from `start`, clamped to the table.
    fn span(&self, section: Section, start: u32, count: u32) -> Range<usize> {
        let rows = self.rows(section);
        let start = (start as usize).min(rows);
        start..start.saturating_add(count as usize).min(rows)
    }

    fn module_row(&self, position: usize) -> ModuleEntry<'_> {
        let word = |column| self.word(Section::Modules, position, column);
        ModuleEntry {
            name: self.string(word(0)),
            canonical: self.string(word(1)),
            path: self.string(word(2)),
            link: word(3) as usize..word(4) as usize,
        }
    }

    /// The rows of the module `module`'s bindings.
    fn module_bindings(&self, module: usize) -> Range<usize> {
        if module >= self.module_count() {
            return 0..0;
        }
        let word = |column| self.word(Section::Modules, module, column);
        self.span(Section::Bindings, word(5), word(6))
    }

    /// The row in lowering order of the function `name`.
    fn function_row(&self, name: &str) -> Option<usize> {
        let position = self.function_position(name)?;
        Some(self.word(Section::FunctionsByName, position, 0) as usize)
    }

    /// The position of the function `name` in name order.
    fn function_position(&self, name: &str) -> Option<usize> {
        self.find(Section::FunctionsByName, |position| {
            self.function_name(position).cmp(name.as_bytes())
        })
    }

    /// The positions in name order of every function whose name starts
    /// with `prefix`: one run, since names sort by their bytes.
    fn function_positions(&self, prefix: &str) -> Range<usize> {
        let prefix = prefix.as_bytes();
        let rows = 0..self.rows(Section::FunctionsByName);
        let start = partition_point(rows.clone(), |position| {
            self.function_name(position) < prefix
        });
        let end = partition_point(start..rows.end, |position| {
            self.function_name(position).starts_with(prefix)
        });
        start..end
    }

    /// The name bytes of the function at `position` in name order.
    fn function_name(&self, position: usize) -> &[u8] {
        let row = self.word(Section::FunctionsByName, position, 0) as usize;
        self.text(self.word(Section::Functions, row, 0))
    }

    /// The position of the struct `name` in name order.
    fn struct_position(&self, name: &str) -> Option<usize> {
        self.find(Section::Structs, |row| {
            self.text(self.word(Section::Structs, row, 0))
                .cmp(name.as_bytes())
        })
    }

    /// The position of the declaration named `name`.
    fn declaration_position(&self, name: &str) -> Option<usize> {
        self.find(Section::Declarations, |row| {
            self.text(self.word(Section::Declarations, row, 0))
                .cmp(name.as_bytes())
        })
    }
}

/// One library module, read from the module table: its identity, and where
/// its link record lies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleEntry<'a> {
    /// Its dotted name (`std.io.file`).
    pub name: &'a str,
    /// The canonical path of its source file, its identity in a link.
    pub canonical: &'a str,
    /// The path its declarations' locations name.
    pub path: &'a str,
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

/// One library declaration, read from the directory: its name, its module,
/// and where its record lies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclarationEntry<'a> {
    pub name: &'a str,
    /// Its module's position in the module table.
    pub module: u32,
    pub record: Range<usize>,
    /// Where the residue starts, from the record's start.
    pub residue: usize,
}

/// One library function's body as a package build writes it: its name, its
/// runtime parameter count, its `mut`/`ref` parameter mask (receiver
/// included), and where its encoded body lies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackagedBody {
    pub name: String,
    pub n_params: usize,
    pub ref_params: Vec<bool>,
    pub range: Range<usize>,
}

/// A package's declarations: each record's lowered part decoded the first
/// time one of its names is looked up, once per process.
///
/// A package is shared by every compilation in a process, so nothing an
/// importer adds to a declaration lives here.
#[derive(Debug)]
pub struct PackageDeclarations {
    directory: DeclarationDirectory,
    decoded: Vec<OnceLock<Option<LoweredDeclaration>>>,
    failures: Mutex<Vec<String>>,
}

impl PackageDeclarations {
    /// The declarations the directory at `range` of `payload` places.
    ///
    /// # Errors
    ///
    /// A directory whose string section or tables overrun it.
    pub fn read(payload: Payload, range: &Range<usize>) -> Result<Self, String> {
        let directory = DeclarationDirectory::read(payload, range)?;
        let decoded = (0..directory.declaration_count())
            .map(|_| OnceLock::new())
            .collect();
        Ok(Self {
            directory,
            decoded,
            failures: Mutex::new(Vec::new()),
        })
    }

    /// `records` encoded on their own, as a package holding no bodies
    /// would hold them.
    ///
    /// # Errors
    ///
    /// A record that does not encode.
    pub fn of_records(records: DeclarationRecords) -> Result<Self, String> {
        let mut payload = Vec::new();
        let range = records.encode(None, &mut |bytes| {
            let start = payload.len();
            payload.extend_from_slice(bytes);
            start..payload.len()
        })?;
        Self::read(Payload::new(PackageBytes::Owned(payload), 0), &range)
    }

    /// The struct declaration `name`, decoded on first use; `None` when the
    /// package declares no such struct or its record does not decode (a
    /// failure [`Self::failures`] reports).
    pub fn structure(&self, name: &str) -> Option<&StructDeclaration> {
        self.structure_at(self.struct_position(name)?)
    }

    /// The function declaration `name`, decoded on first use.
    pub fn function(&self, name: &str) -> Option<&FunctionDeclaration> {
        self.function_at(self.directory.function_position(name)?)
    }

    /// Whether the package declares a function `name`, decoding nothing.
    pub fn contains_function(&self, name: &str) -> bool {
        self.directory.function_position(name).is_some()
    }

    /// The position of the struct `name` in package order, decoding
    /// nothing.
    pub fn struct_position(&self, name: &str) -> Option<usize> {
        self.directory.struct_position(name)
    }

    /// The struct declaration at `position` in package order.
    pub fn structure_at(&self, position: usize) -> Option<&StructDeclaration> {
        let word = |column| self.directory.word(Section::Structs, position, column);
        self.lowered(word(1) as usize)?
            .structs
            .get(word(2) as usize)
    }

    /// The function declaration at `position` in package order.
    pub fn function_at(&self, position: usize) -> Option<&FunctionDeclaration> {
        let directory = &self.directory;
        let row = directory.word(Section::FunctionsByName, position, 0) as usize;
        let word = |column| directory.word(Section::Functions, row, column);
        self.lowered(word(1) as usize)?
            .functions
            .get(word(2) as usize)
    }

    /// Every struct name, in package order, decoding nothing.
    pub fn struct_names(&self) -> impl Iterator<Item = &str> {
        let directory = &self.directory;
        (0..directory.rows(Section::Structs))
            .map(|row| directory.string(directory.word(Section::Structs, row, 0)))
    }

    /// Every function name, in package order, decoding nothing.
    pub fn function_names(&self) -> impl Iterator<Item = &str> {
        let directory = &self.directory;
        (0..directory.rows(Section::FunctionsByName)).map(|position| {
            let row = directory.word(Section::FunctionsByName, position, 0) as usize;
            directory.string(directory.word(Section::Functions, row, 0))
        })
    }

    /// The name and runtime parameter count of every function whose name
    /// starts with `prefix`, in name order, decoding nothing: a range of
    /// the by-name table, as upstream looks an overload set up by name.
    pub fn functions_prefixed<'s>(
        &'s self,
        prefix: &str,
    ) -> impl Iterator<Item = (&'s str, usize)> + use<'s> {
        let directory = &self.directory;
        directory.function_positions(prefix).map(|position| {
            let row = directory.word(Section::FunctionsByName, position, 0) as usize;
            (
                directory.string(directory.word(Section::Functions, row, 0)),
                directory.word(Section::Functions, row, 3) as usize,
            )
        })
    }

    /// How many structs the package declares.
    pub const fn struct_count(&self) -> usize {
        self.directory.rows(Section::Structs)
    }

    /// Every function's name and runtime parameter count, in the order the
    /// package's bodies were lowered in, decoding nothing.
    pub fn bodies(&self) -> impl Iterator<Item = (&str, usize)> + Clone {
        let directory = &self.directory;
        (0..directory.rows(Section::Functions)).map(|row| {
            (
                directory.string(directory.word(Section::Functions, row, 0)),
                directory.word(Section::Functions, row, 3) as usize,
            )
        })
    }

    /// How many functions the package declares.
    pub const fn body_count(&self) -> usize {
        self.directory.rows(Section::Functions)
    }

    /// The row of the function `name` among [`Self::bodies`].
    pub fn body_row(&self, name: &str) -> Option<usize> {
        self.directory.function_row(name)
    }

    /// The name of the function at `row` among [`Self::bodies`].
    pub fn body_name(&self, row: usize) -> &str {
        let directory = &self.directory;
        directory.string(directory.word(Section::Functions, row, 0))
    }

    /// Where the body of the function at `row` among [`Self::bodies`]
    /// lies in the payload; `None` when the package holds no body for it.
    pub fn body_range(&self, row: usize) -> Option<Range<usize>> {
        let word = |column| self.directory.word(Section::Functions, row, column);
        let range = word(6) as usize..word(7) as usize;
        (row < self.body_count() && !range.is_empty()).then_some(range)
    }

    /// Whether the function `name` takes its parameter at `slot` (receiver
    /// at slot zero) by `mut` or `ref`; `None` when the package declares
    /// no such function or it has no such parameter.
    pub fn ref_param(&self, name: &str, slot: usize) -> Option<bool> {
        let directory = &self.directory;
        let row = directory.function_row(name)?;
        let word = |column| directory.word(Section::Functions, row, column);
        if slot >= word(5) as usize {
            return None;
        }
        let index = (word(4) as usize).saturating_add(slot / 32);
        (index < directory.rows(Section::Masks))
            .then(|| (directory.word(Section::Masks, index, 0) >> (slot % 32)) & 1 == 1)
    }

    /// Every trait the package declares, sorted.
    pub fn traits(&self) -> impl Iterator<Item = &str> {
        self.directory
            .table_keys(InterfaceTable::Traits)
            .map(|(name, _)| name)
    }

    /// Whether the package declares the trait `name`, decoding nothing.
    pub fn declares_trait(&self, name: &str) -> bool {
        self.directory
            .key_declaration(InterfaceTable::Traits, name)
            .is_some()
    }

    /// The trait declaration `name`, decoded on first use; `None` when the
    /// package declares no such trait.
    pub fn trait_declaration(&self, name: &str) -> Option<&TraitDeclaration> {
        let entry = self
            .directory
            .key_declaration(InterfaceTable::Traits, name)?;
        self.lowered(entry as usize)?
            .traits
            .iter()
            .find(|declaration| declaration.name == name)
    }

    /// The directory, which names every module and declaration record.
    pub const fn directory(&self) -> &DeclarationDirectory {
        &self.directory
    }

    /// The package's module table, sorted by dotted name.
    pub fn modules(&self) -> impl ExactSizeIterator<Item = ModuleEntry<'_>> + '_ {
        self.directory.modules()
    }

    /// The encoded link record of the module at `position` in the module
    /// table.
    pub fn link_record(&self, position: usize) -> Option<&[u8]> {
        let module = self.directory.module(position)?;
        Some(self.directory.payload.get(&module.link))
    }

    /// Whether a declaration stamped with `module` comes from one of the
    /// package's modules, so its body is in the package.
    pub fn compiles(&self, module: Option<&str>) -> bool {
        module.is_some_and(|module| self.modules().any(|entry| entry.path == module))
    }

    /// The lowered part of the declaration at `entry`, decoded on first
    /// use.
    pub fn lowered(&self, entry: usize) -> Option<&LoweredDeclaration> {
        self.decoded
            .get(entry)?
            .get_or_init(|| {
                let declaration = self.directory.declaration(entry)?;
                let record = self.directory.payload.get(&declaration.record);
                record
                    .get(..declaration.residue)
                    .ok_or_else(|| "its record is truncated".to_string())
                    .and_then(|lowered| {
                        let mut lowered: LoweredDeclaration =
                            postcard::from_bytes(lowered).map_err(|error| error.to_string())?;
                        lowered.join(&self.directory, entry, declaration.name)?;
                        Ok(lowered)
                    })
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
        self.directory
            .declaration(entry)
            .and_then(|declaration| {
                self.directory
                    .payload
                    .get(&declaration.record)
                    .get(declaration.residue..)
            })
            .unwrap_or_default()
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
        declaration_candidates(name)
            .find_map(|candidate| self.directory.declaration_position(candidate))
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

    /// Whether the link reached the module of the declaration at `entry`.
    fn reaches(&self, entry: usize) -> bool {
        self.package
            .directory
            .declaration(entry)
            .is_some_and(|declaration| {
                self.reached
                    .get(declaration.module as usize)
                    .copied()
                    .unwrap_or(false)
            })
    }

    /// The source arity of every reached function of the overload set
    /// `family`: the declarations a lookup finds under its name, the plain
    /// one and each signature-qualified one, decoded on first use.
    fn arities(&self, family: OverloadFamily<'_>) -> Vec<usize> {
        let (OverloadFamily::Free(name) | OverloadFamily::Method(name)) = family;
        let directory = &self.package.directory;
        directory
            .function_position(name)
            .into_iter()
            .chain(directory.function_positions(&format!("{name}{OV_SEP}")))
            .filter(|position| {
                let row = directory.word(Section::FunctionsByName, *position, 0) as usize;
                self.reaches(directory.word(Section::Functions, row, 1) as usize)
            })
            .filter_map(|position| self.package.function_at(position)?.overload_member())
            .filter(|(member, _)| *member == family)
            .map(|(_, arity)| arity)
            .collect()
    }

    /// Whether `arities`, one overload set's, are an overloaded name's
    /// that defines `arity`.
    fn overloaded(arities: &[usize], arity: usize) -> bool {
        arities.len() > 1 && arities.contains(&arity)
    }
}

impl OverloadLookup for PackageScope {
    fn function_is_overloaded(&self, name: &str, arity: usize) -> bool {
        Self::overloaded(&self.arities(OverloadFamily::Free(name)), arity)
    }

    fn is_function(&self, name: &str) -> bool {
        !self.arities(OverloadFamily::Free(name)).is_empty()
    }

    fn method_is_overloaded(&self, source_name: &str, arity: usize) -> bool {
        Self::overloaded(&self.arities(OverloadFamily::Method(source_name)), arity)
    }

    fn fieldwise_parameters(&self, name: &str) -> Option<Cow<'_, [String]>> {
        let position = self.package.struct_position(name)?;
        let entry = self.package.directory.word(Section::Structs, position, 1) as usize;
        if !self.reaches(entry) {
            return None;
        }
        self.package
            .structure_at(position)?
            .fieldwise_parameters()
            .map(Cow::Owned)
    }

    fn comptime(&self, name: &str) -> Option<i64> {
        let entry = self.package.owning(name)?;
        if !self.reaches(entry) {
            return None;
        }
        self.package.lowered(entry)?.constant(name)
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
        self.own.scan_more(program, |name| package?.comptime(name));
    }

    /// Record the own method `source_name`'s declarations by arity
    /// ([`OverloadSets::declare_method`]).
    pub fn declare_method(&mut self, source_name: &str, arities: Vec<usize>) {
        self.own.declare_method(source_name, arities);
    }
}

impl OverloadLookup for ProgramOverloads {
    fn function_is_overloaded(&self, name: &str, arity: usize) -> bool {
        self.own.function_is_overloaded(name, arity)
            || self
                .package
                .as_ref()
                .is_some_and(|package| package.function_is_overloaded(name, arity))
    }

    fn is_function(&self, name: &str) -> bool {
        self.own.is_function(name)
            || self
                .package
                .as_ref()
                .is_some_and(|package| package.is_function(name))
    }

    fn method_is_overloaded(&self, source_name: &str, arity: usize) -> bool {
        self.own.method_is_overloaded(source_name, arity)
            || self
                .package
                .as_ref()
                .is_some_and(|package| package.method_is_overloaded(source_name, arity))
    }

    fn fieldwise_parameters(&self, name: &str) -> Option<Cow<'_, [String]>> {
        self.own
            .fieldwise_parameters(name)
            .or_else(|| self.package.as_ref()?.fieldwise_parameters(name))
    }

    fn comptime(&self, name: &str) -> Option<i64> {
        self.own
            .comptime(name)
            .or_else(|| self.package.as_ref()?.comptime(name))
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

    /// Put the integer value of the module constant `name` in the record
    /// of the declaration it belongs to.
    ///
    /// # Errors
    ///
    /// No library declaration owns `name`.
    pub fn place_constant(&mut self, name: String, value: i64) -> Result<(), String> {
        let record = self.record_of(&name)?;
        sorted_insert(&mut record.lowered.constants, name, value);
        Ok(())
    }

    /// Encode every record with `append`, which stores its bytes and
    /// answers their range, then the directory placing them and `bodies`,
    /// and answer the directory's range.
    ///
    /// `bodies` are the package's function bodies in lowering order, each
    /// joined to its function's declaration; `None` encodes a package of
    /// declarations alone.
    ///
    /// # Errors
    ///
    /// A record that does not encode, or whose names or masks would not
    /// join back from the directory, a module recorded twice, a
    /// declaration of no recorded module, a name two declarations hold, a
    /// body no declaration declares, or a declared function with no body.
    pub fn encode(
        mut self,
        bodies: Option<Vec<PackagedBody>>,
        append: &mut impl FnMut(&[u8]) -> Range<usize>,
    ) -> Result<Range<usize>, String> {
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
        let mut functions: HashMap<String, (u32, u32)> = HashMap::new();
        for (position, record) in self.declarations.values().enumerate() {
            for (index, declaration) in record.lowered.functions.iter().enumerate() {
                if functions
                    .insert(
                        declaration.lowered_name.clone(),
                        (position as u32, index as u32),
                    )
                    .is_some()
                {
                    return Err(format!(
                        "function `{}` is declared twice",
                        declaration.lowered_name
                    ));
                }
            }
        }
        let bodies = if let Some(bodies) = bodies {
            let mut joined = Vec::with_capacity(bodies.len());
            for body in bodies {
                let (declaration, index) = functions
                    .remove(&body.name)
                    .ok_or_else(|| format!("body `{}` has no declaration", body.name))?;
                joined.push((body, declaration, index));
            }
            if let Some(name) = functions.keys().min() {
                return Err(format!("function `{name}` has no body"));
            }
            joined
        } else {
            let mut declared: Vec<_> = functions
                .into_iter()
                .map(|(name, (declaration, index))| {
                    let body = PackagedBody {
                        name,
                        n_params: 0,
                        ref_params: Vec::new(),
                        range: 0..0,
                    };
                    (body, declaration, index)
                })
                .collect();
            declared.sort_by(|left, right| left.0.name.cmp(&right.0.name));
            declared
        };
        let mut function_rows: Vec<Vec<u32>> = self
            .declarations
            .values()
            .map(|record| vec![0; record.lowered.functions.len()])
            .collect();
        for (row, (_, declaration, index)) in bodies.iter().enumerate() {
            function_rows[*declaration as usize][*index as usize] = offset(row)?;
        }
        let mut writer = DirectoryWriter::default();
        let mut structs = Vec::new();
        let mut keys = Vec::new();
        let mut methods = Vec::new();
        let mut bindings = vec![Vec::new(); self.modules.len()];
        for (position, ((name, mut record), rows)) in
            self.declarations.into_iter().zip(function_rows).enumerate()
        {
            let position = position as u32;
            let module = positions
                .get(record.module.as_str())
                .copied()
                .ok_or_else(|| format!("`{name}` has no module record"))?;
            record.lowered.refuse_unjoinable(&name)?;
            record.lowered.function_rows = rows;
            let mut bytes = postcard::to_stdvec(&record.lowered)
                .map_err(|error| format!("the declarations of `{name}` do not encode: {error}"))?;
            let residue = bytes.len();
            bytes.extend_from_slice(&record.residue);
            let range = append(&bytes);
            let mut declared = record.keys;
            declared.sort();
            declared.dedup();
            let first_key = writer.rows(Section::DeclarationKeys);
            for (table, key) in &declared {
                let key = writer.string(key);
                writer.row(Section::DeclarationKeys, &[*table as u32, key]);
            }
            let row = [
                writer.string(&name),
                module,
                offset(range.start)?,
                offset(range.end)?,
                offset(residue)?,
                offset(first_key)?,
                offset(declared.len())?,
            ];
            writer.row(Section::Declarations, &row);
            for (index, declaration) in record.lowered.structs.iter().enumerate() {
                structs.push((declaration.name.clone(), position, index as u32));
            }
            for (table, key) in declared {
                if table == InterfaceTable::Bindings {
                    bindings[module as usize].push(key.clone());
                }
                keys.push((table, key, position));
            }
            methods.extend(record.methods.into_iter().map(|method| (method, position)));
        }
        for (body, declaration, index) in &bodies {
            let name = writer.string(&body.name);
            let mask = writer.rows(Section::Masks);
            for chunk in body.ref_params.chunks(32) {
                let word = chunk
                    .iter()
                    .enumerate()
                    .fold(0, |word, (bit, set)| word | (u32::from(*set) << bit));
                writer.row(Section::Masks, &[word]);
            }
            let row = [
                name,
                *declaration,
                *index,
                offset(body.n_params)?,
                offset(mask)?,
                offset(body.ref_params.len())?,
                offset(body.range.start)?,
                offset(body.range.end)?,
            ];
            writer.row(Section::Functions, &row);
        }
        let mut by_name: Vec<usize> = (0..bodies.len()).collect();
        by_name.sort_by(|left, right| bodies[*left].0.name.cmp(&bodies[*right].0.name));
        for position in by_name {
            writer.row(Section::FunctionsByName, &[offset(position)?]);
        }
        structs.sort();
        if let Some(pair) = structs.windows(2).find(|pair| pair[0].0 == pair[1].0) {
            return Err(format!("struct `{}` is declared twice", pair[0].0));
        }
        for (name, declaration, index) in structs {
            let name = writer.string(&name);
            writer.row(Section::Structs, &[name, declaration, index]);
        }
        keys.sort();
        if let Some(pair) = keys
            .windows(2)
            .find(|pair| (pair[0].0, &pair[0].1) == (pair[1].0, &pair[1].1))
        {
            return Err(format!(
                "two declarations hold the {:?} key `{}`",
                pair[0].0, pair[0].1
            ));
        }
        for (table, key, declaration) in keys {
            let key = writer.string(&key);
            writer.row(Section::Keys, &[table as u32, key, declaration]);
        }
        methods.sort();
        for (method, declaration) in methods {
            let method = writer.string(&method);
            writer.row(Section::Methods, &[method, declaration]);
        }
        for (module, mut names) in self.modules.into_iter().zip(bindings) {
            let link = append(&module.link);
            names.sort();
            let first_binding = writer.rows(Section::Bindings);
            for name in &names {
                let name = writer.string(name);
                writer.row(Section::Bindings, &[name]);
            }
            let row = [
                writer.string(&module.name),
                writer.string(&module.canonical),
                writer.string(&module.path),
                offset(link.start)?,
                offset(link.end)?,
                offset(first_binding)?,
                offset(names.len())?,
            ];
            writer.row(Section::Modules, &row);
        }
        Ok(append(&writer.finish()))
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
/// A table of a package's declaration directory.
#[derive(Debug, Clone, Copy)]
enum Section {
    /// `[name, canonical path, stamped path, link start, link end, first
    /// binding, binding count]`, sorted by name.
    Modules,
    /// `[name, module, record start, record end, residue start, first key,
    /// key count]`, sorted by name.
    Declarations,
    /// `[name, declaration, index in its record]`, sorted by name: the
    /// package order of structs.
    Structs,
    /// `[name, declaration, index in its record, runtime parameter count,
    /// first mask word, mask length, body start, body end]`, in the order
    /// the bodies were lowered in.
    Functions,
    /// `[function row]`, sorted by the function's name: the package order
    /// of function declarations.
    FunctionsByName,
    /// `[table, key]`, each declaration's keys together and sorted.
    DeclarationKeys,
    /// `[table, key, declaration]`, sorted by table, then key.
    Keys,
    /// `[method, declaration]`, sorted by method, then declaration.
    Methods,
    /// `[name]`, each module's bindings together and sorted.
    Bindings,
    /// `[mask word]`, a function's `mut`/`ref` parameter mask, 32 slots a
    /// word, lowest bit first.
    Masks,
}

impl Section {
    const COUNT: usize = Self::Masks as usize + 1;
    const ALL: [Self; Self::COUNT] = [
        Self::Modules,
        Self::Declarations,
        Self::Structs,
        Self::Functions,
        Self::FunctionsByName,
        Self::DeclarationKeys,
        Self::Keys,
        Self::Methods,
        Self::Bindings,
        Self::Masks,
    ];

    /// How many words each of its rows holds.
    const fn width(self) -> usize {
        match self {
            Self::Modules | Self::Declarations => 7,
            Self::Structs | Self::Keys => 3,
            Self::Functions => 8,
            Self::DeclarationKeys | Self::Methods => 2,
            Self::FunctionsByName | Self::Bindings | Self::Masks => 1,
        }
    }
}

/// A declaration directory as a package build writes it: its string
/// section, each string interned once, and its tables' words.
#[derive(Default)]
struct DirectoryWriter {
    strings: Vec<String>,
    interned: HashMap<String, u32>,
    tables: [Vec<u32>; Section::COUNT],
}

impl DirectoryWriter {
    /// The index of the string `text`, interned on first use.
    fn string(&mut self, text: &str) -> u32 {
        if let Some(index) = self.interned.get(text) {
            return *index;
        }
        let index = self.strings.len() as u32;
        self.strings.push(text.to_string());
        self.interned.insert(text.to_string(), index);
        index
    }

    fn row(&mut self, section: Section, words: &[u32]) {
        self.tables[section as usize].extend_from_slice(words);
    }

    const fn rows(&self, section: Section) -> usize {
        self.tables[section as usize].len() / section.width()
    }

    /// The directory's bytes: the string section, each table's row count,
    /// then the tables.
    fn finish(self) -> Vec<u8> {
        let mut bytes = Vec::new();
        push_leb128(&mut bytes, self.strings.len() as u64);
        for text in self.strings.iter().rev() {
            push_leb128(&mut bytes, text.len() as u64);
        }
        for text in &self.strings {
            bytes.extend_from_slice(text.as_bytes());
        }
        for section in Section::ALL {
            let rows = self.tables[section as usize].len() / section.width();
            bytes.extend_from_slice(&(rows as u32).to_le_bytes());
        }
        for word in self.tables.iter().flatten() {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes
    }
}

/// The first row of `rows` `before` is false of, `before` being true of a
/// prefix of them.
fn partition_point(rows: Range<usize>, before: impl Fn(usize) -> bool) -> usize {
    let (mut low, mut high) = (rows.start, rows.end);
    while low < high {
        let middle = low + (high - low) / 2;
        if before(middle) {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    low
}

/// `value` as a directory word.
fn offset(value: usize) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| format!("{value} does not fit a package directory word"))
}

/// The little-endian word at `at` in `bytes`; `u32::MAX` past their end.
fn word_at(bytes: &[u8], at: usize) -> u32 {
    at.checked_add(4)
        .and_then(|end| bytes.get(at..end))
        .and_then(|word| word.try_into().ok())
        .map_or(u32::MAX, u32::from_le_bytes)
}

/// The LEB128 number at `cursor` in `bytes`, advancing past it.
fn leb128(bytes: &[u8], cursor: &mut usize) -> Result<u64, String> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes
            .get(*cursor)
            .ok_or_else(|| "the declaration directory's strings are truncated".to_string())?;
        *cursor += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("the declaration directory holds a malformed length".to_string())
}

fn push_leb128(bytes: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            bytes.push(byte);
            return;
        }
        bytes.push(byte | 0x80);
    }
}
