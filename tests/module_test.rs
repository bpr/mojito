//! Module system (Phase 3): `from module import …` links a referenced `.mojo`
//! file's top-level declarations into the program. These tests write a small
//! multi-file layout into a unique temp directory, then either inspect linking or
//! compile and run the entry through the authoritative whole-program pipeline.

use mojito::{BackendKind, Compiler, LinkOptions, ModuleError, inject_prelude, link, parse};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A throwaway directory for one test's module files (best-effort cleanup on drop).
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("mojito_mod_{}_{}", std::process::id(), id));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        TempDir(dir)
    }
    fn write(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self.0.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create subdir");
        }
        std::fs::write(&path, contents).expect("write module file");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Compile and run the entry file through the authoritative whole-program
/// pipeline, returning its captured VM output.
fn run(entry: &Path) -> Result<String, String> {
    let compiler = Compiler::default();
    let program = compiler.compile_path(entry).map_err(|e| e.to_string())?;
    let execution = compiler.execute(&program).map_err(|e| e.to_string())?;
    Ok(execution.output)
}

fn declaration_count(program: &[mojito::Stmt], expected: &str) -> usize {
    program
        .iter()
        .filter(|statement| match &statement.kind {
            mojito::ast::StmtKind::Def { name, .. }
            | mojito::ast::StmtKind::Struct { name, .. }
            | mojito::ast::StmtKind::Trait { name, .. }
            | mojito::ast::StmtKind::Comptime { name, .. } => name == expected,
            _ => false,
        })
        .count()
}

#[test]
fn implicit_prelude_loads_each_core_collection_identity_once() {
    let d = TempDir::new();
    let main = d.write("main.mojo", "def main():\n    pass\n");
    let program = link(&main).expect("link implicit prelude");

    for (name, expected) in [
        ("List", 1),
        ("Set", 1),
        ("Dict", 1),
        ("Optional", 1),
        ("Tuple", 1),
        // `range` is one public overload set with one-, two-, and three-argument
        // declarations, all under the same stable identity. The range structs
        // themselves are private (underscore) module members, like upstream.
        ("range", 3),
    ] {
        assert_eq!(
            declaration_count(&program, name),
            expected,
            "unexpected declaration count for {name}"
        );
    }
}

#[test]
fn explicit_core_imports_and_aliases_reuse_prelude_declarations() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "from std.collections.list import List as ExplicitList\nfrom std.collections.tuple import Tuple\nimport std.range as ranges\n\ndef consume(values: ExplicitList[Int], pair: Tuple[Int, Bool]):\n    pass\n\ndef main():\n    for i in ranges.range(1):\n        pass\n",
    );
    let program = link(&main).expect("link explicit core aliases");

    for (name, expected) in [("List", 1), ("Tuple", 1), ("range", 3)] {
        assert_eq!(
            declaration_count(&program, name),
            expected,
            "explicit import duplicated {name}"
        );
    }

    let consume = program
        .iter()
        .find_map(|statement| match &statement.kind {
            mojito::ast::StmtKind::Def { name, params, .. } if name == "consume" => Some(params),
            _ => None,
        })
        .expect("entry function");
    let type_names: Vec<_> = consume
        .iter()
        .map(|parameter| match &parameter.ty {
            mojito::Type::Named(name, _) => name.clone(),
            other => panic!("expected a named core type, got {other:?}"),
        })
        .collect();
    assert_eq!(type_names, ["List", "Tuple"]);
}

#[test]
fn parsed_snippets_can_inject_the_same_prelude_without_an_entry_path() {
    let parsed = parse("def main():\n    pass\n").expect("parse snippet");
    let program = inject_prelude(parsed).expect("inject implicit prelude");

    for (name, expected) in [
        ("List", 1),
        ("Set", 1),
        ("Dict", 1),
        ("Optional", 1),
        ("Tuple", 1),
        ("range", 3),
    ] {
        assert_eq!(
            declaration_count(&program, name),
            expected,
            "missing or duplicated {name}"
        );
    }
}

