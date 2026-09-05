# requires: discovery
# Methods of a specialized variadic struct that declare their own type
# parameters are specialized per call: the checker records each
# instantiation, the discovery loop replays it, the specializer mints one
# clone per distinct instantiation (`find$y3:Int`) whose `comptime if T ==
# Ts[i]` folds, and the call retargets to the clone by name. Static
# methods, reference-returning accessors, constructors with an infer-only
# `T` solved through a callable bound (`F: def() -> T`), and inferred
# (unspelled) type arguments all take that path.
from std.collections.tuple import Tuple

struct Bag[*Ts: Copyable & Movable](Copyable, Movable):
    var storage: Tuple[*Ts]

    def __init__(out self, var *args: *Ts):
        self.storage = Tuple[*Ts](*args^)

    def find[T: AnyType](self) -> Int:
        comptime for i in range(Ts.length):
            comptime if Ts[i] == T:
                return i
        return -1

    def get[T: AnyType](ref self) -> ref[origin_of(self)] T:
        comptime for i in range(Ts.length):
            comptime if Ts[i] == T:
                return self.storage[i]
        _mojito_abort("Bag.get: no such element type")

    def count_matching[T: AnyType](self, value: T) -> Int:
        comptime for i in range(Ts.length):
            comptime if Ts[i] == T:
                if self.storage[i] == value:
                    return 1
        return 0

    @staticmethod
    def has[T: AnyType]() -> Bool:
        return Ts.contains[T]()

struct Cell[*Ts: Copyable & Movable](Copyable, Movable):
    var tag: Int

    def __init__[T: AnyType, //, F: def() -> T](out self, *, init_with: F):
        self.tag = -1
        comptime for i in range(Self.Ts.length):
            comptime if Self.Ts[i] == T:
                self.tag = i

def main():
    var b = Bag[Int, String](7, "x")
    print(b.find[String]())
    print(b.find[Int]())
    print(b.find[Bool]())
    print(b.get[String]())
    print(b.get[Int]() + 1)
    print(b.count_matching(7))
    print(b.count_matching("y"))
    print(Bag[Int, String].has[Bool]())
    print(Bag[Int, String].has[String]())
    def make_str() -> String:
        return "s"
    var c = Cell[Int, String](init_with=make_str)
    print(c.tag)
    def make_int() -> Int:
        return 4
    print(Cell[Int, String](init_with=make_int).tag)
