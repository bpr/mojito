//! The compiled package container, one file per package.
//!
//! It holds the package's checker interface, the record its importer's link
//! reads for each of its source modules, one record per library
//! declaration (`mojito_checked::package`), each library module's checker
//! record, and each of its verified, ownership-checked function bodies,
//! before drop elaboration. An importer maps the file and reads its
//! directory and interface eagerly, a declaration's record the first time
//! one of its phases looks one of its names up (as upstream's importer
//! resolves a declaration from bytecode on first lookup), a module's record
//! when its link reaches the module, and a body only when the elaborator
//! first reaches it, inserting that body's drops then, as upstream
//! materializes a package's bodies on demand and runs its lifetime pass on
//! each in the importer.
//!
//! The container is a directory followed by records:
//!
//! ```text
//! mojito-package 11
//! <the directory's length, 8 bytes little-endian>
//! <the directory, as bytecode>
//! <the payload: every record, as bytecode>
//! ```
//!
//! The directory holds the header and the payload range of every record:
//! the interface, the source-file table every body's locations name, the
//! declaration directory (each declaration's record and every name it
//! answers, each module's checker record, and the package order of every
//! struct and function declaration), each link module record (by dotted
//! module name, beside its source file's canonical and stamped paths), and
//! each body (in package order, with its runtime parameter count, its
//! reference parameter mask, and a by-name permutation). Every record is
//! postcard bytecode; a link module record and a declaration's checker
//! residue are opaque here, since the linker and the checker that read
//! them sit above MIR. No part of a package is MIR text.

use crate::mir::{MirDeclarations, MirFunction, MirProgram, SpanTable};
use mojito_checked::interface::PackageInterface;
use mojito_checked::package::{DeclarationDirectory, LoweredDeclaration, Payload};
pub use mojito_checked::package::{DeclarationRecords, PackageBytes, PackageDeclarations};
use mojito_common::token::SourceSpan;
use mojito_hir::hir::VarId;
use mojito_types::origin::encoding_packaged_owners;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock};

/// The container format this compiler writes and reads.
pub const PACKAGE_FORMAT: &str = "mojito-package 11";

/// Who built a package from what: an importer refuses a package another
/// compiler built, or one built from other sources.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PackageHeader {
    /// The package's name (`std`).
    pub package: String,
    /// The identity of the compiler build that wrote it.
    pub compiler: String,
    /// The library root its modules' stamped paths name.
    pub root: String,
}

/// A read package: its header, its checker interface, its link module
/// records, and its declarations and bodies, each decoded on demand.
#[derive(Debug)]
pub struct Package {
    pub header: PackageHeader,
    pub interface: PackageInterface,
    pub declarations: Arc<PackageDeclarations>,
    /// Each module's encoded record, beside the module's identity: copied
    /// out of the file, since the linker that reads them sits above MIR and
    /// holds no [`PackageBytes`].
    pub modules: Vec<(PackagedModuleKey, Vec<u8>)>,
    pub bodies: PackageBodies,
}

/// Which module a package's module record is: its dotted name, the
/// canonical path of its source file, and the path its declarations'
/// locations name.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PackagedModuleKey {
    pub name: String,
    pub canonical: String,
    pub path: String,
}

/// A package's function bodies, each decoded the first time it is asked
/// for, then finished as its importer finishes a body it lowers itself.
#[derive(Debug)]
pub struct PackageBodies {
    payload: Payload,
    /// The source file each body location names, by file id.
    files: Vec<Option<String>>,
    /// The package's own declarations, which its bodies were lowered
    /// against, resolved through the same lookups an importer's are.
    declarations: MirDeclarations,
    entries: Vec<BodyEntry>,
    /// Each entry's position, sorted by the entry's name.
    by_name: Vec<u32>,
    decoded: Vec<OnceLock<Option<MirFunction>>>,
    finish: Option<BodyFinish>,
    failures: Mutex<Vec<String>>,
}

/// What an importer does to a decoded body before using it — insert its
/// drops — given the body's name and the package's declarations. A plain
/// function, since the phase it runs sits above MIR.
pub type BodyFinish = fn(&str, MirFunction, &MirDeclarations) -> MirFunction;

