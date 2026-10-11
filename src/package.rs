//! The bundled standard library as a compiled package.
//!
//! The library's bodies are checked, lowered, and ownership-verified once
//! per compiler build, into one `std` package file (`mir::text::package`).
//! Every compilation after that imports it instead of reading the library
//! again, as upstream imports its precompiled `std.mojoc`: its link reads
//! each library module's record (`module::package`), and it inserts a
//! body's drops when its elaboration first reaches it, as upstream's
//! importer runs its lifetime pass on each body of a package that holds them
//! before destructor insertion. The package is
//! written to a cache the first time a build needs it, keyed by the
//! build's identity, which covers the library's source, and the library's
//! root: like upstream's, the importer reads no library source to find it.

use crate::checked_package::PackagedModule;
use crate::compiler::{Compiler, CompilerError};
use crate::interface::PackageInterface;
use crate::mir::MirProgram;
use crate::mir::text::package::{
    PackageBodies, PackageBytes, PackageDeclarations, PackageHeader, read_package, write_package,
};
use crate::module::{LinkOptions, bundled_root, link_source_recording};
use crate::module_package::{ModuleIdentity, PackageModules};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// An imported package, read once per process.
///
/// It holds the module records its importer's link reads, the interface its
/// importer's check reads, the declarations every phase of its importer
/// reads (the check through its index of them), and its bodies.
#[derive(Debug)]
pub struct ImportedPackage {
    pub modules: PackageModules,
    pub interface: Arc<PackageInterface>,
    /// The check's index of its declarations, each declaration's residue
    /// decoded on first lookup by each compilation.
    pub checked: Arc<crate::checker::DeclarationInterfaces>,
    pub declarations: Arc<PackageDeclarations>,
    pub bodies: PackageBodies,
}

/// The identity of this compiler build, which a package it reads must
/// carry.
pub const BUILD_ID: &str = env!("MOJITO_BUILD_ID");

/// The bundled library's package for this build, read once per process.
///
/// It comes from the cache when this build already wrote it, else it is
/// built and cached now. `None` when there is no bundled library, or
/// packaging it failed (then the library compiles from source, after one
/// warning).
pub fn bundled_std_package() -> Option<Arc<ImportedPackage>> {
    static PACKAGE: OnceLock<Option<Arc<ImportedPackage>>> = OnceLock::new();
    PACKAGE
        .get_or_init(|| {
            let root = bundled_root()?.join("stdlib");
            let header = PackageHeader {
                package: "std".to_string(),
                compiler: BUILD_ID.to_string(),
                root: root.display().to_string(),
            };
            match load_or_build(&root, &header) {
                Ok(package) => Some(Arc::new(package)),
                Err(error) => {
                    eprintln!(
                        "warning: the bundled standard library could not be packaged ({error}); \
                         it is compiled from source"
                    );
                    None
                }
            }
        })
        .clone()
}

/// Map the cached package `header` names, or build and cache it. One
/// process builds while any other that needs the same package waits for
/// it, under a lock file a crashed builder leaves stale after a while.
fn load_or_build(root: &Path, header: &PackageHeader) -> Result<ImportedPackage, String> {
    const STALE: std::time::Duration = std::time::Duration::from_mins(5);
    let cache = cache_dir();
    let stem = format!("std-{}-{}", header.compiler, root_key(&header.root));
    let path = cache.join(format!("{stem}.mojoc"));
    let lock = cache.join(format!(".{stem}.lock"));
    loop {
        let map = crate::timing::span("package.map");
        let bytes = std::fs::File::open(&path).and_then(|file| PackageBytes::map(&file));
        drop(map);
        if let Ok(bytes) = bytes
            && let Ok(package) = import(bytes, header)
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
                return import(PackageBytes::Owned(built?), header);
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
    )
}

