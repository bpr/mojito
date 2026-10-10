//! The compiled package container, one file per package.
//!
//! It holds the package's checker interface, its MIR declarations, the
//! record its importer's link reads for each of its source modules, and
//! each of its verified, ownership-checked function bodies as MIR text,
//! before drop elaboration. An importer reads the interface and the
//! declarations eagerly, a module's record when its link reaches the
//! module, and a body only when the elaborator first reaches it, inserting
//! that body's drops then, as upstream materializes a package's bodies on
//! demand and runs its lifetime pass on each in the importer.
//!
//! The container is line-framed:
//!
//! ```text
//! mojito-package 2
//! package <name>
//! compiler <identity>
//! source <hash>
//! interface <bytes>
//! <the interface record>
//! declarations <bytes>
//! <a MIR artifact whose function list is empty>
//! module <bytes> <canonical source path>
//! <the module's record, as bytecode>
//! …
//! function <bytes> <params> <ref-parameter mask> <name>
//! <the function's MIR record>
//! …
//! end
//! ```
//!
//! Each section is exactly its byte count, followed by a newline. Every
//! section but a module record is UTF-8 text; a module record is opaque
//! here, since the linker that reads it sits above MIR.

use super::parse::package::{self as decode, PackageFiles};
use super::{ArtifactDiagnostic, DisassembleError, write};
use crate::mir::{MirDeclarations, MirFunction, MirProgram};
use mojito_checked::interface::PackageInterface;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

/// The container format this compiler writes and reads.
pub const PACKAGE_FORMAT: &str = "mojito-package 2";

/// Who built a package from what: an importer refuses a package another
/// compiler built, or one built from other sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageHeader {
    /// The package's name (`std`).
    pub package: String,
    /// The identity of the compiler build that wrote it.
    pub compiler: String,
    /// A digest of the source tree it was built from.
    pub source: String,
}

/// A read package: its header, its checker interface, its MIR declarations,
/// its module records, and its bodies, decoded on demand.
#[derive(Debug)]
pub struct Package {
    pub header: PackageHeader,
    pub interface: PackageInterface,
    pub declarations: MirDeclarations,
    /// Each module's encoded record, beside its canonical source path.
    pub modules: Vec<(String, Vec<u8>)>,
    pub bodies: PackageBodies,
}

