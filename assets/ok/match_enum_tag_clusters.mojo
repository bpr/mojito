# Neighbouring cases whose first command tests an enum case on the same
# path form a cluster, as upstream's `emitCaseCluster` emits them: the
# discriminant is read once, one arm per case index runs its members in
# case order, and a payload the members test is read once for them all. A
# binding moves behind its case's remaining tests, so a tuple member reads
# its payload only once the later element matches. A `|` is not clustered:
# each alternative reads what it needs afresh, as does a binding after it.
from std.builtin.variadics import TypeList, _MLIR


struct Rec(EnumLike, ImplicitlyCopyable):
    var tag: Int
    var n: Int

    comptime _enum_case_names = ParameterList.of[
        "Leaf".value, "Node".value, "Pair".value
    ].values
    comptime _enum_case_types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, NoneType, Int, Int
    ].values

    def __init__(out self, tag: Int, n: Int):
        self.tag = tag
        self.n = n

    def _get_enum_discriminant(self) -> Int:
        print("  disc")
        return self.tag

    def _unsafe_get_enum_payload[
        id: Int
    ](ref self) -> ref[self] TypeList[Trait=AnyType, Self._enum_case_types]()[
        id
    ]:
        print("  payload", id)
        return rebind[TypeList[Trait=AnyType, Self._enum_case_types]()[id]](
            Pointer(to=self.n).unsafe_origin_cast[origin_of(self)]()[]
        )


struct Box(EnumLike, ImplicitlyCopyable):
    var tag: Int
    var t: Tuple[Int, Int]

    comptime _enum_case_names = ParameterList.of["Empty".value, "Full".value].values
    comptime _enum_case_types: _MLIR.KGENParamListType[AnyType] = TypeList.of[
        Trait=AnyType, NoneType, Tuple[Int, Int]
    ].values

    def __init__(out self, tag: Int, a: Int, b: Int):
        self.tag = tag
        self.t = (a, b)

    def _get_enum_discriminant(self) -> Int:
        print("  disc")
        return self.tag

    def _unsafe_get_enum_payload[
        id: Int
    ](ref self) -> ref[self] TypeList[Trait=AnyType, Self._enum_case_types]()[
        id
    ]:
        print("  payload", id)
        return rebind[TypeList[Trait=AnyType, Self._enum_case_types]()[id]](
            Pointer(to=self.t).unsafe_origin_cast[origin_of(self)]()[]
        )


@fieldwise_init
struct S(ImplicitlyCopyable):
    var v: Int

    def __eq__(self, other: Int) -> Bool:
        print("  eq", self.v, other)
        return self.v == other


def g(tag: String) -> Bool:
    print("  guard", tag)
    return False


def main():
    var r = Rec(1, 5)
    print("one discriminant read")
    match r:
        case .Pair(x):
            print("pair", x)
        case .Node(x):
            print("node", x)
        case .Leaf:
            print("leaf")
    print("payload literals")
    match r:
        case .Node(4):
            print("four")
        case .Node(5):
            print("five")
        case _:
            print("other")
    print("a guard between members")
    match r:
        case .Node(x) if g("a"):
            print("node-a", x)
        case .Leaf:
            print("leaf")
        case .Node(y):
            print("node-b", y)
    print("tests, a guard, and a binding")
    match r:
        case .Node(0):
            print("zero")
        case .Node(x) if x > 100:
            print("big")
        case Rec.Node(var x):
            x += 1
            print("node", x)
        case .Leaf:
            print("leaf")
    print("a binding ends the run")
    match r:
        case .Leaf:
            print("leaf")
        case v if g("v"):
            print("v", v.n)
        case .Node(y):
            print("node", y)
        case _:
            print("other")
    print("a tuple member binds after its later test")
    var t = (r, S(2))
    match t:
        case (.Node(x), 1):
            print("one", x)
        case (.Node(y), 2):
            print("two", y)
        case _:
            print("other")
    print("unclustered, the binding reads in pattern order")
    match t:
        case (.Node(x), 1):
            print("one", x)
        case _:
            print("other")
    print("alternatives read afresh")
    match r:
        case .Leaf | .Node(_):
            print("any")
    var b = Box(1, 7, 2)
    match b:
        case .Full((x, 1 | 2)):
            print("small", x)
        case .Full((y, _)):
            print("other", y)
        case .Empty:
            print("empty")
    print("payload elements cluster below the tag")
    match b:
        case .Full((7, 1)):
            print("71")
        case .Full((7, 2)):
            print("72")
        case _:
            print("other")
