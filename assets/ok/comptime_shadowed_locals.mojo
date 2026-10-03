# Module comptime constants materialize shadow-aware: a local declaration,
# loop variable, declaring unpack target, or later statement in the same
# block that rebinds the name stays local instead of becoming the
# materialized literal, in a generic `def`'s body as in a plain one
# (identical output on both compilers).
comptime n = 2 + 3
comptime i = 40

def shadowing() -> Int:
    var i = 1
    i += 1
    var total = 0
    for n in range(3):
        total += n
    return i + total

def generic_shadowing[T: Copyable](value: T) -> Int:
    var i = 100
    return i

def unpack_shadowing() -> Int:
    var i, j = 100, 1
    return i + j

def generic_unpack_shadowing[T: Copyable](value: T) -> Int:
    var n, j = 200, 2
    return n + j + i

def main():
    print(n, i)
    print(shadowing())
    print(generic_shadowing(1), generic_shadowing("a"))
    print(unpack_shadowing())
    print(generic_unpack_shadowing(1.5))
