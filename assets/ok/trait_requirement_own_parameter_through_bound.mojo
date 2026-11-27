# A requirement's own compile-time parameters are applied by a call through
# a bound, from its bracket, the receiver's place, or an argument's origin,
# as a call on the conformer itself applies them.


trait Maker:
    def mk[n: Int](self) -> SIMD[DType.int32, n * 2]:
        ...


@fieldwise_init
struct M(Copyable, Maker):
    def mk[n: Int](self) -> SIMD[DType.int32, n * 2]:
        return SIMD[DType.int32, n * 2](n)


def use_maker[T: Maker](x: T):
    print(x.mk[4]())


trait Spaced:
    def space[a: AddressSpace](ref[_, a] self) -> Int:
        ...


@fieldwise_init
struct S(Copyable, Spaced):
    def space[a: AddressSpace](ref[_, a] self) -> Int:
        return 0 if a == AddressSpace.GENERIC else 1


def use_spaced[U: Spaced](u: U):
    print(u.space())


trait Peek:
    def peek[m: Bool, //, o: Origin[mut=m]](self, ref[o] x: Int) -> Bool:
        ...


@fieldwise_init
struct Box(Copyable, Peek):
    var v: Int

    def peek[m: Bool, //, o: Origin[mut=m]](self, ref[o] x: Int) -> Bool:
        return m


def via[P: Peek](p: P, mut x: Int):
    print(p.peek(x))


def main():
    print(M().mk[2]())
    use_maker(M())
    use_spaced(S())
    var x = 3
    print(Box(1).peek(x))
    via(Box(1), x)
