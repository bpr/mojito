# requires: discovery
# Methods of a specialized variadic struct that declare their own type
# parameters are specialized per call: the checker records each
# instantiation, the discovery loop replays it, the specializer mints one
# clone per distinct instantiation (`find$y3:Int`) whose `comptime if
# Self.Ts[i] == T` folds, and the call retargets to the clone by name. Static
# methods, constructors with an infer-only `T` solved through a callable bound
# (`F: def() -> T`), and inferred (unspelled) type arguments all take that
# path. The accessors that narrow a pack element to the method's own `T` need
# upstream's `rebind`, so they live in
# `conformance/fixtures/pack_element_type_narrowing.mojo` instead.
struct Bag[*Ts: Movable](
    Copyable where Ts.all_conforms_to[Copyable](),
    Deinitable where Ts.all_conforms_to[Deinitable](),
    Movable,
):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple[*Self.Ts](*args^)

    def find[T: AnyType](self) -> Int:
        comptime for i in range(Self.Ts.length):
            comptime if Self.Ts[i] == T:
                return i
        return -1

    @staticmethod
    def has[T: AnyType]() -> Bool:
        return Self.Ts.contains[T]()

struct Cell[*Ts: Movable](Copyable, Movable):
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
    print(Bag[Int, String].has[Bool]())
    print(Bag[Int, String].has[String]())
    def make_str() -> String:
        return "s"
    var c = Cell[Int, String](init_with=make_str)
    print(c.tag)
    def make_int() -> Int:
        return 4
    print(Cell[Int, String](init_with=make_int).tag)
