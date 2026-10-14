//! The compiled package container, one file per package.
//!
//! It holds its one module table, which holds the record its importer's link reads for each of its source
//! modules, one record per library declaration (both
//! `mojito_checked::package`), and each of its bodies as the check left
//! it: one record per lowering unit (`mojito_checked::unit::CheckedUnit`),
//! a declaration's checked syntax and the facts the check recorded for it.
//! An importer maps the file and decodes its header eagerly; it reads the declaration directory in place, a declaration's record the
//! first time one of its phases looks one of its names up (as upstream's
//! importer resolves a declaration from bytecode on first lookup), a
//! module's link record when its link reaches the module, and a unit only
//! when the elaborator first reaches one of its functions: it lowers the
//! unit then (HIR CFG, MIR) and inserts the reached body's drops. Upstream's
//! precompiled package likewise holds each body's IR as its parser wrote it
//! and its semantic checks validated it, and its importer lowers its
//! semantic control flow and runs its lifetime pass on each body it
//! materializes (`docs/notes/upstream-package-contents.md`).
//!
//! The container is a header followed by records:
//!
//! ```text
//! mojito-package 50
//! <the header's length, 8 bytes little-endian>
//! <the header, as bytecode>
//! <the payload: every record, as bytecode, then the type section>
//! ```
//!
//! The type section holds every type the records hold once, and a record
//! refers to a type by its index there, as MLIR bytecode's attribute and
//! type section holds each type once per file; an importer decodes an
//! entry the first time a record refers to it
//! (`mojito_types::type_section`).
//!
//! The header names the package and the compiler that built it, and holds
//! the payload ranges of the type section and of the declaration directory
//! (`mojito_checked::package::DeclarationDirectory`). That directory holds
//! every name once, in one string section, and places every module's link
//! record, every declaration's record, and every function's unit: one row
//! per function names it once and holds its declaration, its runtime
//! parameter count, its reference parameter mask, and the range of the
//! unit it lowers from, as an MLIR function op names its symbol once and
//! holds its region. Every record is postcard bytecode; a link module
//! record and a declaration's checker residue are opaque here, since the
//! linker and the checker that read them sit above MIR. No part of a
//! package is MIR.

use crate::mir::{MirDeclarations, MirFunction, MirProgram};
pub use mojito_checked::package::{
    DeclarationRecords, LibraryRoot, PackageBytes, PackageDeclarations,
};
use mojito_checked::package::{
    LoweredDeclaration, PackageScope, PackagedBody, Payload, ProgramConstants, ProgramOverloads,
};
use mojito_checked::unit::CheckedUnit;
use mojito_common::timing;
use mojito_types::type_section::encoding_types;
use std::collections::HashMap;
use std::fmt;
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock};

/// The container format this compiler writes and reads.
pub const PACKAGE_FORMAT: &str = "mojito-package 50";

/// Who built a package from what: an importer refuses a package another
/// compiler built, whose identity covers the library's sources.
///
/// It names no library root: a package names each file relative to its
/// library's root ([`LibraryRoot`]), and its importer joins its own.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PackageHeader {
    /// The package's name (`std`).
    pub package: String,
    /// The identity of the compiler build that wrote it.
    pub compiler: String,
}

/// A read package: its header, and its modules,
/// declarations, and bodies, each decoded on demand.
#[derive(Debug)]
pub struct Package {
    pub header: PackageHeader,
    /// Its module table and declaration records.
    pub declarations: Arc<PackageDeclarations>,
    pub bodies: PackageBodies,
}