impl PackageBodies {
    /// The body of `name`, decoded on first use; `None` when the package
    /// has no such function or its record does not decode (a failure
    /// [`Self::failures`] reports).
    pub fn get(&self, name: &str) -> Option<&MirFunction> {
        let position = self.position(name)?;
        self.decoded[position]
            .get_or_init(|| {
                let entry = &self.entries[position];
                let function = match self.decode(entry) {
                    Ok(function) => function,
                    Err(error) => {
                        self.fail(format!(
                            "package body `{}` does not decode: {error}",
                            entry.name
                        ));
                        return None;
                    }
                };
                let Some(finish) = self.finish else {
                    return Some(function);
                };
                let function = finish(&entry.name, function, &self.declarations);
                let findings = crate::mir::verify::verify_function_in(
                    &entry.name,
                    &function,
                    &self.declarations,
                );
                if !findings.is_empty() {
                    self.fail(findings.join("; "));
                }
                Some(function)
            })
            .as_ref()
    }

    /// Finish every body with `finish` as it is decoded, and verify it.
    #[must_use]
    pub fn finishing(mut self, finish: BodyFinish) -> Self {
        self.finish = Some(finish);
        self
    }

    /// Whether the package has a function `name`.
    pub fn contains(&self, name: &str) -> bool {
        self.position(name).is_some()
    }

    /// Every function's name and runtime parameter count, in package order.
    pub fn signatures(&self) -> impl Iterator<Item = (&str, usize)> + Clone {
        self.entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.n_params))
    }

    /// Each function's `mut`/`ref` parameter mask, receiver included, by
    /// name.
    pub fn ref_params(&self) -> impl Iterator<Item = (&str, &[bool])> {
        self.entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.ref_params.as_slice()))
    }

    /// How many bodies have been decoded so far.
    pub fn materialized(&self) -> usize {
        self.decoded
            .iter()
            .filter(|cell| cell.get().is_some())
            .count()
    }

    /// Every body, decoded, in package order.
    pub fn all(&self) -> Vec<(String, MirFunction)> {
        self.entries
            .iter()
            .filter_map(|entry| {
                self.get(&entry.name)
                    .map(|function| (entry.name.clone(), function.clone()))
            })
            .collect()
    }

    /// The bodies that failed to decode so far.
    pub fn failures(&self) -> Vec<String> {
        self.failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn position(&self, name: &str) -> Option<usize> {
        let found = self
            .by_name
            .binary_search_by(|position| {
                self.entries[*position as usize]
                    .name
                    .as_bytes()
                    .cmp(name.as_bytes())
            })
            .ok()?;
        Some(self.by_name[found] as usize)
    }

    /// The body `entry` locates, its locations rebuilt against the
    /// package's file table.
    fn decode(&self, entry: &BodyEntry) -> Result<MirFunction, String> {
        let (mut function, locations): (MirFunction, Vec<Location>) =
            encoding_packaged_owners(|| postcard::from_bytes(self.payload.get(&entry.range)))
                .map_err(|error| error.to_string())?;
        let mut spans = SpanTable::default();
        for location in locations {
            let source = self
                .files
                .get(location.file as usize)
                .ok_or_else(|| format!("location names no file {}", location.file))?;
            spans.0.insert(
                location.reg,
                (
                    SourceSpan::new(source.clone(), (location.start, location.end)),
                    location.origin,
                ),
            );
        }
        function.spans = spans;
        Ok(function)
    }

    fn fail(&self, message: String) {
        self.failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(message);
    }
}

/// Why a package could not be written or read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageError(pub String);

impl fmt::Display for PackageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid package: {}", self.0)
    }
}

impl std::error::Error for PackageError {}

