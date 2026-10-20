# `Sized` and `SizedRaising` are the bundled library's traits, each
# requiring `__len__`: a bound's `__len__` resolves through the trait, a
# struct conforms by declaring one, and the `SIMD` scalars and tuples
# conform as upstream's structs declare.
def through_method[T: Sized](x: T) -> Int:
    return x.__len__()


def through_len[T: Sized](x: T) -> Int:
    return len(x)


def raising[T: SizedRaising](x: T) raises -> Int:
    return x.__len__()


def sized[T: AnyType]() -> String:
    comptime if conforms_to(T, Sized):
        return "Sized"
    comptime if conforms_to(T, SizedRaising):
        return "SizedRaising"
    return "-"


@fieldwise_init
struct Shelf(Sized):
    var n: Int

    def __len__(self) -> Int:
        return self.n


@fieldwise_init
struct Probe(SizedRaising):
    var n: Int

    def __len__(self) raises -> Int:
        if self.n < 0:
            raise Error("negative")
        return self.n


def main() raises:
    print(through_method(Shelf(3)))
    print(raising(Probe(4)))
    print(through_len(Shelf(5)))
    print(through_method([1, 2]))
    print(through_len(SIMD[DType.int32, 4](1)))
    print(through_method(Float64(2.5)))
    print(through_len(7))
    print(through_len((1, 2, 3)))
    print(len(UInt(9)))
    print(sized[Int](), sized[Bool](), sized[String](), sized[Optional[Int]]())
    print(sized[List[Int]](), sized[Dict[Int, Int]](), sized[Shelf](), sized[Probe]())
