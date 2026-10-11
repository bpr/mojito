//! The bundled standard library as a compiled package.
//!
//! The library's bodies are checked, lowered, and ownership-verified once
//! per compiler build, and kept as checked in one `std` package file
//! (`mir::text::package`), one record per lowering unit. Every compilation
//! after that imports it instead of reading the library again, as upstream
//! imports its precompiled `std.mojoc`: its link reads each library
//! module's record (`module::package`), and it lowers a body's unit and
//! inserts the body's drops when its elaboration first reaches it, as
//! upstream's importer lowers and runs its lifetime pass on each body it
//! materializes from a package that holds each body as checked.
//!
//! Like upstream's, the package names each library file relative to the
//! library's root, so it reads the same wherever it is installed, and is
//! keyed by the build's identity alone, which covers the library's source.
//! An installed compiler reads the package its bundle ships at
//! `<prefix>/lib/mojito/std.mojoc`, which `mojito precompile` writes, as
//! upstream's toolchain ships `lib/mojo/std.mojoc` built by `mojo
//! precompile`, and refuses to compile without it. A development build
//! writes the package to `target/mojito-cache` the first time it needs it.

use crate::checked_package::PackagedModule;
use crate::compiler::{Compiler, CompilerError};
use crate::mir::MirProgram;
use crate::mir::text::package::{
    LibraryRoot, PackageBodies, PackageBytes, PackageDeclarations, PackageHeader, read_package,
    same_body, write_package,
};
use crate::module::{
    LinkOptions, ModuleError, bundled_root, installed_prefix, link_source_recording,
};
use crate::module_package::{ModuleIdentity, PackageModules};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// An imported package, read once per process.
///
/// It holds the module records its importer's link reads, the declarations
/// every phase of its importer reads (the check through its index of them),
/// and its bodies.
#[derive(Debug)]
pub struct ImportedPackage {
    pub modules: PackageModules,
    /// The check's index of its declarations, each declaration's residue
    /// decoded on first lookup by each compilation.
    pub checked: Arc<crate::checker::DeclarationInterfaces>,
    pub declarations: Arc<PackageDeclarations>,
    pub bodies: PackageBodies,
}

/// Why an installed compiler neither compiles nor packages the library from
/// source: its bundle ships the package alone, as upstream's toolchain
/// ships `std.mojoc` and no `std` source.
pub const NO_LIBRARY_SOURCE: &str =
    "an installed compiler has no standard library source; it imports its package";

/// The identity of this compiler build, which a package it reads must
/// carry.
pub const BUILD_ID: &str = env!("MOJITO_BUILD_ID");

/// The bundled library's package for this build, read once per process.
///
/// An installed compiler maps the package its bundle ships
/// (`<prefix>/lib/mojito/std.mojoc`). A development build's comes from
/// the cache when this build already wrote it, else it is built and cached
/// now.
///
/// `Ok(None)` when there is no bundled library, or a development build
/// failed to package it (then the library compiles from source, after one
/// warning).
///
/// # Errors
///
/// An installed compiler whose package is missing, unreadable, or another
/// build's, as upstream's refuses a missing or mismatched `std.mojoc`.
pub fn bundled_std_package() -> Result<Option<Arc<ImportedPackage>>, ModuleError> {
    static PACKAGE: OnceLock<BundledPackage> = OnceLock::new();
    PACKAGE
        .get_or_init(|| {
            let Some(root) = bundled_root().map(|root| root.join("stdlib")) else {
                return Ok(None);
            };
            let header = std_header();
            if let Some(prefix) = installed_prefix() {
                let path = installed_package(&prefix);
                return std::fs::File::open(&path)
                    .and_then(|file| PackageBytes::map(&file))
                    .map_err(|error| error.to_string())
                    .and_then(|bytes| import(bytes, &header, &LibraryRoot::new(&root)))
                    .map(|package| Some(Arc::new(package)))
                    .map_err(|reason| (path, reason));
            }
            match load_or_build(&root, &header) {
                Ok(package) => Ok(Some(Arc::new(package))),
                Err(error) => {
                    eprintln!(
                        "warning: the bundled standard library could not be packaged ({error}); \
                         it is compiled from source"
                    );
                    Ok(None)
                }
            }
        })
        .clone()
        .map_err(|(path, reason)| ModuleError::InstalledPackage { path, reason })
}

