# An instance method called through a `SIMD` type takes the receiver as the
# first argument, whatever spelling names the type: a scalar alias (`Int`,
# `Int8`), an explicit application, a local alias, or `type_of`. An argument
# of another type converts to the type first (`Float64.__int__(2.5)`,
# `Int8.__add__(100, 100)` wraps), and the method read as a value takes its
# receiver first. A static called through a generic alias's application is
# the static of the type the alias names, a symbolic argument included.
struct W[n: Int]:
    @staticmethod
    def get() -> Int:
        return Self.n


comptime V[k: Int] = W[k]


def through_alias[k: Int]() -> Int:
    return V[k].get()


def main():
    var x = 1
    print(Int.__add__(x, x))
    var y: Int8 = 1
    print(Int8.__add__(y, y))
    print(Float64.__int__(2.5))
    print(SIMD[DType.int8, 1].__add__(y, y))
    print(Int8.__add__(100, 100))
    var z: Float32 = 1.5
    print(Float32.__add__(z, 1))
    print(type_of(y).__mul__(y, y))
    comptime T = Int16
    print(T.__add__(3, 4))
    print(String.byte_length("abcd"))
    var f = Int.__add__
    print(f(1, 2))
    print(through_alias[3](), V[4].get())
