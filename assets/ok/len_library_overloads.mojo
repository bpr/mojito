# `len` is upstream's library overload set (`std/builtin/len.mojo`): one
# `@inline(.always)` overload over `Sized`, a raising one over
# `SizedRaising`, and an `@unavailable` one over a string. Each argument
# below binds the overload its conformance selects, a pack's length
# included; a type pack and a compile-time `TypeList` are measured at
# compile time.
@fieldwise_init
struct Shelf(Sized):
    var n: Int

    def __len__(self) -> Int:
        return self.n


@fieldwise_init
struct Stream(SizedRaising):
    var n: Int

    def __len__(self) raises -> Int:
        if self.n < 0:
            raise Error("unknown length")
        return self.n


def measure[T: Sized](x: T) -> Int:
    return len(x)


def runtime_pack(*values: Int) -> Int:
    return len(values)


def mixed_pack[*Ts: Writable](*args: *Ts) -> Int:
    return len(args)


def value_pack[*vs: Int]() -> Int:
    return len(vs)


def type_pack[*Ts: AnyType]() -> Int:
    return len(Ts)


def main() raises:
    var xs: List[Int] = [1, 2, 3]
    var d: Dict[String, Int] = {"a": 1, "b": 2}
    var t = (1, "two", 3.0)
    print(len(xs), len(d), len(t))
    print(len(1), len(SIMD[DType.float32, 4](0)), len(range(2, 9)))
    print(len(Shelf(4)), measure(Shelf(5)), measure(xs))
    print(len(Stream(6)))
    try:
        print(len(Stream(-1)))
    except e:
        print(e)
    print(runtime_pack(1, 2), mixed_pack(1, "a", True), value_pack[4, 5, 6]())
    print(type_pack[Int, Bool, String, Float64]())
    comptime types = TypeList.of[Trait=AnyType, Int, Bool]()
    print(len(types))