/// Write the package of the library directory `directory` to `output`, as
/// `mojo precompile <directory> -o <output>` writes upstream's.
///
/// Only a development build's bundled library precompiles: `directory`
/// must be its `std`, and `output` must name a `std.mojoc` or `std.mojopkg`, since the file's
/// stem is the package's name.
///
/// # Errors
///
/// An installed compiler, which has no library source; any other directory
/// or output name, a library that does not package, or an output that
/// cannot be written.
pub fn precompile(directory: &Path, output: &Path) -> Result<(), String> {
    const UNSUPPORTED: &str =
        "precompiling a package other than the bundled `std` is not supported";
    if installed_prefix().is_some() {
        return Err(NO_LIBRARY_SOURCE.to_string());
    }
    let root = bundled_root()
        .map(|root| root.join("stdlib"))
        .ok_or("there is no bundled standard library")?;
    let same = |left: &Path, right: &Path| {
        left.canonicalize()
            .is_ok_and(|left| right.canonicalize().is_ok_and(|right| left == right))
    };
    if !same(directory, &root.join("std")) {
        return Err(format!("{UNSUPPORTED}: `{}`", directory.display()));
    }
    let extension = output.extension().and_then(|extension| extension.to_str());
    if !matches!(extension, Some("mojoc" | "mojopkg")) {
        return Err(format!(
            "the output `{}` must end in `.mojoc` or `.mojopkg`",
            output.display()
        ));
    }
    if output.file_stem().and_then(|stem| stem.to_str()) != Some("std") {
        return Err(format!(
            "{UNSUPPORTED}: the output `{}` names another package",
            output.display()
        ));
    }
    let bytes = build(&root, &std_header()).map_err(|error| error.to_string())?;
    let partial = output.with_extension(format!("{}.partial", std::process::id()));
    std::fs::write(&partial, bytes)
        .and_then(|()| std::fs::rename(&partial, output))
        .map_err(|error| {
            let _ = std::fs::remove_file(&partial);
            format!("cannot write `{}`: {error}", output.display())
        })
}

/// Where the installation at `prefix` ships its standard library's
/// package.
pub fn installed_package(prefix: &Path) -> PathBuf {
    prefix.join("lib").join("mojito").join("std.mojoc")
}

/// The bundled library's package as [`bundled_std_package`] keeps it: an
/// installed package's failure as its file and the reason.
type BundledPackage = Result<Option<Arc<ImportedPackage>>, (PathBuf, String)>;

/// The header of this build's `std` package.
fn std_header() -> PackageHeader {
    PackageHeader {
        package: "std".to_string(),
        compiler: BUILD_ID.to_string(),
    }
}