#[test]
fn implicit_prelude_is_visible_but_not_reexported_by_every_module() {
    let d = TempDir::new();
    d.write(
        "helper.mojo",
        "def size(values: List[Int]) -> Int:\n    return len(values)\n",
    );
    let main = d.write(
        "main.mojo",
        "from helper import size\n\ndef main():\n    pass\n",
    );
    let program = link(&main).expect("module sees implicit prelude");
    assert_eq!(declaration_count(&program, "List"), 1);

    let bad = d.write(
        "bad.mojo",
        "from helper import List\n\ndef main():\n    pass\n",
    );
    assert!(matches!(
        link(&bad),
        Err(mojito::ModuleError::NameNotFound { module, name })
            if module == "helper" && name == "List"
    ));
}

#[test]
fn selective_import_brings_struct_and_fn_into_scope() {
    let d = TempDir::new();
    d.write(
        "collections.mojo",
        "struct Pair:\n    var a: Int\n    var b: Int\n    def __init__(out self, a: Int, b: Int):\n        self.a = a\n        self.b = b\n    def sum(self) -> Int:\n        return self.a + self.b\n\ndef twice(x: Int) -> Int:\n    return x * 2\n",
    );
    let main = d.write(
        "main.mojo",
        "from collections import Pair, twice\n\ndef main():\n    print(Pair(3, 4).sum())\n    print(twice(21))\n",
    );
    assert_eq!(run(&main).unwrap(), "7\n42\n");
}

#[test]
fn linking_rewrites_conditional_conformance_and_where_clause_names() {
    let d = TempDir::new();
    d.write(
        "contracts.mojo",
        "trait Capability:\n    def use(self): ...\n\nstruct Conditional[T: Movable](Capability where conforms_to(T, Capability)):\n    def use(self) where conforms_to(Self.T, Capability):\n        pass\n\ndef constrained[T: Movable](value: T) where conforms_to(T, Capability):\n    pass\n",
    );
    let main = d.write(
        "main.mojo",
        "from contracts import Conditional, constrained\n\ndef main():\n    pass\n",
    );
    let program = link(&main).expect("link conditional declarations");

    let (trait_name, struct_stmt, function_stmt) = {
        let trait_name = program
            .iter()
            .find_map(|statement| match &statement.kind {
                mojito::ast::StmtKind::Trait { name, .. } if name.ends_with("$Capability") => {
                    Some(name.clone())
                }
                _ => None,
            })
            .expect("qualified trait");
        let struct_stmt = program
            .iter()
            .find(|statement| {
                matches!(&statement.kind, mojito::ast::StmtKind::Struct { name, .. } if name.ends_with("$Conditional"))
            })
            .expect("qualified struct");
        let function_stmt = program
            .iter()
            .find(|statement| {
                matches!(&statement.kind, mojito::ast::StmtKind::Def { name, .. } if name.ends_with("$constrained"))
            })
            .expect("qualified function");
        (trait_name, struct_stmt, function_stmt)
    };

    let mojito::ast::StmtKind::Struct {
        conforms,
        conformance_conditions,
        methods,
        ..
    } = &struct_stmt.kind
    else {
        unreachable!()
    };
    assert_eq!(conforms, std::slice::from_ref(&trait_name));
    assert_eq!(conformance_conditions[0].0, trait_name);
    let condition_names_trait = |condition: &mojito::Expr| {
        matches!(
            &condition.kind,
            mojito::ast::ExprKind::Call { args, .. }
                if matches!(&args[1].kind, mojito::ast::ExprKind::Identifier(name) if name == &trait_name)
        )
    };
    assert!(condition_names_trait(&conformance_conditions[0].1));
    assert!(
        matches!(methods[0].where_clauses.as_slice(), [condition] if condition_names_trait(condition))
    );

    let mojito::ast::StmtKind::Def { where_clauses, .. } = &function_stmt.kind else {
        unreachable!()
    };
    assert!(matches!(where_clauses.as_slice(), [condition] if condition_names_trait(condition)));
}

#[test]
fn wildcard_and_relative_import() {
    let d = TempDir::new();
    d.write(
        "util.mojo",
        "def triple(x: Int) -> Int:\n    return x * 3\n",
    );
    let main = d.write(
        "main.mojo",
        "from .util import *\n\ndef main():\n    print(triple(5))\n",
    );
    assert_eq!(run(&main).unwrap(), "15\n");
}