/// Write a package of `program`, its verified, ownership-checked MIR.
///
/// The package holds `program` before drop elaboration, its interface, its
/// `modules`' encoded link records, and its declaration `records`, which
/// hold every struct and function declaration of `program`.
///
/// Verification runs first, so an error never leaves a partial package.
pub fn write_package(
    header: &PackageHeader,
    interface: &PackageInterface,
    program: &MirProgram,
    modules: &[(PackagedModuleKey, Vec<u8>)],
    records: DeclarationRecords,
) -> Result<Vec<u8>, PackageError> {
    let mut findings = program.invariant_errors.clone();
    findings.extend(crate::mir::verify::verify(program));
    findings.extend(super::write::schema_findings(program));
    let recorded = |names: fn(&LoweredDeclaration) -> Vec<&String>| {
        let mut recorded: Vec<&String> = records
            .declarations
            .values()
            .flat_map(|record| names(&record.lowered))
            .collect();
        recorded.sort();
        recorded
    };
    let mut structs: Vec<&String> = program
        .declarations
        .structs
        .iter()
        .map(|declaration| &declaration.name)
        .collect();
    structs.sort();
    let mut functions: Vec<&String> = program
        .declarations
        .functions
        .iter()
        .map(|declaration| &declaration.lowered_name)
        .collect();
    functions.sort();
    if recorded(|lowered| {
        lowered
            .structs
            .iter()
            .map(|declaration| &declaration.name)
            .collect()
    }) != structs
        || recorded(|lowered| {
            lowered
                .functions
                .iter()
                .map(|declaration| &declaration.lowered_name)
                .collect()
        }) != functions
    {
        findings.push("the declaration records hold other declarations than the program".into());
    }
    let mut declared: Vec<&String> = records
        .declarations
        .values()
        .flat_map(|record| &record.keys)
        .filter(|(table, _)| *table == mojito_checked::package::InterfaceTable::Traits)
        .map(|(_, key)| key)
        .collect();
    declared.sort();
    let mut traits: Vec<&String> = program.declarations.traits.iter().collect();
    traits.sort();
    if declared != traits {
        findings.push(format!(
            "the program declares the traits {traits:?}, its declarations {declared:?}"
        ));
    }
    if !findings.is_empty() {
        return Err(PackageError(findings.join("; ")));
    }
    let mut payload = Vec::new();
    let mut record = |bytes: &[u8]| {
        let start = payload.len();
        payload.extend_from_slice(bytes);
        start..payload.len()
    };
    let interface = record(&bytecode(interface)?);
    let files = files(program);
    let files_range = record(&bytecode(&files.keys().collect::<Vec<_>>())?);
    let declarations = records.encode(&mut record).map_err(PackageError)?;
    let modules = modules
        .iter()
        .map(|(key, bytes)| (key.clone(), record(bytes)))
        .collect();
    let bodies = program
        .functions
        .iter()
        .map(|(name, function)| {
            let locations = (0..function.n_regs)
                .filter_map(|reg| {
                    let (span, origin) = function.spans.0.get(&reg)?;
                    Some(Location {
                        reg,
                        file: files[&span.source],
                        start: span.span.0,
                        end: span.span.1,
                        origin: *origin,
                    })
                })
                .collect::<Vec<_>>();
            let bytes = encoding_packaged_owners(|| bytecode(&(function, locations))).map_err(
                |PackageError(error)| {
                    PackageError(format!("body `{name}` does not encode: {error}"))
                },
            )?;
            Ok(BodyEntry {
                name: name.clone(),
                n_params: function.n_params,
                ref_params: function.ref_params.clone(),
                range: record(&bytes),
            })
        })
        .collect::<Result<Vec<_>, PackageError>>()?;
    let mut by_name = (0..bodies.len() as u32).collect::<Vec<_>>();
    by_name.sort_by(|left, right| {
        bodies[*left as usize]
            .name
            .as_bytes()
            .cmp(bodies[*right as usize].name.as_bytes())
    });
    let directory = bytecode(&PackageDirectory {
        header: header.clone(),
        interface,
        files: files_range,
        declarations,
        modules,
        bodies,
        by_name,
        payload: payload.len(),
    })?;
    let mut output = format!("{PACKAGE_FORMAT}\n").into_bytes();
    output.extend_from_slice(&(directory.len() as u64).to_le_bytes());
    output.extend_from_slice(&directory);
    output.extend_from_slice(&payload);
    Ok(output)
}

