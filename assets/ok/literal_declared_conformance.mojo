# A numeric literal answers a trait as its declared `IntLiteral` or
# `FloatLiteral` struct lists it: neither lists `Equatable` or `Comparable`,
# `IntLiteral` lists `Indexer` and `Ceilable`, `FloatLiteral` `Floatable`.
from std.math import Ceilable, ceil, floor, trunc



def gen[T: AnyType]() -> String:
    var s = String()
    comptime if conforms_to(T, Boolable):
        s += "boolable "
    else:
        s += "not-boolable "
    comptime if conforms_to(T, Ceilable):
        s += "ceilable "
    else:
        s += "not-ceilable "
    comptime if conforms_to(T, Intable):
        s += "intable"
    else:
        s += "not-intable"
    return s

def main():
    comptime I = type_of(3)
    comptime F = type_of(2.5)
    print(conforms_to(I, Equatable), conforms_to(I, Comparable), conforms_to(I, Indexer), conforms_to(I, Floatable), conforms_to(I, Ceilable), conforms_to(I, Boolable), conforms_to(I, Intable))
    print(conforms_to(F, Equatable), conforms_to(F, Comparable), conforms_to(F, Indexer), conforms_to(F, Floatable), conforms_to(F, Ceilable), conforms_to(F, Boolable), conforms_to(F, Intable))
    print(gen[I]())
    print(ceil(3), floor(3), trunc(3), ceil(2.5))
