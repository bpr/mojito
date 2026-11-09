# A compile-time `Bool`'s operators are its builtin dunders over its
# `!kgen.scalar<bool>` storage: `b & c` is `pop.simd.and(b._mlir_value,
# c._mlir_value)` created back as a `Bool`, so `&`, `|`, and `^` commute and
# associate, `~b` and `not b` are `b ^ True`, `==`, `!=`, and `<` are their
# bodies over them, and `Bool(mlir_value=x._mlir_value)` is `x`, as is
# `Bool(value=x.__mlir_i1__())`, the round trip through the `i1` builtin
# casts. Each
# parameter type below is therefore the declared one, here and at each
# instance.


struct Tag[b: Bool](ImplicitlyCopyable):
    def __init__(out self):
        pass


struct Storage[v: __mlir_type.`!kgen.scalar<bool>`](ImplicitlyCopyable):
    def __init__(out self):
        pass


def show[b: Bool](x: Tag[b]) -> String:
    return String(b)


def stored[b: Bool](x: Storage[b._mlir_value]) -> String:
    return String(b)


def identities[b: Bool, c: Bool, d: Bool]():
    var commuted: Tag[c & b] = Tag[b & c]()
    var joined: Tag[d | (c & b)] = Tag[(b & c) | d]()
    var grouped: Tag[(b ^ c) ^ d] = Tag[d ^ (c ^ b)]()
    var inverted: Tag[b ^ True] = Tag[~b]()
    var negated: Tag[b ^ True] = Tag[not b]()
    var cancelled: Tag[b] = Tag[~(b ^ True)]()
    var idempotent: Tag[b] = Tag[b & b | b]()
    var dominated: Tag[False] = Tag[b & False]()
    var complement: Tag[False] = Tag[b & ~b]()
    var unequal: Tag[b ^ c] = Tag[b != c]()
    var equal: Tag[~(b ^ c)] = Tag[b == c]()
    var less: Tag[~b & c] = Tag[b < c]()
    var created: Tag[b & c] = Tag[Bool(mlir_value=(b & c)._mlir_value)]()
    var cast: Tag[b] = Tag[Bool(value=b.__mlir_i1__())]()
    var cast_joined: Tag[b & c] = Tag[Bool((c & b).__mlir_i1__())]()
    print(
        show(commuted),
        show(joined),
        show(grouped),
        show(inverted),
        show(negated),
        show(cancelled),
        show(idempotent),
        show(dominated),
        show(complement),
        show(unequal),
        show(equal),
        show(less),
        show(created),
        show(cast),
        show(cast_joined),
        stored[b ^ c](Storage[(c ^ b)._mlir_value]()),
    )


def main():
    identities[True, False, True]()
    identities[False, True, False]()
    identities[True, True, False]()
