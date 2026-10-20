# A struct's parameterized type member may read the struct's own type pack:
# each application closes the pack at the instance's elements beside the
# member's own arguments, spelled directly or through a `TypeList` over the
# pack's `values`, at a concrete instance and inside a generic function.
struct W[T: AnyType, *Ts: AnyType]:
    comptime E[id: Int]: AnyType = Self.Ts[id]
    comptime L[id: Int]: AnyType = TypeList[Trait=AnyType, Self.Ts.values]()[
        id
    ]


def generic[U: AnyType]():
    var z: W[U, Bool, Int].E[1] = 7
    print(z)


def main():
    var x: W[String, Bool, Int].E[1] = 6
    print(x)
    var b: W[String, Bool, Int].E[0] = True
    print(b)
    var s: W[Int, Bool, String].L[1] = "list"
    print(s)
    generic[Float64]()