/// Read a package: its header, interface, and module records now, and its
/// directory of declarations and bodies, which decode on demand.
pub fn read_package(bytes: PackageBytes) -> Result<Package, PackageError> {
    let rest = bytes
        .strip_prefix(format!("{PACKAGE_FORMAT}\n").as_bytes())
        .ok_or_else(|| PackageError(format!("expected `{PACKAGE_FORMAT}`")))?;
    let (length, rest) = rest
        .split_first_chunk::<8>()
        .ok_or_else(|| PackageError("truncated package".to_string()))?;
    let length = usize::try_from(u64::from_le_bytes(*length))
        .ok()
        .filter(|length| *length <= rest.len())
        .ok_or_else(|| PackageError("truncated package".to_string()))?;
    let reading = mojito_common::timing::span("package.directory");
    let directory: PackageDirectory = from_bytecode(&rest[..length], "directory")?;
    let start = bytes.len() - rest.len() + length;
    if bytes.len() - start != directory.payload {
        return Err(PackageError(format!(
            "the payload is {} bytes, not {}",
            bytes.len() - start,
            directory.payload
        )));
    }
    directory.validate()?;
    drop(reading);
    let payload = Payload::new(bytes, start);
    let decoding = mojito_common::timing::span("package.interface");
    let interface = from_bytecode(payload.get(&directory.interface), "interface")?;
    drop(decoding);
    let files = from_bytecode(payload.get(&directory.files), "files")?;
    let modules = directory
        .modules
        .into_iter()
        .map(|(key, range)| (key, payload.get(&range).to_vec()))
        .collect();
    let declarations = Arc::new(
        PackageDeclarations::read(payload.clone(), directory.declarations).map_err(PackageError)?,
    );
    Ok(Package {
        header: directory.header,
        interface,
        declarations: Arc::clone(&declarations),
        modules,
        bodies: PackageBodies {
            payload,
            files,
            declarations: MirDeclarations::of_package(declarations),
            decoded: directory.bodies.iter().map(|_| OnceLock::new()).collect(),
            entries: directory.bodies,
            by_name: directory.by_name,
            finish: None,
            failures: Mutex::new(Vec::new()),
        },
    })
}

/// Where every record of a package lies in its payload.
#[derive(serde::Serialize, serde::Deserialize)]
struct PackageDirectory {
    header: PackageHeader,
    interface: Range<usize>,
    files: Range<usize>,
    declarations: DeclarationDirectory,
    /// Each link module record, beside the module's identity.
    modules: Vec<(PackagedModuleKey, Range<usize>)>,
    /// Each body, in package order.
    bodies: Vec<BodyEntry>,
    /// Each body's position, sorted by the body's name.
    by_name: Vec<u32>,
    /// The payload's length.
    payload: usize,
}

impl PackageDirectory {
    /// Refuse a directory whose ranges leave the payload or whose bodies'
    /// names are not strictly sorted, which the lookups by name rely on
    /// (the declaration directory validates its own).
    fn validate(&self) -> Result<(), PackageError> {
        let ranges = [&self.interface, &self.files]
            .into_iter()
            .chain(self.modules.iter().map(|(_, range)| range))
            .chain(self.bodies.iter().map(|entry| &entry.range));
        for range in ranges {
            if range.start > range.end || !(..=self.payload).contains(&range.end) {
                return Err(PackageError("a record overruns the payload".to_string()));
            }
        }
        let sorted = self.by_name.len() == self.bodies.len()
            && self
                .by_name
                .iter()
                .all(|position| (*position as usize) < self.bodies.len())
            && self.by_name.windows(2).all(|pair| {
                self.bodies[pair[0] as usize].name.as_bytes()
                    < self.bodies[pair[1] as usize].name.as_bytes()
            });
        if sorted {
            Ok(())
        } else {
            Err(PackageError(
                "the bodies' name order is not a strict sort".to_string(),
            ))
        }
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct BodyEntry {
    name: String,
    n_params: usize,
    ref_params: Vec<bool>,
    range: Range<usize>,
}

/// A body register's source location, its file named by id.
#[derive(serde::Serialize, serde::Deserialize)]
struct Location {
    reg: u32,
    file: u32,
    start: usize,
    end: usize,
    origin: Option<VarId>,
}

fn bytecode<T: serde::Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, PackageError> {
    postcard::to_stdvec(value).map_err(|error| PackageError(error.to_string()))
}

fn from_bytecode<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    label: &str,
) -> Result<T, PackageError> {
    postcard::from_bytes(bytes)
        .map_err(|error| PackageError(format!("the {label} record does not decode: {error}")))
}

/// Every source file a body location names, by its file id.
fn files(program: &MirProgram) -> BTreeMap<&Option<String>, u32> {
    program
        .functions
        .iter()
        .flat_map(|(_, function)| {
            function
                .spans
                .0
                .iter()
                .filter(|(reg, _)| **reg < function.n_regs)
                .map(|(_, (span, _))| &span.source)
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .zip(0..)
        .collect()
}