/// A package's function bodies, each lowered from its unit the first time
/// it is asked for, then finished as its importer finishes a body it
/// lowers itself.
#[derive(Debug)]
pub struct PackageBodies {
    payload: Payload,
    /// The library root its importer reads its units' files under.
    root: LibraryRoot,
    /// The package's declarations, whose function table places each
    /// function's unit.
    package: Arc<PackageDeclarations>,
    /// The package's own declarations, which its bodies were lowered
    /// against, resolved through the same lookups an importer's are.
    declarations: MirDeclarations,
    /// The overload sets and module constants of the package alone, which
    /// a unit's lowering reads.
    overloads: Arc<ProgramOverloads>,
    constants: Arc<ProgramConstants>,
    /// Each unit lowered so far, by where its record starts.
    units: Mutex<HashMap<usize, Arc<OnceLock<LoweredUnit>>>>,
    /// Each function table row's body, once finished.
    decoded: Vec<OnceLock<Option<MirFunction>>>,
    finish: Option<BodyFinish>,
    failures: Mutex<Vec<String>>,
}

/// What an importer does to a lowered body before using it — insert its
/// drops — given the body's name and the package's declarations. A plain
/// function, since the phase it runs sits above MIR.
pub type BodyFinish = fn(&str, MirFunction, &MirDeclarations) -> MirFunction;

impl PackageBodies {
    /// The body of `name`, lowered and finished on first use; `None` when
    /// the package has no such function or its unit does not lower (a
    /// failure [`Self::failures`] reports).
    ///
    /// Its unit's decode and lowering, and its finish and verify, report as
    /// the `package.unit.*` and `package.body.*` `--timings` spans of the
    /// thread that first asks for it.
    pub fn get(&self, name: &str) -> Option<&MirFunction> {
        let row = self.package.body_row(name)?;
        self.decoded
            .get(row)?
            .get_or_init(|| {
                let function = match self.lowered(name) {
                    Ok(function) => function,
                    Err(error) => {
                        self.fail(format!("package body `{name}` does not lower: {error}"));
                        return None;
                    }
                };
                let Some(finish) = self.finish else {
                    return Some(function);
                };
                let function = {
                    let _finish = timing::span("package.body.finish");
                    finish(name, function, &self.declarations)
                };
                let _verify = timing::span("package.body.verify");
                let findings =
                    crate::mir::verify::verify_function_in(name, &function, &self.declarations);
                if !findings.is_empty() {
                    self.fail(findings.join("; "));
                }
                Some(function)
            })
            .as_ref()
    }

    /// The body of `name` as its unit lowers to it, before it is finished.
    ///
    /// # Errors
    ///
    /// When the package has no such function, or its unit does not decode
    /// or lower, or lowers to no such function.
    pub fn lowered(&self, name: &str) -> Result<MirFunction, String> {
        let row = self
            .package
            .body_row(name)
            .ok_or_else(|| "the package has no such function".to_string())?;
        let range = self
            .package
            .body_range(row)
            .ok_or_else(|| "the package holds no unit for it".to_string())?;
        let cell = Arc::clone(
            self.units
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(range.start)
                .or_default(),
        );
        let functions = cell.get_or_init(|| self.lower(&range));
        functions
            .as_ref()
            .map_err(Clone::clone)?
            .iter()
            .find(|(lowered, _)| lowered == name)
            .map(|(_, function)| function.clone())
            .ok_or_else(|| "its unit does not lower to it".to_string())
    }

    /// Finish every body with `finish` as it is lowered, and verify it.
    #[must_use]
    pub fn finishing(mut self, finish: BodyFinish) -> Self {
        self.finish = Some(finish);
        self
    }

    /// Whether the package has a function `name`.
    pub fn contains(&self, name: &str) -> bool {
        self.package.body_row(name).is_some()
    }