#[test]
fn imported_generic_comptime_alias_expands_in_type_annotations() {
    let d = TempDir::new();
    d.write(
        "aliases.mojo",
        "comptime Pair[T: Copyable & Movable]: AnyType = Tuple[T, T]\ncomptime Guard[n: Int]: AnyType where (n > 0, \"positive only\") = Int\n",
    );
    let main = d.write(
        "main.mojo",
        "from aliases import Pair, Guard as Bounded\n\ndef main():\n    var pair: Pair[Int] = (1, 2)\n    var guarded: Bounded[3] = 7\n    print(pair[0] + guarded)\n",
    );
    assert_eq!(run(&main).unwrap(), "8\n");
}

#[test]
fn transitive_and_dotted_imports() {
    let d = TempDir::new();
    d.write(
        "pkg/base.mojo",
        "def base(x: Int) -> Int:\n    return x + 1\n",
    );
    d.write(
        "mid.mojo",
        "from pkg.base import base\n\ndef mid(x: Int) -> Int:\n    return base(x) * 10\n",
    );
    let main = d.write(
        "main.mojo",
        "from mid import mid\n\ndef main():\n    print(mid(4))\n",
    );
    assert_eq!(run(&main).unwrap(), "50\n");
}

#[test]
fn bundled_stdlib_root_supports_mojo_shaped_imports() {
    let d = TempDir::new();
    let main = d.write(
        "main.mojo",
        "from std.optional import Optional\nfrom std.collections.list import List\n\ndef main():\n    var o: Optional[Int] = Optional[Int](9)\n    var xs: List[Int] = List[Int]()\n    xs.append(o.or_else(0))\n    print(xs[0])\n",
    );
    assert_eq!(run(&main).unwrap(), "9\n");
}

#[test]
fn custom_search_root_is_used_after_importer_directory() {
    let d = TempDir::new();
    d.write("lib/pkg/tool.mojo", "def answer() -> Int:\n    return 42\n");
    let main = d.write(
        "src/main.mojo",
        "from pkg.tool import answer\n\ndef main():\n    print(answer())\n",
    );
    let compiler = Compiler::new(
        LinkOptions {
            search_roots: vec![d.0.join("lib")],
        },
        BackendKind::Vm,
    );
    let execution = compiler.run_path(&main).unwrap();
    assert_eq!(execution.output, "42\n");
}

#[test]
fn custom_search_roots_are_tried_in_order() {
    let d = TempDir::new();
    d.write(
        "first/pkg/tool.mojo",
        "def answer() -> Int:\n    return 1\n",
    );
    d.write(
        "second/pkg/tool.mojo",
        "def answer() -> Int:\n    return 2\n",
    );
    let main = d.write(
        "src/main.mojo",
        "from pkg.tool import answer\n\ndef main():\n    print(answer())\n",
    );
    let compiler = Compiler::new(
        LinkOptions {
            search_roots: vec![d.0.join("first"), d.0.join("second")],
        },
        BackendKind::Vm,
    );
    let execution = compiler.run_path(&main).unwrap();
    assert_eq!(execution.output, "1\n");
}

#[test]
fn two_roots_share_a_namespace_directory_prefix() {
    // The permanent two-root namespace-directory case (Confirmed Alignment,
    // audit ae386d1b204): `foo.bar` and `foo.baz` resolve from distinct
    // search roots that both contribute to the `foo` namespace prefix.
    // Source-package precedence and package `__init__.mojo` boundaries are
    // unchanged.
    let d = TempDir::new();
    d.write("root_a/foo/bar.mojo", "def one() -> Int:\n    return 1\n");
    d.write("root_b/foo/baz.mojo", "def two() -> Int:\n    return 2\n");
    let main = d.write(
        "src/main.mojo",
        "from foo.bar import one\nfrom foo.baz import two\n\ndef main():\n    print(one() + two())\n",
    );
    let compiler = Compiler::new(
        LinkOptions {
            search_roots: vec![d.0.join("root_a"), d.0.join("root_b")],
        },
        BackendKind::Vm,
    );
    let execution = compiler.run_path(&main).unwrap();
    assert_eq!(execution.output, "3\n");
}

#[test]
fn importer_directory_precedes_custom_search_roots() {
    let d = TempDir::new();
    d.write("root/pkg/tool.mojo", "def answer() -> Int:\n    return 1\n");
    d.write("src/pkg/tool.mojo", "def answer() -> Int:\n    return 9\n");
    let main = d.write(
        "src/main.mojo",
        "from pkg.tool import answer\n\ndef main():\n    print(answer())\n",
    );
    let compiler = Compiler::new(
        LinkOptions {
            search_roots: vec![d.0.join("root")],
        },
        BackendKind::Vm,
    );
    let execution = compiler.run_path(&main).unwrap();
    assert_eq!(execution.output, "9\n");
}

