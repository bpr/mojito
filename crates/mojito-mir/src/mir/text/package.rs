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
//! unit then (HIR region tree, MIR) and inserts the reached body's drops. Each
//! unit's lowering and each body's finish read the package's declarations
//! through a view of their own, so a record one of them reaches that does
//! not decode is that body's verdict, never another's. Upstream's
//! precompiled package likewise holds each body's IR as its parser wrote it
//! and its semantic checks validated it, and its importer lowers its
//! semantic control flow and runs its lifetime pass on each body it
//! materializes (`docs/notes/upstream-package-contents.md`).
//!
//! The container is a header followed by records:
//!
//! ```text
//! mojito-package 85
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

use crate::mir::{MirDeclarations, MirFunction, MirFunctionDeclaration, MirProgram};
pub use mojito_checked::package::{
    DeclarationRecords, LibraryRoot, PackageBytes, PackageDeclarations,
};
use mojito_checked::package::{
    LoweredDeclaration, PackageScope, PackagedBody, Payload, ProgramConstants, ProgramOverloads,
    ReachedFailures,
};
use mojito_checked::unit::CheckedUnit;
use mojito_common::timing;
use mojito_types::type_section::encoding_types;
use std::collections::HashMap;
use std::fmt;
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock};

/// The container format this compiler writes and reads.
pub const PACKAGE_FORMAT: &str = "mojito-package 103";

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
    /// Every module of the package, which a unit's lowering reaches, as
    /// its build's did.
    modules: Arc<[bool]>,
    /// Each unit lowered so far, by where its record starts.
    units: Mutex<HashMap<usize, Arc<OnceLock<LoweredUnit>>>>,
    /// Each function table row's body, once finished.
    decoded: Vec<OnceLock<FinishedBody>>,
    /// Whether each row's function lies on a cycle of the calls
    /// [`Self::on_cycle`] was asked about, once answered.
    cycles: Mutex<HashMap<usize, bool>>,
    finish: Option<BodyFinish>,
}

/// What an importer does to a lowered body before using it: insert its
/// drops and run the pre-elaboration pipeline.
///
/// It is given the body's name, the package's declarations, and the
/// package's bodies, whose finished callees a pass may read, and pushes the
/// findings of its own verification to the vector. A plain function, since
/// the phases it runs sit above MIR.
pub type BodyFinish =
    fn(&str, MirFunction, &MirDeclarations, &PackageBodies, &mut Vec<String>) -> MirFunction;

impl PackageBodies {
    /// The body of `name`, lowered and finished on first use, whether or
    /// not it verified; `None` when the package has no such function or its
    /// unit does not lower (a failure [`Self::findings`] reports).
    ///
    /// Its unit's decode and lowering, and its finish and verify, report as
    /// the `package.unit.*` and `package.body.*` `--timings` spans of the
    /// thread that first asks for it.
    pub fn get(&self, name: &str) -> Option<&MirFunction> {
        self.finished(name)?.function.as_ref()
    }

    /// What made the body of `name` unusable, finishing it on first use:
    /// why each package declaration its unit's lowering, its finish, or its
    /// verify reached does not decode, then why its unit does not lower, or
    /// its finish's and the verifier's findings. Empty when it verified or
    /// the package has no such function.
    pub fn findings(&self, name: &str) -> &[String] {
        self.finished(name)
            .map_or(&[], |finished| finished.findings.as_slice())
    }

    /// Why each package declaration the body of `name` reached does not
    /// decode, finishing it on first use: its unit's lowering's, its
    /// finish's, and its verify's, in package order.
    pub fn reached(&self, name: &str) -> &[String] {
        self.finished(name)
            .map_or(&[], |finished| finished.reached.as_slice())
    }

    /// Whether this thread is finishing the body of `name`: a pass reading
    /// it from inside that finish reached it again around a cycle, one a
    /// drop elaboration closed, which the lowered bodies
    /// [`Self::on_cycle`] walks do not show.
    pub fn finishing_body(&self, name: &str) -> bool {
        self.package.body_row(name).is_some_and(|row| {
            FINISHING.with(|finishing| finishing.borrow().contains(&(self.identity(), row)))
        })
    }

    /// The body of `name` as its unit lowers to it, before it is finished.
    ///
    /// # Errors
    ///
    /// When the package has no such function, or its unit does not decode
    /// or lower, or lowers to no such function, or its lowering reached a
    /// package declaration that does not decode.
    pub fn lowered(&self, name: &str) -> Result<MirFunction, String> {
        let (function, reached) = self.unit_body(name);
        if reached.is_empty() {
            function
        } else {
            Err(reached.join("; "))
        }
    }

