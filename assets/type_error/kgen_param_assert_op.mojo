# expect: constraint failed: n=0 but must be positive
# Upstream's `kgen.param.assert` op, the form a `comptime assert` denotes,
# fails an instance whose condition is false with its concatenated message.
from std.collections.string.string_span import _get_kgen_string


def _assert[cond: Bool, msg: StaticString, *extra: StaticString]():
    __mlir_op.`kgen.param.assert`[
        cond=cond.__mlir_bool__(),
        message=_get_kgen_string[msg, *extra](),
    ]()


def check[n: Int]():
    _assert[n > 0, "n=", String(n), " but must be positive"]()


def main():
    check[3]()
    check[0]()