#[test]
fn source_package_precedes_same_named_source_module() {
    let d = TempDir::new();
    d.write("choice.mojo", "def answer() -> Int:\n    return 1\n");
    d.write(
        "choice/__init__.mojo",
        "def answer() -> Int:\n    return 2\n",
    );
    let main = d.write(
        "main.mojo",
        "from choice import answer\n\ndef main():\n    print(answer())\n",
    );
    assert_eq!(run(&main).unwrap(), "2\n");
}

#[test]
fn ordinary_directories_can_form_dotted_import_paths() {
    let d = TempDir::new();
    d.write(
        "plain/nested/tool.mojo",
        "def answer() -> Int:\n    return 42\n",
    );
    let main = d.write(
        "main.mojo",
        "import plain.nested.tool\n\ndef main():\n    print(plain.nested.tool.answer())\n",
    );
    assert_eq!(run(&main).unwrap(), "42\n");
}

#[test]
fn linked_declarations_preserve_module_identity_in_checked_program() {
    let d = TempDir::new();
    let module = d.write("library.mojo", "def answer() -> Int:\n    return 42\n");
    let main = d.write(
        "main.mojo",
        "from library import answer\n\ndef main():\n    print(answer())\n",
    );
    let compiler = Compiler::default();
    let program = compiler
        .compile_path(&main)
        .expect("compile linked modules");
    let checked = program.checked();
    let answer = checked
        .statements()
        .iter()
        .find(|stmt| matches!(&stmt.kind, mojito::ast::StmtKind::Def { name, .. } if name.ends_with("answer")))
        .unwrap();
    let entry = checked
        .statements()
        .iter()
        .find(
            |stmt| matches!(&stmt.kind, mojito::ast::StmtKind::Def { name, .. } if name == "main"),
        )
        .unwrap();
    assert_eq!(answer.module.as_deref(), Some(module.to_str().unwrap()));
    assert_eq!(entry.module.as_deref(), Some(main.to_str().unwrap()));
}

#[test]
fn linked_expression_locations_include_their_source_module() {
    let d = TempDir::new();
    let library = d.write(
        "lib.mojo",
        "def pick(x: Int) -> Int:\n    return x\n\ndef pick(x: String) -> String:\n    return x\n\ndef from_lib() -> Int:\n    return pick(1)\n",
    );
    let entry = d.write(
        "main.mojo",
        "from lib import pick, from_lib\n\ndef pick(x: Bool) -> Bool:\n    return x\n\ndef main():\n    print(from_lib(), pick(True))\n",
    );
    let compiler = Compiler::default();
    let program = compiler
        .compile_path(&entry)
        .expect("compile linked modules");
    let checked = program.checked();

    let sources: std::collections::HashSet<_> = checked
        .overload_targets()
        .keys()
        .filter_map(|location| location.source.as_deref())
        .collect();
    assert!(sources.contains(library.to_str().unwrap()));
    assert!(sources.contains(entry.to_str().unwrap()));

    let from_lib = checked
        .statements()
        .iter()
        .find(|statement| matches!(&statement.kind, mojito::ast::StmtKind::Def { name, .. } if name.ends_with("from_lib")))
        .expect("imported function");
    let mojito::ast::StmtKind::Def { body, .. } = &from_lib.kind else {
        unreachable!()
    };
    let mojito::ast::StmtKind::Return(Some(call)) = &body[0].kind else {
        panic!("expected return call")
    };
    assert_eq!(call.source.as_deref(), Some(library.to_str().unwrap()));
    assert_eq!(body[0].module.as_deref(), Some(library.to_str().unwrap()));
}

#[test]
fn missing_module_and_missing_name_error() {
    let d = TempDir::new();
    d.write("m.mojo", "def f(x: Int) -> Int:\n    return x\n");
    let bad_mod = d.write(
        "bad1.mojo",
        "from nope import f\ndef main():\n    print(1)\n",
    );
    assert!(
        run(&bad_mod)
            .unwrap_err()
            .contains("cannot load module 'nope'")
    );
    let bad_name = d.write("bad2.mojo", "from m import g\ndef main():\n    print(1)\n");
    assert!(
        run(&bad_name)
            .unwrap_err()
            .contains("no declaration named 'g'")
    );
}