    /// Finish every body with `finish` as it is lowered, and verify it.
    #[must_use]
    pub fn finishing(mut self, finish: BodyFinish) -> Self {
        self.finish = Some(finish);
        self
    }

    /// Whether the function `name` reaches itself through the calls
    /// `successors` lists of each body by its name, as its unit lowers to it, and so
    /// lies on a cycle of them; memoized by row.
    ///
    /// It reads lowered bodies, never finished ones, so the answer is the
    /// same whichever body finishes first, and a finishing hook may ask it
    /// of a callee before finishing that callee: a callee on no cycle never
    /// reaches the body being finished.
    pub fn on_cycle(
        &self,
        name: &str,
        successors: &dyn Fn(&str, &MirFunction) -> Vec<String>,
    ) -> bool {
        let Some(row) = self.package.body_row(name) else {
            return false;
        };
        if let Some(&cyclic) = self
            .cycles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&row)
        {
            return cyclic;
        }
        let mut seen = std::collections::HashSet::new();
        let mut pending = vec![name.to_string()];
        let mut cyclic = false;
        while let Some(next) = pending.pop() {
            let Ok(function) = self.lowered(&next) else {
                continue;
            };
            for callee in successors(&next, &function) {
                if callee == name {
                    cyclic = true;
                    pending.clear();
                    break;
                }
                if self.contains(&callee) && seen.insert(callee.clone()) {
                    pending.push(callee);
                }
            }
        }
        self.cycles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(row, cyclic);
        cyclic
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

    /// The findings of every body finished so far, in this process, in
    /// package order: not one compilation's verdict, since a package is
    /// shared by every compilation that imports it.
    pub fn failures(&self) -> Vec<String> {
        self.decoded
            .iter()
            .filter_map(OnceLock::get)
            .filter(|finished| !finished.findings.is_empty())
            .map(|finished| finished.findings.join("; "))
            .collect()
    }

    /// The finished body of `name`, finishing it on first use.
    fn finished(&self, name: &str) -> Option<&FinishedBody> {
        let row = self.package.body_row(name)?;
        Some(self.decoded.get(row)?.get_or_init(|| {
            let key = (self.identity(), row);
            FINISHING.with(|finishing| finishing.borrow_mut().push(key));
            let finished = self.finish_body(name);
            FINISHING.with(|finishing| finishing.borrow_mut().retain(|held| *held != key));
            finished
        }))
    }

    /// This package's address, which tells its rows from another's in
    /// [`FINISHING`].
    fn identity(&self) -> usize {
        std::ptr::from_ref(self) as usize
    }

    /// Lower the body of `name`, finish it, and verify it.
    fn finish_body(&self, name: &str) -> FinishedBody {
        let (function, mut reached) = self.unit_body(name);
        let function = match function {
            Ok(function) => function,
            Err(error) => {
                let findings =
                    std::iter::once(format!("package body `{name}` does not lower: {error}"))
                        .chain(reached.iter().cloned())
                        .collect();
                return FinishedBody {
                    function: None,
                    reached,
                    findings,
                };
            }
        };
        let mut findings = Vec::new();
        let function = match self.finish {
            Some(finish) => {
                let view = self.view();
                let function = {
                    let _finish = timing::span("package.body.finish");
                    finish(name, function, &view.declarations, self, &mut findings)
                };
                let _verify = timing::span("package.body.verify");
                findings.extend(crate::mir::verify::verify_function_in(
                    name,
                    &function,
                    &view.declarations,
                ));
                for message in view.reached.messages(&self.package) {
                    if !reached.contains(&message) {
                        reached.push(message);
                    }
                }
                function
            }
            None => function,
        };
        FinishedBody {
            function: Some(function),
            findings: reached.iter().cloned().chain(findings).collect(),
            reached,
        }
    }

