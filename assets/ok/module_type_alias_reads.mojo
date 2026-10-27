# A module type alias is a declaration the check reads by name: an
# expression it heads is the same expression over its parameter expression
# (a static call, a member read, a construction), it types a signature, a
# field, a parameter default, and a list, it compares in a `comptime if`,
# and an overload over it is told apart by the type it names.


struct Q:
    @staticmethod
    def make() -> Int:
        return 7

    comptime k = 5


comptime A = Q
comptime F = Float64
comptime V = SIMD[DType.int32, 2]
comptime B = Int
comptime C = B
comptime KS = __mlir_type.`!kgen.string`


struct Holder[U: AnyType = C]:
    var v: C

    def __init__(out self, v: C):
        self.v = v

    def twice(self) -> C:
        return Int(F(2.0)) * self.v


struct Keyed[key: V]:
    var n: Int

    def __init__(out self):
        self.n = Int(Self.key[0])

    def __init__(out self, seed: V):
        self.n = Int(Self.key[1] + seed[0])


comptime Zero = Keyed[V(5, 6)]


struct Sized[n: Int]:
    @staticmethod
    def total() -> Int:
        return Self.n


comptime S3 = Sized[3]
comptime Pair = Tuple[Int, String]


def bump(x: C) -> C:
    return x + 1


def pick(x: V) -> String:
    return "vector"


def pick(x: F) -> String:
    return "float"


def which[U: AnyType]():
    comptime if U == C:
        print("int")
    else:
        print("other")


def build[T: Copyable]() -> Int:
    var l = List[C]()
    l.append(bump(4))
    return l[0] + A.make()


def shadow[B: Intable](x: B) -> Int:
    return Int(x)


def tag[s: KS](label: String):
    print(label)


def main():
    print(A.make(), A.k)
    print(F(1.5), V(3))
    print(bump(2), Holder(3).v, Holder(3).twice())
    which[Int]()
    which[Bool]()
    print(build[Int]())
    print(shadow(Float64(2.5)))
    tag["hi".value]("hi")
    print(Zero().n, Zero(V(10)).n)
    print(pick(V(1)), pick(F(1.0)))
    print(S3.total(), Pair.__len__())