#[test]
fn duplicate_explicit_import_bindings_from_distinct_modules_are_rejected() {
    let d = TempDir::new();
    d.write("left.mojo", "def pick() -> Int:\n    return 1\n");
    d.write(
        "right.mojo",
        "def pick() -> Int:\n    return 2\n\ndef other() -> Int:\n    return 3\n",
    );
    let direct = d.write(
        "direct.mojo",
        "from left import pick\nfrom right import pick\n\ndef main():\n    pass\n",
    );
    let aliased = d.write(
        "aliased.mojo",
        "from left import pick\nfrom right import other as pick\n\ndef main():\n    pass\n",
    );
    d.write(
        "facade.mojo",
        "from left import pick\nfrom right import pick\n",
    );
    let transitive = d.write(
        "transitive.mojo",
        "import facade\n\ndef main():\n    pass\n",
    );

    for entry in [&direct, &aliased, &transitive] {
        let error = link(entry).expect_err("different explicit imports must not overwrite");
        assert!(
            matches!(
                &error,
                ModuleError::DuplicateImport { module, name }
                    if module == "right" && name == "pick"
            ),
            "unexpected error: {error}"
        );
        assert_eq!(
            error.to_string(),
            "duplicate import of 'pick' from module 'right': an earlier import already binds \
             this name; rename one with 'as'"
        );
    }
}

#[test]
fn identical_explicit_imports_and_local_declarations_keep_shadowing_rules() {
    let d = TempDir::new();
    d.write("values.mojo", "def pick() -> Int:\n    return 1\n");
    d.write("reexport.mojo", "from values import pick\n");
    let repeated = d.write(
        "repeated.mojo",
        "from values import pick\nfrom values import pick\nfrom reexport import pick\n\ndef main():\n    print(pick())\n",
    );
    assert_eq!(run(&repeated).expect("same-target re-import"), "1\n");

    d.write(
        "local.mojo",
        "from values import pick\n\ndef pick() -> Int:\n    return 2\n",
    );
    let local_entry = d.write(
        "local_entry.mojo",
        "from local import pick\n\ndef main():\n    print(pick())\n",
    );
    assert_eq!(run(&local_entry).expect("local declaration shadow"), "2\n");
}

#[test]
fn explicit_imports_can_shadow_implicit_prelude_and_string_dict_bindings() {
    let d = TempDir::new();
    d.write(
        "replacements.mojo",
        "def replacement() -> Int:\n    return 42\n",
    );
    let prelude = d.write(
        "prelude.mojo",
        "from replacements import replacement as range\n\ndef main():\n    print(range())\n",
    );
    assert_eq!(run(&prelude).expect("explicit prelude shadow"), "42\n");

    let string_dict = d.write(
        "string_dict.mojo",
        "from replacements import replacement as StringDict\n\ndef collect(var **kwargs: Int):\n    pass\n\ndef main():\n    print(StringDict())\n",
    );
    link(&string_dict).expect("explicit StringDict shadow after runtime injection");
}

#[test]
fn exact_self_imports_are_rejected() {
    let d = TempDir::new();
    let named = d.write(
        "named.mojo",
        "from named import value\n\ndef value() -> Int:\n    return 1\n\ndef main():\n    pass\n",
    );
    let qualified = d.write(
        "qualified.mojo",
        "import qualified\n\ndef main():\n    pass\n",
    );
    let relative = d.write(
        "relative.mojo",
        "from . import relative\n\ndef main():\n    pass\n",
    );

    for (entry, module) in [
        (&named, "named"),
        (&qualified, "qualified"),
        (&relative, "relative"),
    ] {
        let error = link(entry).expect_err("a module must not import its own file");
        assert!(
            matches!(&error, ModuleError::SelfImport { module: found } if found == module),
            "unexpected error: {error}"
        );
        assert_eq!(
            error.to_string(),
            format!("module '{module}' imports itself")
        );
    }
}