    /// The body of `name` as its unit lowers to it, or why it does not,
    /// and why each package declaration the unit's lowering reached does
    /// not decode.
    fn unit_body(&self, name: &str) -> (Result<MirFunction, String>, Vec<String>) {
        let Some(range) = self
            .package
            .body_row(name)
            .map(|row| self.package.body_range(row))
        else {
            return (
                Err("the package has no such function".to_string()),
                Vec::new(),
            );
        };
        let Some(range) = range else {
            return (
                Err("the package holds no unit for it".to_string()),
                Vec::new(),
            );
        };
        let cell = Arc::clone(
            self.units
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(range.start)
                .or_default(),
        );
        let unit = cell.get_or_init(|| self.lower(&range));
        let function = unit
            .functions
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|functions| {
                functions
                    .iter()
                    .find(|(lowered, _)| lowered == name)
                    .map(|(_, function)| function.clone())
                    .ok_or_else(|| "its unit does not lower to it".to_string())
            });
        (function, unit.reached.clone())
    }

    /// Decode the unit at `range` of the payload and lower it, through a
    /// view of the package of its own.
    fn lower(&self, range: &Range<usize>) -> LoweredUnit {
        let unit = {
            let _decode = timing::span("package.unit.decode");
            self.payload
                .decoding(|| CheckedUnit::decode(self.payload.get(range), &self.root))
        };
        let unit = match unit {
            Ok(unit) => unit,
            Err(error) => {
                return LoweredUnit {
                    functions: Err(error),
                    reached: Vec::new(),
                };
            }
        };
        let _lower = timing::span("package.unit.lower");
        let view = self.view();
        let program = unit.program(
            Arc::new(ProgramOverloads::importing(Some(view.scope.clone()))),
            Arc::new(ProgramConstants::importing(Some(view.scope.clone()))),
            &|name| {
                view.declarations
                    .structure(name)
                    .map(|declaration| declaration.param_decls.clone())
            },
        );
        let functions = crate::mir::lower_package_unit(&program, &view.declarations);
        LoweredUnit {
            functions,
            reached: view.reached.messages(&self.package),
        }
    }

    /// A fresh view of the package's own declarations, for one unit's
    /// lowering or one body's finish: every lookup through it notes a
    /// record that does not decode in its own set.
    fn view(&self) -> PackageView {
        let reached = ReachedFailures::default();
        PackageView {
            declarations: MirDeclarations::of_package_noting(
                Arc::clone(&self.package),
                reached.clone(),
            ),
            scope: PackageScope::new(
                Arc::clone(&self.package),
                Arc::clone(&self.modules),
                reached.clone(),
            ),
            reached,
        }
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

/// A function's body as [`same_body`] compares it, and its declaration.
///
/// Two functions that share one may stand for each other, once each
/// spells its own name alike. It is hashable, so a pass can group
/// functions by it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FunctionIdentity {
    body: Vec<u8>,
    locations: Vec<(u32, String)>,
    declaration: Option<Vec<u8>>,
}

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

/// The identity of `function`, declared by `declaration` when it has one,
/// or `None` when either does not encode outside its compilation.
pub fn function_identity(
    function: &MirFunction,
    declaration: Option<&MirFunctionDeclaration>,
) -> Option<FunctionIdentity> {
    let encode = || {
        let body = bytecode(function)?;
        let declaration = declaration.map(bytecode).transpose()?;
        Ok::<_, PackageError>((body, declaration))
    };
    let (body, declaration) = mojito_types::origin::encoding_packaged_owners(encode).ok()?;
    Some(FunctionIdentity {
        body,
        locations: body_locations(function),
        declaration,
    })
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
    let modules = vec![true; declarations.modules().len()].into();
    Ok(Package {
        header: directory.header,
        declarations: Arc::clone(&declarations),
        bodies: PackageBodies {
            payload,
            root: root.clone(),
            decoded: (0..declarations.body_count())
                .map(|_| OnceLock::new())
                .collect(),
            modules,
            units: Mutex::new(HashMap::new()),
            cycles: Mutex::new(HashMap::new()),
            package: declarations,
            finish: None,
        },
    })
}

thread_local! {
    /// The package bodies, by package and row, this thread is finishing.
    static FINISHING: std::cell::RefCell<Vec<(usize, usize)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// A package body as its importer finished it, and its verdict.
#[derive(Debug)]
struct FinishedBody {
    /// `None` when its unit does not lower.
    function: Option<MirFunction>,
    /// Why each package declaration its unit's lowering, its finish, or its
    /// verify reached does not decode, in the order they were reached.
    reached: Vec<String>,
    /// Those, then why it does not lower, or its finish's and the
    /// verifier's findings.
    findings: Vec<String>,
}

/// A unit as its importer lowered it.
#[derive(Debug)]
struct LoweredUnit {
    /// The functions it lowers to, or why it does not lower.
    functions: Result<Vec<(String, MirFunction)>, String>,
    /// Why each package declaration its lowering reached does not decode,
    /// in package order.
    reached: Vec<String>,
}

/// One view of a package's own declarations, its MIR lookups and its
/// checked ones noting into one set.
struct PackageView {
    declarations: MirDeclarations,
    scope: PackageScope,
    reached: ReachedFailures,
}

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
    (encoded, body_locations(function))
}

/// Each register's source location in `function`, sorted by register.
fn body_locations(function: &MirFunction) -> Vec<(u32, String)> {
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
    locations
}