/// A package's function bodies, each decoded the first time it is asked
/// for, then finished as its importer finishes a body it lowers itself.
#[derive(Debug)]
pub struct PackageBodies {
    bytes: Vec<u8>,
    files: PackageFiles,
    declarations: MirDeclarations,
    entries: Vec<BodyEntry>,
    index: HashMap<String, usize>,
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
    /// has no such function or its text does not decode (a failure
    /// [`Self::failures`] reports).
    pub fn get(&self, name: &str) -> Option<&MirFunction> {
        let position = *self.index.get(name)?;
        self.decoded[position]
            .get_or_init(|| {
                let entry = &self.entries[position];
                let text =
                    std::str::from_utf8(&self.bytes[entry.start..entry.end]).map_err(|error| {
                        vec![ArtifactDiagnostic {
                            span: (0, 0),
                            message: error.to_string(),
                            context: Vec::new(),
                        }]
                    });
                match text.and_then(|text| decode::function(text, &self.files)) {
                    Ok((decoded, function)) if decoded == entry.name => {
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
                    }
                    Ok((decoded, _)) => {
                        self.fail(format!(
                            "package body `{}` decodes as `{decoded}`",
                            entry.name
                        ));
                        None
                    }
                    Err(diagnostics) => {
                        self.fail(format!(
                            "package body `{}` does not decode: {}",
                            entry.name,
                            describe(&diagnostics)
                        ));
                        None
                    }
                }
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
        self.index.contains_key(name)
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

impl From<DisassembleError> for PackageError {
    fn from(error: DisassembleError) -> Self {
        Self(error.to_string())
    }
}

/// Write a package: `program`, its verified, ownership-checked MIR before
/// drop elaboration, its interface, and its `modules`' encoded records.
///
/// Verification runs first, so an error never leaves a partial package.
pub fn write_package(
    header: &PackageHeader,
    interface: &PackageInterface,
    program: &MirProgram,
    modules: &[(String, Vec<u8>)],
) -> Result<Vec<u8>, PackageError> {
    let mut findings = program.invariant_errors.clone();
    findings.extend(crate::mir::verify::verify(program));
    findings.extend(write::schema_findings(program));
    if !findings.is_empty() {
        return Err(PackageError(findings.join("; ")));
    }
    for line in [&header.package, &header.compiler, &header.source] {
        if line.contains('\n') || line.is_empty() {
            return Err(PackageError(format!(
                "header field `{line}` is not one line"
            )));
        }
    }
    let mut output = format!(
        "{PACKAGE_FORMAT}\npackage {}\ncompiler {}\nsource {}\n",
        header.package, header.compiler, header.source
    )
    .into_bytes();
    let mut section = |label: String, bytes: &[u8]| {
        output.extend_from_slice(label.as_bytes());
        output.push(b'\n');
        output.extend_from_slice(bytes);
        output.push(b'\n');
    };
    let interface_text = write::package_interface(interface);
    section(
        format!("interface {}", interface_text.len()),
        interface_text.as_bytes(),
    );
    let (declarations, functions) = write::package_parts(program);
    section(
        format!("declarations {}", declarations.len()),
        declarations.as_bytes(),
    );
    for (path, record) in modules {
        if path.contains('\n') {
            return Err(PackageError(format!("module path `{path}` spans lines")));
        }
        section(format!("module {} {path}", record.len()), record);
    }
    for ((name, function), text) in program.functions.iter().zip(&functions) {
        if name.contains('\n') {
            return Err(PackageError(format!("function name `{name}` spans lines")));
        }
        let mask: String = function
            .ref_params
            .iter()
            .map(|reference| if *reference { '1' } else { '0' })
            .collect();
        section(
            format!(
                "function {} {} {} {name}",
                text.len(),
                function.n_params,
                if mask.is_empty() { "-" } else { &mask }
            ),
            text.as_bytes(),
        );
    }
    output.extend_from_slice(b"end\n");
    Ok(output)
}

/// Read a package: its header, interface, declarations, and module
/// records now, and an index of its bodies, which decode on demand.
pub fn read_package(bytes: Vec<u8>) -> Result<Package, PackageError> {
    let mut reader = Reader {
        bytes: &bytes,
        pos: 0,
    };
    if reader.line()? != PACKAGE_FORMAT {
        return Err(PackageError(format!("expected `{PACKAGE_FORMAT}`")));
    }
    let header = PackageHeader {
        package: reader.labelled("package")?.to_string(),
        compiler: reader.labelled("compiler")?.to_string(),
        source: reader.labelled("source")?.to_string(),
    };
    let interface_text = reader.text_section("interface")?;
    let decoding = mojito_common::timing::span("package.interface");
    let interface = decode::interface(interface_text)
        .map_err(|diagnostics| PackageError(describe(&diagnostics)))?;
    drop(decoding);
    let declarations_text = reader.text_section("declarations")?;
    let decoding = mojito_common::timing::span("package.declarations");
    let (declarations, files) = decode::declarations(declarations_text)
        .map_err(|diagnostics| PackageError(describe(&diagnostics)))?;
    drop(decoding);
    let mut modules = Vec::new();
    let mut entries = Vec::new();
    loop {
        let line = reader.line()?;
        if line == "end" {
            break;
        }
        if let Some(rest) = line.strip_prefix("module ") {
            let Some((length, path)) = rest.split_once(' ') else {
                return Err(PackageError(format!("malformed module line `{line}`")));
            };
            let length = number(length)?;
            let start = reader.pos;
            reader.skip(length)?;
            modules.push((path.to_string(), bytes[start..start + length].to_vec()));
            continue;
        }
        let mut fields = line.splitn(5, ' ');
        let (Some("function"), Some(length), Some(params), Some(mask), Some(name)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            return Err(PackageError(format!("malformed function line `{line}`")));
        };
        let length = number(length)?;
        let n_params = number(params)?;
        let ref_params = if mask == "-" {
            Vec::new()
        } else {
            mask.chars().map(|bit| bit == '1').collect()
        };
        let start = reader.pos;
        reader.skip(length)?;
        entries.push(BodyEntry {
            name: name.to_string(),
            n_params,
            ref_params,
            start,
            end: start + length,
        });
    }
    if reader.pos != bytes.len() {
        return Err(PackageError("trailing text after `end`".to_string()));
    }
    let index = entries
        .iter()
        .enumerate()
        .map(|(position, entry)| (entry.name.clone(), position))
        .collect::<HashMap<_, _>>();
    if index.len() != entries.len() {
        return Err(PackageError("a function name repeats".to_string()));
    }
    let decoded = entries.iter().map(|_| OnceLock::new()).collect();
    let package_declarations = declarations.clone();
    Ok(Package {
        header,
        interface,
        declarations,
        modules,
        bodies: PackageBodies {
            bytes,
            files,
            declarations: package_declarations,
            entries,
            index,
            decoded,
            finish: None,
            failures: Mutex::new(Vec::new()),
        },
    })
}

#[derive(Debug)]
struct BodyEntry {
    name: String,
    n_params: usize,
    ref_params: Vec<bool>,
    start: usize,
    end: usize,
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn line(&mut self) -> Result<&'a str, PackageError> {
        let rest = &self.bytes[self.pos..];
        let end = rest
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or_else(|| PackageError("truncated package".to_string()))?;
        self.pos += end + 1;
        std::str::from_utf8(&rest[..end])
            .map_err(|_| PackageError("a section label is not UTF-8".to_string()))
    }

    fn labelled(&mut self, label: &str) -> Result<&'a str, PackageError> {
        self.line()?
            .strip_prefix(label)
            .and_then(|rest| rest.strip_prefix(' '))
            .ok_or_else(|| PackageError(format!("expected `{label}`")))
    }

    fn text_section(&mut self, label: &str) -> Result<&'a str, PackageError> {
        let length = number(self.labelled(label)?)?;
        let start = self.pos;
        self.skip(length)?;
        std::str::from_utf8(&self.bytes[start..start + length])
            .map_err(|_| PackageError(format!("the `{label}` section is not UTF-8")))
    }

    /// Step over `length` bytes of section text and its closing newline.
    fn skip(&mut self, length: usize) -> Result<(), PackageError> {
        let end = self.pos + length;
        if self.bytes.get(end) != Some(&b'\n') {
            return Err(PackageError("a section overruns its length".to_string()));
        }
        self.pos = end + 1;
        Ok(())
    }
}

fn number(text: &str) -> Result<usize, PackageError> {
    text.parse()
        .map_err(|_| PackageError(format!("expected a count, found `{text}`")))
}

fn describe(diagnostics: &[ArtifactDiagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.clone())
        .collect::<Vec<_>>()
        .join("; ")
}