#[test]
fn self_imports_are_rejected_while_loading_modules_and_packages() {
    let d = TempDir::new();
    d.write(
        "dependency.mojo",
        "from dependency import value\n\ndef value() -> Int:\n    return 1\n",
    );
    let module_entry = d.write(
        "module_entry.mojo",
        "import dependency\n\ndef main():\n    pass\n",
    );
    let error = link(&module_entry).expect_err("loaded module self-import");
    assert!(
        matches!(&error, ModuleError::SelfImport { module } if module == "dependency"),
        "unexpected error: {error}"
    );

    d.write("pkg/__init__.mojo", "from .. import pkg\n");
    let package_entry = d.write(
        "package_entry.mojo",
        "import pkg\n\ndef main():\n    pass\n",
    );
    let error = link(&package_entry).expect_err("package initializer self-import");
    assert!(
        matches!(&error, ModuleError::SelfImport { module } if module == "pkg"),
        "unexpected error: {error}"
    );
}

#[test]
fn mutual_module_import_cycle_is_not_a_self_import() {
    let d = TempDir::new();
    d.write(
        "a.mojo",
        "from b import value_b\n\ndef value_a() -> Int:\n    return value_b()\n",
    );
    d.write(
        "b.mojo",
        "from a import value_a\n\ndef value_b() -> Int:\n    return 42\n",
    );
    let main = d.write(
        "main.mojo",
        "from a import value_a\n\ndef main():\n    print(value_a())\n",
    );
    assert_eq!(run(&main).expect("mutual import cycle"), "42\n");
}

#[test]
fn aliases_qualified_imports_and_same_named_declarations_do_not_collide() {
    let d = TempDir::new();
    d.write("left.mojo", "def answer() -> Int:\n    return 1\n");
    d.write("right.mojo", "def answer() -> Int:\n    return 2\n");
    let main = d.write(
        "main.mojo",
        "from left import answer as left_answer\nimport right as r\n\ndef main():\n    print(left_answer(), r.answer())\n",
    );
    assert_eq!(run(&main).unwrap(), "1 2\n");
}

#[test]
fn unaliased_dotted_import_uses_the_full_qualified_path() {
    let d = TempDir::new();
    d.write("pkg/__init__.mojo", "");
    d.write("pkg/tool.mojo", "def answer() -> Int:\n    return 42\n");
    let main = d.write(
        "main.mojo",
        "import pkg.tool\n\ndef main():\n    print(pkg.tool.answer())\n",
    );
    assert_eq!(run(&main).unwrap(), "42\n");
}

#[test]
fn dotted_import_prefix_is_shadowed_as_one_namespace_tree() {
    let d = TempDir::new();
    d.write("pkg/__init__.mojo", "");
    d.write("pkg/tool.mojo", "def answer() -> Int:\n    return 42\n");
    let main = d.write(
        "main.mojo",
        "import pkg.tool\n\ndef echo(pkg: Int) -> Int:\n    return pkg\n\ndef main():\n    print(echo(7))\n",
    );
    assert_eq!(run(&main).unwrap(), "7\n");
}

#[test]
fn dotted_namespace_resolves_exported_types() {
    let d = TempDir::new();
    d.write("pkg/__init__.mojo", "");
    d.write(
        "pkg/models.mojo",
        "@fieldwise_init\nstruct Box:\n    var value: Int\n",
    );
    let main = d.write(
        "main.mojo",
        "import pkg.models\n\ndef main():\n    var box: pkg.models.Box = pkg.models.Box(9)\n    print(box.value)\n",
    );
    assert_eq!(run(&main).unwrap(), "9\n");
}

#[test]
fn local_bindings_shadow_imported_members() {
    let d = TempDir::new();
    d.write("values.mojo", "comptime value = 41\n");
    let main = d.write(
        "main.mojo",
        "from values import value\n\ndef main():\n    var value: Int = 7\n    print(value)\n",
    );
    assert_eq!(run(&main).unwrap(), "7\n");
}

#[test]
fn imports_inside_functions_and_blocks_are_lexically_scoped() {
    let d = TempDir::new();
    d.write("util.mojo", "def answer() -> Int:\n    return 42\n");
    let main = d.write(
        "main.mojo",
        "def main():\n    from util import answer\n    if True:\n        import util as nested\n        print(nested.answer())\n    print(answer())\n",
    );
    assert_eq!(run(&main).unwrap(), "42\n42\n");

    let bad = d.write(
        "bad.mojo",
        "def main():\n    if True:\n        from util import answer\n        print(answer())\n    print(answer())\n",
    );
    assert!(run(&bad).unwrap_err().contains("answer"));
}

