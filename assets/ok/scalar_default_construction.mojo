# The scalar built-ins are `Defaultable`: `Int()`, `UInt()`, `Float64()` and
# `Bool()` are their zeros, at run time, at compile time, and through a
# `Defaultable` type parameter.
comptime ZERO = Int()


def zero[T: Defaultable & Writable & Copyable]() -> T:
    return T()


def main():
    var count = Int()
    count += 2
    print(count, UInt(), Float64(), Bool())
    print(ZERO)
    print(zero[Int](), zero[UInt](), zero[Float64](), zero[Bool]())
