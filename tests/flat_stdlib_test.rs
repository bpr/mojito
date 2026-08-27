//! Legacy flat-import facades must resolve to the same authoritative `std.*`
//! declarations used by the compiler prelude and canonical imports.

use mojito::Compiler;
use std::path::Path;

#[test]
fn flat_list_and_iterable_facades_construct_iterate_and_pop() {
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "from iterable import Iterable, Iterator, StopIteration\nfrom list import List\n\ndef main():\n    var values = List[Int]()\n    values.append(2)\n    values.append(5)\n    values.append(9)\n    for value in values:\n        print(value)\n    print(values.pop())\n    print(len(values))\n",
            Path::new("/tmp/mojito_flat_list_compat.mojo"),
        )
        .expect("flat List/Iterable imports use the authoritative declarations");
    let execution = compiler.execute(&program).expect("execute flat List");
    assert_eq!(execution.output, "2\n5\n9\n9\n2\n");
}

#[test]
fn remaining_flat_facades_reexport_their_public_surfaces() {
    mojito::link_source(
        "from dict import Dict, DictEntry\nfrom hashing import IncrementalHasher, bucket_index\nfrom math import ceil, ceildiv, floor, trunc\nfrom optional import Optional\nfrom set import Set\n\ndef main():\n    pass\n",
        Path::new("/tmp/mojito_flat_stdlib_compat.mojo"),
    )
    .expect("every flat facade exports its authoritative public names");
}

#[test]
fn flat_algorithms_facade_reexports_generic_and_ctfe_helpers() {
    let compiler = Compiler::default();
    let program = compiler
        .compile_source(
            "from algorithms import capacity_blocks, default_capacity, next_pow2, type_tag\n\ndef main():\n    print(next_pow2(9), default_capacity(), capacity_blocks[2]())\n    print(type_tag[Int](), type_tag[String]())\n",
            Path::new("/tmp/mojito_flat_algorithms_compat.mojo"),
        )
        .expect("flat algorithms import resolves the authoritative helpers");
    let execution = compiler
        .execute(&program)
        .expect("execute flat algorithms helpers");
    assert_eq!(execution.output, "16 8 16\n1 2\n");
}
