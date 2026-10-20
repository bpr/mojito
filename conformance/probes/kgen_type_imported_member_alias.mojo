# PROBE: a parameter typed by the bundled `_MLIR.KGENTypeType` (`!kgen.type`)
# in an importing module.
#
# **Differs.** The pin runs it and prints 2 twice. Mojito stops with "type
# mismatch for value parameter 'T': expected a value, found a type": the
# prepare-time fold that makes a `!kgen.type`-typed parameter a type binder
# reads the member aliases of the program's own structs only, not an
# imported package struct's. Filed as R1174 in `docs/roadmap.md`. When Mojito
# runs it, promote this file to `assets/ok/` with its manifest rows.
from std.builtin.variadics import _MLIR


def k[T: _MLIR.KGENTypeType]() -> Int:
    return 2


def main():
    print(k[Int]())
    print(k[_MLIR.KGENTypeType]())