#[test]
fn package_init_reexports_members() {
    let d = TempDir::new();
    d.write("tools/value.mojo", "def answer() -> Int:\n    return 42\n");
    d.write("tools/__init__.mojo", "from .value import answer\n");
    let main = d.write(
        "main.mojo",
        "from tools import answer\n\ndef main():\n    print(answer())\n",
    );
    assert_eq!(run(&main).unwrap(), "42\n");
}

#[test]
fn package_init_can_reexport_a_submodule_namespace() {
    let d = TempDir::new();
    d.write("tools/value.mojo", "def answer() -> Int:\n    return 42\n");
    d.write("tools/__init__.mojo", "from . import value\n");
    let main = d.write(
        "main.mojo",
        "import tools\n\ndef main():\n    print(tools.value.answer())\n",
    );
    assert_eq!(run(&main).unwrap(), "42\n");
}

#[test]
fn package_submodule_requires_reexport_or_explicit_import() {
    let d = TempDir::new();
    d.write("tools/__init__.mojo", "");
    d.write("tools/value.mojo", "def answer() -> Int:\n    return 42\n");
    let direct = d.write(
        "direct.mojo",
        "import tools.value\n\ndef main():\n    print(tools.value.answer())\n",
    );
    assert_eq!(run(&direct).unwrap(), "42\n");

    let hidden = d.write(
        "hidden.mojo",
        "import tools\n\ndef main():\n    print(tools.value.answer())\n",
    );
    assert!(run(&hidden).is_err());
}

#[test]
fn sibling_modules_are_not_implicitly_visible() {
    let d = TempDir::new();
    d.write("pkg/__init__.mojo", "");
    d.write("pkg/tool.mojo", "def answer() -> Int:\n    return 42\n");
    d.write(
        "pkg/use.mojo",
        "def indirect() -> Int:\n    return tool.answer()\n",
    );
    let main = d.write(
        "main.mojo",
        "from pkg.use import indirect\n\ndef main():\n    print(indirect())\n",
    );
    assert!(run(&main).unwrap_err().contains("tool"));
}

#[test]
fn dots_only_relative_import_binds_a_sibling_module_namespace() {
    let d = TempDir::new();
    d.write("pkg/__init__.mojo", "");
    d.write("pkg/tool.mojo", "def answer() -> Int:\n    return 42\n");
    d.write(
        "pkg/use.mojo",
        "from . import tool\n\ndef indirect() -> Int:\n    return tool.answer()\n",
    );
    let main = d.write(
        "main.mojo",
        "from pkg.use import indirect\n\ndef main():\n    print(indirect())\n",
    );
    assert_eq!(run(&main).unwrap(), "42\n");
}

#[test]
fn wildcard_import_hides_underscore_prefixed_declarations() {
    let d = TempDir::new();
    d.write(
        "api.mojo",
        "def shown() -> Int:\n    return 1\n\ndef _hidden() -> Int:\n    return 2\n",
    );
    let main = d.write(
        "main.mojo",
        "from api import *\n\ndef main():\n    print(shown())\n",
    );
    assert_eq!(run(&main).unwrap(), "1\n");
    let bad = d.write(
        "bad.mojo",
        "from api import *\n\ndef main():\n    print(_hidden())\n",
    );
    assert!(run(&bad).unwrap_err().contains("_hidden"));
}

#[test]
fn imported_trait_effect_types_are_rewritten_with_their_module() {
    let d = TempDir::new();
    d.write(
        "validation.mojo",
        "@fieldwise_init\nstruct ValidationError:\n    var reason: String\n\ntrait Validates:\n    def validate(self) raises ValidationError -> Int: ...\n\n@fieldwise_init\nstruct Validator(Validates):\n    var value: Int\n    def validate(self) raises ValidationError -> Int:\n        if self.value < 0:\n            raise ValidationError(\"negative\")\n        return self.value\n\ndef invoke[T: Validates](value: T) raises ValidationError -> Int:\n    return value.validate()\n",
    );
    let main = d.write(
        "main.mojo",
        "from validation import ValidationError, Validator, invoke\n\ndef main() raises ValidationError:\n    print(invoke(Validator(7)))\n",
    );

    assert_eq!(run(&main).unwrap(), "7\n");
}

