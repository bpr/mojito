//! The compiled package container, one file per package.
//!
//! It holds the package's checker interface, its one module table, which
//! holds the record its importer's link reads for each of its source
//! modules, one record per library declaration (both
//! `mojito_checked::package`), and each of its verified, ownership-checked
//! function bodies, before drop elaboration. An importer maps the file and
//! decodes its header and interface eagerly; it reads the declaration
//! directory in place, a declaration's record the first time one of its
//! phases looks one of its names up (as upstream's importer resolves a
//! declaration from bytecode on first lookup), a module's link record when
//! its link reaches the module, and a body only when the elaborator first
//! reaches it, inserting that body's drops then. Upstream's precompiled
//! package likewise holds each body as checked, before its lifetime pass
//! inserts destructors, and its importer runs that pass on each body it
//! materializes (`docs/notes/upstream-package-contents.md`).
//!
//! The container is a header followed by records:
//!
//! ```text
//! mojito-package 15
//! <the header's length, 8 bytes little-endian>
//! <the header, as bytecode>
//! <the payload: every record, as bytecode>
//! ```
//!
//! The header names the package and the compiler that built it, and holds
//! the payload range of the interface, of the source-file table every
//! body's locations name, and of the declaration directory
//! (`mojito_checked::package::DeclarationDirectory`). That directory holds
//! every name once, in one string section, and places every module's link
//! record, every declaration's record, and every function's body: one row
//! per function names it once and holds its declaration, its runtime
//! parameter count, its reference parameter mask, and its body's range, as
//! an MLIR function op names its symbol once and holds its region. Every
//! record is postcard bytecode; a link module record and a declaration's
//! checker residue are opaque here, since the linker and the checker that
//! read them sit above MIR. No part of a package is MIR text.

use crate::mir::{MirDeclarations, MirFunction, MirProgram, SpanTable};
use mojito_checked::interface::PackageInterface;
pub use mojito_checked::package::{DeclarationRecords, PackageBytes, PackageDeclarations};
use mojito_checked::package::{LoweredDeclaration, PackagedBody, Payload};
use mojito_common::timing;
use mojito_common::token::SourceSpan;
use mojito_hir::hir::VarId;
use mojito_types::origin::encoding_packaged_owners;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock};

/// The container format this compiler writes and reads.
pub const PACKAGE_FORMAT: &str = "mojito-package 15";

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

/// A read package: its header, its checker interface, and its modules,
/// declarations, and bodies, each decoded on demand.
#[derive(Debug)]
pub struct Package {
    pub header: PackageHeader,
    pub interface: PackageInterface,
    /// Its module table and declaration records.
    pub declarations: Arc<PackageDeclarations>,
    pub bodies: PackageBodies,
}

/// A package's function bodies, each decoded the first time it is asked
/// for, then finished as its importer finishes a body it lowers itself.
#[derive(Debug)]
pub struct PackageBodies {
    payload: Payload,
    /// The source file each body location names, by file id.
    files: Vec<Option<String>>,
    /// The package's declarations, whose function table places each body.
    package: Arc<PackageDeclarations>,
    /// The package's own declarations, which its bodies were lowered
    /// against, resolved through the same lookups an importer's are.
    declarations: MirDeclarations,
    /// Each function table row's body, once decoded.
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
    ///
    /// Its decode, finish, and verify report as the `package.body.*`
    /// `--timings` spans of the thread that first asks for the body.
    pub fn get(&self, name: &str) -> Option<&MirFunction> {
        let row = self.package.body_row(name)?;
        self.decoded
            .get(row)?
            .get_or_init(|| {
                let decoded = {
                    let _decode = timing::span("package.body.decode");
                    self.decode(row)
                };
                let function = match decoded {
                    Ok(function) => function,
                    Err(error) => {
                        self.fail(format!("package body `{name}` does not decode: {error}"));
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

    /// Finish every body with `finish` as it is decoded, and verify it.
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
    /// starts with `prefix`, in name order, decoding nothing.
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

    /// How many bodies have been decoded so far.
    pub fn materialized(&self) -> usize {
        self.decoded
            .iter()
            .filter(|cell| cell.get().is_some())
            .count()
    }

    /// Every body, decoded, in package order.
    pub fn all(&self) -> Vec<(String, MirFunction)> {
        self.package
            .bodies()
            .filter_map(|(name, _)| {
                self.get(name)
                    .map(|function| (name.to_string(), function.clone()))
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

    /// The body of the function at `row` of the function table, its
    /// locations rebuilt against the package's file table.
    fn decode(&self, row: usize) -> Result<MirFunction, String> {
        let range = self
            .package
            .body_range(row)
            .ok_or_else(|| "the package holds no body for it".to_string())?;
        let (mut function, locations): (MirFunction, Vec<Location>) =
            encoding_packaged_owners(|| postcard::from_bytes(self.payload.get(&range)))
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
/// The package holds `program` before drop elaboration, its interface, and
/// its declaration `records`, which hold its module table and every struct
/// and function declaration of `program`.
///
/// Verification runs first, so an error never leaves a partial package.
pub fn write_package(
    header: &PackageHeader,
    interface: &PackageInterface,
    program: &MirProgram,
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
    let interface = record(&bytecode(interface)?);
    let files = files(program);
    let files_range = record(&bytecode(&files.keys().collect::<Vec<_>>())?);
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
            Ok(PackagedBody {
                name: name.clone(),
                n_params: function.n_params,
                ref_params: function.ref_params.clone(),
                range: record(&bytes),
            })
        })
        .collect::<Result<Vec<_>, PackageError>>()?;
    let declarations = records
        .encode(Some(bodies), &mut record)
        .map_err(PackageError)?;
    let directory = bytecode(&PackageDirectory {
        header: header.clone(),
        interface,
        files: files_range,
        declarations,
        payload: payload.len(),
    })?;
    let mut output = format!("{PACKAGE_FORMAT}\n").into_bytes();
    output.extend_from_slice(&(directory.len() as u64).to_le_bytes());
    output.extend_from_slice(&directory);
    output.extend_from_slice(&payload);
    Ok(output)
}

/// Read a package: its header and interface now, and its directory of
/// modules, declarations, and bodies, which decode on demand.
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
    let declarations = Arc::new(
        PackageDeclarations::read(payload.clone(), &directory.declarations)
            .map_err(PackageError)?,
    );
    Ok(Package {
        header: directory.header,
        interface,
        declarations: Arc::clone(&declarations),
        bodies: PackageBodies {
            payload,
            files,
            decoded: (0..declarations.body_count())
                .map(|_| OnceLock::new())
                .collect(),
            declarations: MirDeclarations::of_package(Arc::clone(&declarations)),
            package: declarations,
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
    /// The declaration directory, which places every declaration record and
    /// every body.
    declarations: Range<usize>,
    /// The payload's length.
    payload: usize,
}

impl PackageDirectory {
    /// Refuse a directory whose ranges leave the payload (the declaration
    /// directory's rows are checked as they are read).
    fn validate(&self) -> Result<(), PackageError> {
        for range in [&self.interface, &self.files, &self.declarations] {
            if range.start > range.end || !(..=self.payload).contains(&range.end) {
                return Err(PackageError("a record overruns the payload".to_string()));
            }
        }
        Ok(())
    }
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