/// Read `bytes` as the package `header` names.
fn import(bytes: PackageBytes, header: &PackageHeader) -> Result<ImportedPackage, String> {
    let package = read_package(bytes).map_err(|error| error.to_string())?;
    if package.header != *header {
        return Err(format!(
            "the package was built by compiler `{}` for library root `{}`, not this one",
            package.header.compiler, package.header.root
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
        modules: PackageModules::new(&package.header.root, modules),
        interface: Arc::new(package.interface),
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
/// checked effect summaries, and the ownership-checked MIR of its
/// declarations, before drop elaboration, and write them.
fn build(root: &Path, header: &PackageHeader) -> Result<Vec<u8>, CompilerError> {
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
    // An importer reads every library declaration from the declaration
    // interfaces, so a record holds none: each declaring module needs only
    // its record.
    let entry_module = entry.display().to_string();
    let packaged: Vec<_> = exports
        .prepared
        .iter()
        .filter(|statement| statement.module.as_deref() != Some(entry_module.as_str()))
        .collect();
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
    let interface = PackageInterface {
        names: packaged
            .iter()
            .filter_map(|statement| declared_name(statement))
            .collect(),
    };
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
    let records = exports
        .interfaces
        .records(
            modules,
            program.declarations.structs.clone(),
            program.declarations.functions.clone(),
            program.declarations.traits.clone(),
        )
        .map_err(|error| CompilerError::Verify(vec![error]))?;
    write_package(header, &interface, &program, records)
        .map_err(|error| CompilerError::Verify(vec![error.to_string()]))
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

/// Where compiled packages are cached: `MOJITO_CACHE_DIR`, else the
/// development tree's `target/mojito-cache`, else the user's cache
/// directory.
fn cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("MOJITO_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(root) = bundled_root()
        && root.join("Cargo.toml").is_file()
    {
        return root.join("target").join("mojito-cache");
    }
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("mojito")
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

fn declared_name(statement: &crate::ast::Stmt) -> Option<String> {
    use crate::ast::StmtKind;
    match &statement.kind {
        StmtKind::Def { name, .. }
        | StmtKind::Struct { name, .. }
        | StmtKind::Trait { name, .. }
        | StmtKind::Comptime { name, .. } => Some(name.clone()),
        _ => None,
    }
}

/// A file-name key for the library `root`, since a package names its
/// modules by path: a 64-bit FNV-1a of the path.
fn root_key(root: &str) -> String {
    let digest = root
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |digest, byte| {
            (digest ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
    format!("{digest:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::MirDeclarations;

    /// An empty package `header` names.
    fn empty_package(header: &PackageHeader) -> Vec<u8> {
        write_package(
            header,
            &PackageInterface::default(),
            &MirProgram {
                functions: Vec::new(),
                declarations: MirDeclarations::default(),
                invariant_errors: Vec::new(),
            },
            crate::mir::text::package::DeclarationRecords::default(),
        )
        .expect("write")
    }

    #[test]
    fn a_package_another_build_wrote_is_refused() {
        let header = PackageHeader {
            package: "std".to_string(),
            compiler: BUILD_ID.to_string(),
            root: "/std".to_string(),
        };
        let text = empty_package(&header);
        assert!(import(PackageBytes::Owned(text.clone()), &header).is_ok());
        let other = PackageHeader {
            compiler: "another-build".to_string(),
            ..header
        };
        assert!(import(PackageBytes::Owned(text), &other).is_err());
    }

    #[test]
    fn a_package_built_for_another_root_is_refused() {
        let header = PackageHeader {
            package: "std".to_string(),
            compiler: BUILD_ID.to_string(),
            root: "/std".to_string(),
        };
        let text = empty_package(&header);
        assert!(import(PackageBytes::Owned(text.clone()), &header).is_ok());
        let other = PackageHeader {
            root: "/elsewhere/std".to_string(),
            ..header
        };
        assert!(import(PackageBytes::Owned(text), &other).is_err());
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
        let package = bundled_std_package().expect("the bundled library packages");
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
            package.declarations.compiles(statement.module.as_deref())
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
        let package = bundled_std_package().expect("the bundled library packages");
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