    /// The name and runtime parameter count of every function whose name
    /// starts with `prefix`, in name order, lowering nothing.
    pub fn functions_prefixed<'s>(
        &'s self,
        prefix: &str,
    ) -> impl Iterator<Item = (&'s str, usize)> + use<'s> {
        self.package.functions_prefixed(prefix)
    }

    /// The row in package order of the function `name`.
    pub fn row(&self, name: &str) -> Option<usize> {
        self.package.body_row(name)
    }

    /// How many bodies have been finished so far.
    pub fn materialized(&self) -> usize {
        self.decoded
            .iter()
            .filter(|cell| cell.get().is_some())
            .count()
    }

    /// How many units have been lowered so far.
    pub fn units_lowered(&self) -> usize {
        self.units
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .filter(|cell| cell.get().is_some())
            .count()
    }

    /// Every body, lowered and finished, in package order.
    pub fn all(&self) -> Vec<(String, MirFunction)> {
        self.package
            .bodies()
            .filter_map(|(name, _)| {
                self.get(name)
                    .map(|function| (name.to_string(), function.clone()))
            })
            .collect()
    }

    /// The bodies that failed to lower so far.
    pub fn failures(&self) -> Vec<String> {
        self.failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Decode the unit at `range` of the payload and lower it.
    fn lower(&self, range: &Range<usize>) -> LoweredUnit {
        let unit = {
            let _decode = timing::span("package.unit.decode");
            self.payload
                .decoding(|| CheckedUnit::decode(self.payload.get(range), &self.root))?
        };
        let _lower = timing::span("package.unit.lower");
        let program = unit.program(
            Arc::clone(&self.overloads),
            Arc::clone(&self.constants),
            &|name| {
                self.declarations
                    .structure(name)
                    .map(|declaration| declaration.param_decls.clone())
            },
        );
        crate::mir::lower_package_unit(&program, &self.declarations)
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

/// Write a package of `program`, its verified, ownership-checked MIR, as
/// the checked `units` it was lowered from, naming every file relative to
/// the library `root`.
///
/// The package holds the declaration records `records` gives, which hold
/// its module table and every struct and function declaration of
/// `program`, and each of `units` with the names of the functions it
/// places: every function of `program`, each once. It holds every type
/// those records hold once, in its type section, which the records refer
/// to by index ([`mojito_types::type_section`]); `records` runs inside that
/// encoding, so the checker residues it encodes refer to the section too.
///
/// Verification runs first, so an error never leaves a partial package.
pub fn write_package(
    header: &PackageHeader,
    root: &LibraryRoot,
    program: &MirProgram,
    units: &[(CheckedUnit, Vec<String>)],
    records: impl FnOnce() -> Result<DeclarationRecords, String>,
) -> Result<Vec<u8>, PackageError> {
    let (written, section) = encoding_types(|| {
        let records = records().map_err(PackageError)?;
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
            findings
                .push("the declaration records hold other declarations than the program".into());
        }
        let mut declared: Vec<&String> = records
            .declarations
            .values()
            .flat_map(|record| &record.keys)
            .filter(|(table, _)| *table == mojito_checked::package::InterfaceTable::Traits)
            .map(|(_, key)| key)
            .collect();
        declared.sort();
        let mut traits: Vec<&String> = program
            .declarations
            .traits
            .iter()
            .map(|declaration| &declaration.name)
            .collect();
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
        let functions: HashMap<&str, &MirFunction> = program
            .functions
            .iter()
            .map(|(name, function)| (name.as_str(), function))
            .collect();
        let mut placed = 0;
        let mut bodies = Vec::with_capacity(program.functions.len());
        for (unit, names) in units {
            if names.is_empty() {
                continue;
            }
            let range = record(&unit.encode(root).map_err(|error| {
                PackageError(format!(
                    "the unit of `{}` does not encode: {error}",
                    names[0]
                ))
            })?);
            for name in names {
                let function = functions.get(name.as_str()).ok_or_else(|| {
                    PackageError(format!("a unit places `{name}`, which the program lacks"))
                })?;
                placed += 1;
                bodies.push(PackagedBody {
                    name: name.clone(),
                    n_params: function.n_params,
                    ref_params: function.ref_params.clone(),
                    range: range.clone(),
                });
            }
        }
        if placed != program.functions.len() {
            return Err(PackageError(format!(
                "the units place {placed} functions, the program has {}",
                program.functions.len()
            )));
        }
        let declarations = records
            .encode(Some(bodies), Some(root), &mut record)
            .map_err(PackageError)?;
        Ok((payload, declarations))
    });
    let (mut payload, declarations) = written?;
    let types_start = payload.len();
    payload.extend_from_slice(&section);
    let directory = bytecode(&PackageDirectory {
        header: header.clone(),
        declarations,
        types: types_start..payload.len(),
        payload: payload.len(),
    })?;
    let mut output = format!("{PACKAGE_FORMAT}\n").into_bytes();
    output.extend_from_slice(&(directory.len() as u64).to_le_bytes());
    output.extend_from_slice(&directory);
    output.extend_from_slice(&payload);
    Ok(output)
}

/// Whether two lowerings of a body agree: in their encoding, and in each
/// register's source location as provenance.
pub fn same_body(left: &MirFunction, right: &MirFunction) -> bool {
    body_key(left) == body_key(right)
}

/// Read a package: its header now, and its directory of
/// modules, declarations, and bodies, which decode on demand, every file
/// it names under the library `root`.
pub fn read_package(bytes: PackageBytes, root: &LibraryRoot) -> Result<Package, PackageError> {
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
    let payload = Payload::new(bytes, start)
        .with_types(&directory.types)
        .map_err(PackageError)?;
    let declarations = Arc::new(
        PackageDeclarations::read(payload.clone(), &directory.declarations, Some(root))
            .map_err(PackageError)?,
    );
    // A unit's lowering reads the package's own declarations, as its
    // build's did: every module of it.
    let scope = PackageScope::new(
        Arc::clone(&declarations),
        vec![true; declarations.modules().len()].into(),
    );
    Ok(Package {
        header: directory.header,
        declarations: Arc::clone(&declarations),
        bodies: PackageBodies {
            payload,
            root: root.clone(),
            decoded: (0..declarations.body_count())
                .map(|_| OnceLock::new())
                .collect(),
            declarations: MirDeclarations::of_package(Arc::clone(&declarations)),
            overloads: Arc::new(ProgramOverloads::importing(Some(scope.clone()))),
            constants: Arc::new(ProgramConstants::importing(Some(scope))),
            units: Mutex::new(HashMap::new()),
            package: declarations,
            finish: None,
            failures: Mutex::new(Vec::new()),
        },
    })
}

/// The functions a unit lowers to, or why it does not lower.
type LoweredUnit = Result<Vec<(String, MirFunction)>, String>;

/// Where every record of a package lies in its payload.
#[derive(serde::Serialize, serde::Deserialize)]
struct PackageDirectory {
    header: PackageHeader,
    /// The declaration directory, which places every declaration record and
    /// every unit.
    declarations: Range<usize>,
    /// The type section, which every record's types refer to.
    types: Range<usize>,
    /// The payload's length.
    payload: usize,
}

impl PackageDirectory {
    /// Refuse a directory whose declaration directory or type section
    /// leaves the payload (the declaration directory's rows and the type
    /// section's offsets are checked as they are read).
    fn validate(&self) -> Result<(), PackageError> {
        for range in [&self.declarations, &self.types] {
            if range.start > range.end || !(..=self.payload).contains(&range.end) {
                return Err(PackageError("a record overruns the payload".to_string()));
            }
        }
        Ok(())
    }
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

/// What [`same_body`] compares of a body.
fn body_key(function: &MirFunction) -> (Vec<u8>, Vec<(u32, String)>) {
    let encoded =
        mojito_types::origin::encoding_packaged_owners(|| bytecode(function)).unwrap_or_default();
    let mut locations: Vec<(u32, String)> = function
        .spans
        .0
        .iter()
        .filter(|(reg, _)| **reg < function.n_regs)
        .map(|(reg, (span, origin))| {
            (
                *reg,
                format!("{:?} {:?} {origin:?}", span.source, span.span),
            )
        })
        .collect();
    locations.sort();
    (encoded, locations)
}
