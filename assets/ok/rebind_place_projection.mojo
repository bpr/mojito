# A projection after a `rebind` addresses the rebound type: the place reads,
# augmented-assigns, writes, and calls through the field of `S` that the
# rebind names, on the operand's own storage.


@fieldwise_init
struct S(Copyable):
    var f: Int
    var name: String

    def describe(self) -> String:
        return self.name + "=" + String(self.f)


@fieldwise_init
struct Box[T: Copyable & Deinitable](Copyable):
    var v: Self.T


def bump[T: Copyable](mut x: T) -> Int:
    rebind[S](x).f += 1
    return rebind[S](x).f


def rename[T: Copyable & Deinitable](mut b: Box[T]):
    rebind[S](b.v).f = 3
    rebind[S](b.v).name = "three"


def show[T: Copyable](x: T) -> String:
    return rebind[S](x).name.upper() + " " + rebind[S](x).describe()


def main():
    var s = S(4, "four")
    print(bump(s))
    print(s.f)
    var b = Box(S(1, "one"))
    rename(b)
    print(b.v.f, b.v.name)
    print(show(b.v))