#[test]
fn std_traits_and_std_origin_export_builtin_identities() {
    let d = TempDir::new();

    // Named imports resolve and the names stay usable as bounds/binders.
    let main = d.write(
        "main.mojo",
        "from std.traits import Deinitable, Movable, IsTriviallyCopyable\nfrom std.origin import Origin\n\nstruct Res(Movable, Deinitable where False):\n    var id: Int\n    def __init__(out self, id: Int):\n        self.id = id\n    def close(deinit self):\n        print(\"closed\", self.id)\n\ndef borrow[origin: Origin[mut=True]](ref[origin] value: Int) -> ref[origin] Int:\n    return value\n\ndef main():\n    var r = Res(1)\n    r^.close()\n    var x = 40\n    ref y = borrow(x)\n    y += 2\n    print(x)\n    comptime if IsTriviallyCopyable[Int]:\n        print(\"trivial int\")\n",
    );
    assert_eq!(run(&main).expect("run"), "closed 1\n42\ntrivial int\n");

    // An alias rewrites to the canonical structural spelling.
    let aliased = d.write(
        "aliased.mojo",
        "from std.traits import Movable as M\n\ndef consume[T: M](var value: T) -> Int:\n    return 1\n\ndef main():\n    print(consume(42))\n",
    );
    assert_eq!(run(&aliased).expect("run"), "1\n");

    // Wildcard imports work through the same export table.
    let wild = d.write(
        "wild.mojo",
        "from std.origin import *\n\ndef borrow[origin: Origin[mut=True]](ref[origin] value: Int) -> ref[origin] Int:\n    return value\n\ndef main():\n    var x = 41\n    ref y = borrow(x)\n    y += 1\n    print(x)\n",
    );
    assert_eq!(run(&wild).expect("run"), "42\n");

    // A name upstream does not export stays a NameNotFound error.
    let unknown = d.write(
        "unknown.mojo",
        "from std.traits import NotAThing\n\ndef main():\n    pass\n",
    );
    let error = run(&unknown).expect_err("unknown import must fail");
    assert!(
        error.contains("no declaration named 'NotAThing'"),
        "unexpected error: {error}"
    );

    // The pre-rename predicate spelling is gone upstream and stays gone here.
    let renamed = d.write(
        "renamed.mojo",
        "from std.traits import TriviallyMovable\n\ndef main():\n    pass\n",
    );
    let error = run(&renamed).expect_err("old predicate spelling must fail");
    assert!(
        error.contains("no declaration named 'TriviallyMovable'"),
        "unexpected error: {error}"
    );
}

#[test]
fn std_memory_exports_the_allocation_vocabulary() {
    let d = TempDir::new();
    // Explicit imports select the module declarations; `alloc` is also a
    // prelude name, and both routes bind one identity.
    let main = d.write(
        "main.mojo",
        "from std.memory import Layout, Allocation, ThinAllocation, alloc, dealloc, unsafe_alloc\n\ndef main():\n    var allocation: Allocation[Int] = alloc(Layout[Int](count=2))\n    allocation.unsafe_ptr().unsafe_write(5)\n    print(allocation.unsafe_ptr()[])\n    dealloc(allocation^)\n    var raw = unsafe_alloc[Int](1)\n    raw.unsafe_write(7)\n    print(raw[])\n    raw.unsafe_free()\n",
    );
    assert_eq!(run(&main).expect("run"), "5\n7\n");

    // The prelude binding alone suffices for `alloc`.
    let prelude = d.write(
        "prelude_only.mojo",
        "from std.memory import Layout, dealloc\n\ndef main():\n    var a = alloc(Layout[Int](count=1))\n    a.unsafe_ptr().unsafe_write(1)\n    print(a.unsafe_ptr()[])\n    dealloc(a^)\n",
    );
    assert_eq!(run(&prelude).expect("run"), "1\n");

    // std.memory's Layout[T] and the layout package's Layout are distinct
    // declarations; importing only unsafe_alloc keeps the layout package's
    // Layout unshadowed.
    let with_layout_package = d.write(
        "with_layout.mojo",
        "from std.memory import unsafe_alloc\nfrom layout import Layout, LayoutTensor\n\ndef main():\n    var data = unsafe_alloc[Scalar[DType.int32]](4)\n    var i = 0\n    while i < 4:\n        data[i] = Scalar[DType.int32](0)\n        i += 1\n    var tensor = LayoutTensor[DType.int32, Layout.row_major(4)](data)\n    tensor[0] = Scalar[DType.int32](9)\n    print(tensor[0])\n    data.unsafe_free()\n",
    );
    assert_eq!(run(&with_layout_package).expect("run"), "9\n");
}