/// Map the cached package `header` names, or build and cache it. One
/// process builds while any other that needs the same package waits for
/// it, under a lock file a crashed builder leaves stale after a while.
fn load_or_build(root: &Path, header: &PackageHeader) -> Result<ImportedPackage, String> {
    const STALE: std::time::Duration = std::time::Duration::from_mins(5);
    let library = LibraryRoot::new(root);
    let cache = cache_dir();
    let stem = format!("std-{}", header.compiler);
    let path = cache.join(format!("{stem}.mojoc"));
    let lock = cache.join(format!(".{stem}.lock"));
    loop {
        let map = crate::timing::span("package.map");
        let bytes = std::fs::File::open(&path).and_then(|file| PackageBytes::map(&file));
        drop(map);
        if let Ok(bytes) = bytes
            && let Ok(package) = import(bytes, header, &library)
        {
            return Ok(package);
        }
        if std::fs::create_dir_all(&cache).is_err() {
            break;
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
        {
            Ok(_) => {
                let built = build(root, header).map_err(|error| error.to_string());
                if let Ok(bytes) = &built {
                    let partial = cache.join(format!(".{stem}.{}.partial", std::process::id()));
                    if std::fs::write(&partial, bytes).is_ok()
                        && std::fs::rename(&partial, &path).is_err()
                    {
                        let _ = std::fs::remove_file(&partial);
                    }
                    prune(&cache, &path);
                }
                let _ = std::fs::remove_file(&lock);
                return import(PackageBytes::Owned(built?), header, &library);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let stale = std::fs::metadata(&lock)
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .is_some_and(|age| age > STALE);
                if stale {
                    let _ = std::fs::remove_file(&lock);
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
            Err(_) => break,
        }
    }
    // No usable cache: build for this process alone.
    import(
        PackageBytes::Owned(build(root, header).map_err(|error| error.to_string())?),
        header,
        &library,
    )
}

/// Read `bytes` as the package `header` names, its library under `root`.
fn import(
    bytes: PackageBytes,
    header: &PackageHeader,
    root: &LibraryRoot,
) -> Result<ImportedPackage, String> {
    let package = read_package(bytes, root).map_err(|error| error.to_string())?;
    if package.header != *header {
        return Err(format!(
            "the package was built by compiler `{}`, not this one",
            package.header.compiler
        ));
    }
    // The link records are copied out of the mapping, since the linker that
    // reads them sits below the package's declarations.
    let declarations = &package.declarations;
    let modules = declarations
        .modules()
        .enumerate()
        .map(|(position, module)| {
            let identity = ModuleIdentity {
                name: module.name.to_string(),
                canonical: module.canonical.to_string(),
                path: module.path.to_string(),
            };
            let bytes = declarations.link_record(position).unwrap_or_default();
            (identity, bytes.to_vec())
        });
    Ok(ImportedPackage {
        modules: PackageModules::new(&root.stamped, modules),
        checked: Arc::new(crate::checker::DeclarationInterfaces::new(Arc::clone(
            &package.declarations,
        ))),
        declarations: package.declarations,
        bodies: package
            .bodies
            .finishing(crate::analysis::elaborate_drops_function),
    })
}

/// Build the package of the library under `root`: compile a program that
/// imports every one of its modules, keep each module's link record, its
/// checked effect summaries, the MIR declarations of its declarations, and
/// each lowering unit as the check left it, and write them, every file
/// named relative to `root`.
///
/// The build refuses a package that names `root` itself anywhere, so that
/// it reads the same wherever it is installed.
fn build(root: &Path, header: &PackageHeader) -> Result<Vec<u8>, CompilerError> {
    let library = LibraryRoot::new(root);
    let modules = module_paths(&root.join("std"), "std");
    let mut source = String::new();
    for module in &modules {
        source.push_str("import ");
        source.push_str(module);
        source.push('\n');
    }
    // The entry sits where no `std` resolves beside it, so every import
    // resolves through the bundled root, as an importer's does.
    let entry = root.join(".package").join("std.mojo");
    let (linked, records) = link_source_recording(&source, &entry, LinkOptions::default())
        .map_err(CompilerError::Module)?;
    let (compiled, exports) = Compiler::default().compile_for_package(&linked)?;
    // A declaration of a module the link recorded no row for is refused
    // when the records encode.
    let modules = records
        .into_iter()
        .map(|(identity, record)| {
            let link = record
                .encode()
                .map_err(|error| format!("module `{}`: {error}", identity.name))?;
            Ok(PackagedModule {
                name: identity.name,
                canonical: identity.canonical,
                path: identity.path,
                link,
            })
        })
        .collect::<Result<Vec<_>, String>>()
        .map_err(|error| CompilerError::Verify(vec![error]))?;
    let lowered = compiled.mir();
    let mut program = MirProgram {
        functions: Vec::new(),
        declarations: lowered.declarations.clone(),
        invariant_errors: lowered.invariant_errors.clone(),
    };
    for (name, function) in &lowered.functions {
        if name == "__toplevel__" {
            // Mojo runs no module initializer; the library declares none.
            if function.blocks.iter().any(|block| !block.instrs.is_empty()) {
                return Err(CompilerError::Verify(vec![
                    "the standard library initializes a module at run time".to_string(),
                ]));
            }
            continue;
        }
        if !lifted_name_is_stable(name, lowered) {
            return Err(CompilerError::Verify(vec![format!(
                "the standard library lifts `{name}` from no function it packages"
            )]));
        }
        program.functions.push((name.clone(), function.clone()));
    }
    let units = packaged_units(&compiled, &exports.facts);
    // The checker encodes its residues inside the package's type encoding,
    // so they refer to the package's type section as its other records do.
    let records = || {
        exports.interfaces.records(
            modules,
            program.declarations.structs.clone(),
            program.declarations.functions.clone(),
            program.declarations.traits.clone(),
        )
    };
    let bytes = write_package(header, &library, &program, &units, records)
        .map_err(|error| CompilerError::Verify(vec![error.to_string()]))?;
    refuse_install_paths(&bytes, &library)?;
    refuse_unequal_units(&bytes, &program, &library)?;
    Ok(bytes)
}

/// Refuse the package `bytes` if they name the library `root` it was built
/// from, in either its stamped or its canonical spelling.
fn refuse_install_paths(bytes: &[u8], root: &LibraryRoot) -> Result<(), CompilerError> {
    let named = [&root.stamped, &root.canonical]
        .into_iter()
        .find(|spelling| {
            bytes
                .windows(spelling.len())
                .any(|window| window == spelling.as_bytes())
        });
    named.map_or(Ok(()), |spelling| {
        Err(CompilerError::Verify(vec![format!(
            "the package names its library root `{spelling}`, so it reads only where it was built"
        )]))
    })
}

/// Each lowering unit of the library's checked program, with its share of
/// the check's `facts` and the names of the functions it places: every
/// function the library lowers it to, unless an earlier unit lowers to the
/// same one.
fn packaged_units(
    compiled: &crate::compiler::CompiledProgram,
    facts: &crate::checked::DiscoveryResult,
) -> Vec<(crate::checked_unit::CheckedUnit, Vec<String>)> {
    let checked = compiled.checked();
    let (lowered, ranges) = crate::mir::lower_checked_program_units(checked);
    let mut placed = std::collections::HashSet::new();
    facts
        .units(&checked.lowering_units())
        .into_iter()
        .zip(ranges)
        .map(|(unit, range)| {
            let names = lowered.functions[range]
                .iter()
                .map(|(name, _)| name)
                .filter(|name| placed.insert(name.as_str()))
                .cloned()
                .collect();
            (unit, names)
        })
        .collect()
}

/// Refuse the package `bytes` unless each of its units, lowered alone as
/// an importer lowers it, gives every function it places exactly as the
/// library's `program` holds it, so importing the package changes no body.
fn refuse_unequal_units(
    bytes: &[u8],
    program: &MirProgram,
    root: &LibraryRoot,
) -> Result<(), CompilerError> {
    let package = read_package(PackageBytes::Owned(bytes.to_vec()), root)
        .map_err(|error| CompilerError::Verify(vec![error.to_string()]))?;
    let findings: Vec<String> = program
        .functions
        .iter()
        .filter_map(|(name, function)| match package.bodies.lowered(name) {
            Err(error) => Some(format!("the unit of `{name}` does not lower: {error}")),
            Ok(lowered) if !same_body(&lowered, function) => Some(format!(
                "the unit of `{name}` lowers to another body than the library's"
            )),
            Ok(_) => None,
        })
        .collect();
    if findings.is_empty() {
        Ok(())
    } else {
        Err(CompilerError::Verify(findings))
    }
}

/// Whether `name` stays the package's own in every importer.
///
/// A lifted function's name ([`crate::symbol::lifted_owner`]) says whose it
/// is: a module constant's (`$comptime$<constant>$module`), a parameter
/// default's (`$default$<owner>$<parameter>`), or a function's, numbered
/// within its body by MIR (`$comptime$<function>$<k>`) or by the check
/// (`$comptime$<function>$c<k>`). A numbered one is the package's own only
/// while its owner is a function the package holds, never the module
/// initializer.
fn lifted_name_is_stable(name: &str, program: &MirProgram) -> bool {
    use crate::symbol::LiftedOwner;
    match crate::symbol::lifted_owner(name) {
        None | Some(LiftedOwner::Constant(_) | LiftedOwner::Default { .. }) => true,
        Some(LiftedOwner::Numbered(owner)) => {
            owner != "__toplevel__"
                && program
                    .functions
                    .iter()
                    .any(|(function, _)| function == owner)
        }
    }
}

/// The dotted path of every module under `directory`, which is the module
/// `prefix`, sorted.
fn module_paths(directory: &Path, prefix: &str) -> BTreeSet<String> {
    let mut modules = BTreeSet::new();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return modules;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            modules.extend(module_paths(&path, &format!("{prefix}.{name}")));
        } else if name == "__init__.mojo" {
            modules.insert(prefix.to_string());
        } else if let Some(stem) = name.strip_suffix(".mojo") {
            modules.insert(format!("{prefix}.{stem}"));
        }
    }
    modules
}

/// Where a development build caches its package: `MOJITO_CACHE_DIR`, else
/// the development tree's `target/mojito-cache`, as upstream's build
/// system builds its `std.mojoc` into its build tree.
fn cache_dir() -> PathBuf {
    std::env::var_os("MOJITO_CACHE_DIR").map_or_else(
        || {
            bundled_root()
                .unwrap_or_default()
                .join("target")
                .join("mojito-cache")
        },
        PathBuf::from,
    )
}

/// Remove cached packages other builds wrote, keeping `current` and the
/// three most recent others an alternating build may still read.
fn prune(cache: &Path, current: &Path) {
    let Ok(entries) = std::fs::read_dir(cache) else {
        return;
    };
    let mut others: Vec<_> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path != current
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("std-"))
        })
        .filter_map(|path| Some((std::fs::metadata(&path).ok()?.modified().ok()?, path)))
        .collect();
    others.sort();
    others.reverse();
    for (_, path) in others.into_iter().skip(3) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::MirDeclarations;

    /// A package `header` names, of the library under `/a/stdlib`, holding
    /// one empty module.
    fn empty_package(header: &PackageHeader) -> Vec<u8> {
        let path = "/a/stdlib/std/x.mojo".to_string();
        write_package(
            header,
            &library("/a/stdlib"),
            &MirProgram {
                functions: Vec::new(),
                declarations: MirDeclarations::default(),
                invariant_errors: Vec::new(),
            },
            &[],
            || {
                Ok(crate::mir::text::package::DeclarationRecords {
                    modules: vec![PackagedModule {
                        name: "std.x".to_string(),
                        canonical: path.clone(),
                        path,
                        link: Vec::new(),
                    }],
                    ..Default::default()
                })
            },
        )
        .expect("write")
    }

    fn library(root: &str) -> LibraryRoot {
        LibraryRoot {
            stamped: root.to_string(),
            canonical: root.to_string(),
        }
    }

    #[test]
    fn a_package_another_build_wrote_is_refused() {
        let header = std_header();
        let text = empty_package(&header);
        let root = library("/a/stdlib");
        assert!(import(PackageBytes::Owned(text.clone()), &header, &root).is_ok());
        let other = PackageHeader {
            compiler: "another-build".to_string(),
            ..header
        };
        assert!(import(PackageBytes::Owned(text), &other, &root).is_err());
    }

    #[test]
    fn a_package_reads_under_any_root() {
        let header = std_header();
        let text = empty_package(&header);
        assert!(refuse_install_paths(&text, &library("/a/stdlib")).is_ok());
        let package = import(PackageBytes::Owned(text), &header, &library("/b/stdlib"))
            .expect("imports under another root");
        let module = package.declarations.modules().next().expect("one module");
        assert_eq!(module.path, "/b/stdlib/std/x.mojo");
        assert_eq!(module.canonical, "/b/stdlib/std/x.mojo");
    }

    #[test]
    fn only_owner_named_lifted_functions_are_packaged() {
        let program = MirProgram {
            functions: vec![(
                "List.f".to_string(),
                crate::mir::MirFunction {
                    blocks: Vec::new(),
                    n_regs: 0,
                    n_vars: 0,
                    var_names: Vec::new(),
                    n_params: 0,
                    param_types: Vec::new(),
                    owned_params: Vec::new(),
                    deinit_params: Vec::new(),
                    ref_params: Vec::new(),
                    returns_reference: false,
                    var_tys: std::collections::HashMap::new(),
                    ret_ty: None,
                    raises: false,
                    error_ty: None,
                    spans: crate::mir::SpanTable::default(),
                    reg_types: std::collections::HashMap::new(),
                },
            )],
            declarations: MirDeclarations::default(),
            invariant_errors: Vec::new(),
        };
        assert!(lifted_name_is_stable("List.f", &program));
        assert!(lifted_name_is_stable(
            "$comptime$__module$std$io$stdout$module",
            &program
        ));
        assert!(lifted_name_is_stable("$comptime$List.f$0", &program));
        assert!(lifted_name_is_stable("$comptime$List.f$c0", &program));
        assert!(!lifted_name_is_stable("$comptime$L$12", &program));
        assert!(!lifted_name_is_stable("$comptime$__toplevel__$0", &program));
    }

    #[test]
    fn a_package_answers_overload_queries_as_its_source_does() {
        // An importer's overload sets answer a library name from the
        // record of the declaration owning it, as the whole library's
        // sets answer it when the program compiles from source.
        use crate::symbol::OverloadLookup;
        let source = "from std.io.file import O_CREAT\n\n\
                      def main():\n    print(O_CREAT, len(List[Int]()))\n";
        let compile = |package: bool| {
            Compiler::default()
                .with_std_package(package)
                .compile_source(source, Path::new("/probe/overloads.mojo"))
                .expect("compiles")
        };
        let (from_source, from_package) = (compile(false), compile(true));
        let whole = from_source.checked().overloads();
        let imported = from_package.checked().overloads();
        for (name, arity) in [("range", 1), ("range", 2), ("reversed", 1), ("hash", 1)] {
            assert_eq!(
                imported.function_is_overloaded(name, arity),
                whole.function_is_overloaded(name, arity),
                "`{name}` at {arity}"
            );
        }
        for name in [
            "print",
            "len",
            "range",
            "reversed",
            "main",
            "List",
            "List.append",
        ] {
            assert_eq!(
                imported.is_function(name),
                whole.is_function(name),
                "`{name}`"
            );
        }
        assert!(
            !imported.is_function("List.append"),
            "a method is no function"
        );
        for name in ["StopIteration", "List", "String"] {
            assert_eq!(
                imported.fieldwise_parameters(name),
                whole.fieldwise_parameters(name),
                "`{name}`"
            );
        }
        for (method, arity) in [
            ("String.__init__", 1),
            ("List.__init__", 1),
            ("List.append", 1),
        ] {
            assert_eq!(
                imported.method_is_overloaded(method, arity),
                whole.method_is_overloaded(method, arity),
                "`{method}` at {arity}"
            );
        }
        // A module's own name is spelled through its module once linked.
        let constant = "__module$std$io$file$O_CREAT";
        let (imported_constants, whole_constants) = (
            from_package.checked().constants(),
            from_source.checked().constants(),
        );
        assert_eq!(
            imported_constants.get(constant),
            whole_constants.get(constant)
        );
        assert!(imported.function_is_overloaded("range", 2));
        assert_eq!(
            imported_constants.get(constant),
            Some(crate::IntLiteral::from(0x40_i64))
        );
    }

    #[test]
    fn a_package_links_what_its_source_links() {
        use crate::module::{link_source_with_options, link_source_with_package};
        let package = bundled_std_package()
            .expect("the installed package reads")
            .expect("the bundled library packages");
        let root = bundled_root().expect("a bundled root").join("stdlib");
        let mut source = String::new();
        for module in module_paths(&root.join("std"), "std") {
            source.push_str("import ");
            source.push_str(&module);
            source.push('\n');
        }
        source.push_str("def main():\n    print(String(\"probe\"))\n");
        let entry = Path::new("probe.mojo");
        let from_source = link_source_with_options(&source, entry, LinkOptions::default())
            .expect("links from source");
        let from_package =
            link_source_with_package(&source, entry, LinkOptions::default(), &package.modules)
                .expect("links from the package")
                .statements;
        let packaged = |statement: &&crate::ast::Stmt| {
            package
                .declarations
                .modules()
                .any(|module| Some(module.path) == statement.module.as_deref())
        };
        // The program's own statements link as their source spells them.
        assert!(
            from_source
                .iter()
                .filter(|statement| !packaged(statement))
                .eq(from_package.iter().filter(|statement| !packaged(statement)))
        );
        // An importer reads every package declaration from the interfaces,
        // so its link replays none.
        assert!(!from_package.iter().any(|statement| packaged(&statement)));
    }

    #[test]
    fn every_library_module_has_an_interface() {
        let package = bundled_std_package()
            .expect("the installed package reads")
            .expect("the bundled library packages");
        let directory = package.declarations.directory();
        let list = directory
            .modules()
            .position(|module| module.name == "std.collections.list")
            .expect("the library has a list module");
        assert!(
            (0..directory.declaration_count())
                .filter_map(|position| directory.declaration(position))
                .any(|entry| entry.module as usize == list)
        );
        let installed = crate::checker::InstalledInterfaces::reaching(
            &package.checked,
            0..directory.module_count() as u32,
        );
        assert_eq!(installed.decoded(), 0);
    }
}
