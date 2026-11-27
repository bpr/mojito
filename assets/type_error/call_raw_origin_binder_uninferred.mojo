# expect: failed to infer parameter 'origin_of(r)'
# A raw origin binder `r` that no sibling names as its `_mlir_origin`
# stays unbound when the argument binds `o` (and so `m`).
from std.origin import _lit_origin_type_of_mut


def by_r[
    m: Bool, r: _lit_origin_type_of_mut[m], //, o: Origin[mut=m]
](ref[o] x: Int) -> Bool:
    return m


def main():
    var n = 1
    print(by_r(n))
