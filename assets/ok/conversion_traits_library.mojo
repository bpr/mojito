# `Boolable`, `Intable`, `Floatable`, `Indexer`, and `Identifiable` are
# upstream's library traits, bundled in `std/builtin/{bool,int,floatable,
# identifiable}.mojo`: a call through a bound reaches the requirement, a
# scalar answers it as the upstream struct it models declares, and
# `Identifiable` defaults `__isnot__` through `__is__`. A float literal
# binds a `Floatable` parameter as `FloatLiteral`, since `Float64` is not
# `Floatable`. A scalar's conversion dunders are its own methods too, and a
# trait refining one of these converts and tests truth like the trait.
@fieldwise_init
struct Handle(Boolable, Copyable, Floatable, Identifiable, Indexer, Intable):
    var v: Int

    def __bool__(self) -> Bool:
        return self.v != 0

    def __int__(self) -> Int:
        return self.v * 10

    def __float__(self) -> Float64:
        return Float64(self.v) + 0.5

    def __mlir_index__(self) -> __mlir_type.index:
        return self.v.__mlir_index__()

    def __is__(self, rhs: Self) -> Bool:
        return self.v == rhs.v


trait Countable(Boolable, Intable):
    pass


@fieldwise_init
struct Tally(Copyable, Countable):
    var n: Int

    def __bool__(self) -> Bool:
        return self.n > 0

    def __int__(self) -> Int:
        return self.n


def counted[T: Countable](x: T) -> Int:
    if x:
        return Int(x)
    return x.__int__() + 100


def truthy[T: Boolable](x: T) -> Bool:
    return x.__bool__()


def whole[T: Intable](x: T) -> Int:
    return x.__int__()


def real[T: Floatable](x: T) -> Float64:
    return x.__float__()


def distinct[T: Identifiable](a: T, b: T) -> Bool:
    return a is not b


def pick[T: Indexer & Copyable](values: List[Int], i: T) -> Int:
    return values[i]


def main():
    print(truthy(Handle(1)), truthy(Handle(0)), truthy(3), truthy(0.0), truthy(True))
    print(truthy("a"), truthy(""))
    print(whole(Handle(4)), whole(7), whole(True), whole(2.5))
    print(real(Handle(2)), real(True), real(1.5))
    print(distinct(Handle(1), Handle(1)), distinct(Handle(1), Handle(2)))
    print(Handle(1) is not Handle(2), Handle(1) is Handle(1))
    var values: List[Int] = [10, 20, 30]
    print(values[Handle(2)], pick(values, Handle(1)), pick(values, 0))
    comptime handle_boolable = conforms_to(Handle, Boolable)
    comptime int_floatable = conforms_to(Int, Floatable)
    comptime bool_floatable = conforms_to(Bool, Floatable)
    comptime bool_indexer = conforms_to(Bool, Indexer)
    comptime uint_indexer = conforms_to(UInt, Indexer)
    comptime float_intable = conforms_to(Float64, Intable)
    comptime string_boolable = conforms_to(String, Boolable)
    comptime int_identifiable = conforms_to(Int, Identifiable)
    print(handle_boolable, int_floatable, bool_floatable, bool_indexer)
    print(uint_indexer, float_intable, string_boolable, int_identifiable)
    var count = 3
    var ratio = 0.5
    var flag = True
    var narrow = Float32(2.5)
    var lanes = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(count.__bool__(), ratio.__bool__(), UInt(0).__bool__())
    print(ratio.__int__(), flag.__int__(), narrow.__int__(), lanes.reduce_add().__int__())
    print(flag.__float__())
    print(counted(Tally(3)), counted(Tally(-2)), not Tally(0))
