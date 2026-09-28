# A surviving trait-bound module-level `def` converting a parameter whose
# type binder carries the conversion's bound (`Int(mode)` on `T: Intable`,
# `Float64` on `Floatable`, `Bool` on `Boolable`) derives its instances from
# the checked template (`docs/notes/instantiation-from-template.md`, class
# FixedCalls). A struct argument is read in place through its conversion
# dunder. A generic struct's method converting a field or a local of its
# parameter type derives the same way. The bundled `stat.S_ISDIR` family has
# this shape, and so reaches `os.path.isdir`, `isfile`, and `islink`.

from std.os.path import isdir, isfile, islink
from std.stat import S_ISDIR


@fieldwise_init
struct Mode(Boolable, Floatable, ImplicitlyCopyable, Intable):
    var bits: Int

    def __int__(self) -> Int:
        return self.bits

    def __float__(self) -> Float64:
        return Float64(self.bits) / 2.0

    def __bool__(self) -> Bool:
        return self.bits != 0


struct Holder[T: Intable & Floatable & ImplicitlyCopyable & Deinitable](Copyable):
    var value: Self.T

    def __init__(out self, value: Self.T):
        self.value = value

    def as_int(self) -> Int:
        return Int(self.value) + 1

    def local_float(self) -> Float64:
        var v = self.value
        return Float64(v)


def masked[T: Intable](mode: T) -> Int:
    return Int(mode) & 0o170000


def halved[T: Floatable](value: T) -> Float64:
    return Float64(value)


def truthy[T: Boolable](value: T) -> Bool:
    return Bool(value)


def main():
    print(masked(0o040755), masked(Mode(0o100644)))
    print(halved(3.5), halved(Mode(5)))
    print(truthy(True), truthy(Mode(0)))
    print(S_ISDIR(0o040755), S_ISDIR(Mode(0o100644)))
    var h = Holder(Mode(6))
    print(h.as_int(), h.local_float())
    print(isdir(String("/")), isfile(String("/")), islink(String("/")))
