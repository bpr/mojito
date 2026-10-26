# `Defaultable`, `RegisterPassable`, and `TrivialRegisterPassable` are
# upstream's library traits in the bundled `std/builtin/value.mojo`, and a
# declared trait may refine a lifecycle trait: `TrivialRegisterPassable`
# refines `ImplicitlyCopyable`, so a struct declaring only it is copyable.
from std.math import DivModable


@fieldwise_init
struct P(TrivialRegisterPassable, Writable):
    var a: Int


# A `Copyable` struct holds a trivially register-passable field.
@fieldwise_init
struct Box(Copyable):
    var p: P


# A declared trait refining a lifecycle trait, as `std.traits` declares
# them.
trait Duplicable(ImplicitlyCopyable):
    pass


@fieldwise_init
struct D(Duplicable):
    var n: Int


struct Zero(Defaultable):
    var v: Int

    def __init__(out self):
        self.v = 0


def by_method[T: Copyable](x: T) -> T:
    return x.copy()


def by_constructor[T: Copyable](x: T) -> T:
    return T(copy=x)


def twice[T: Duplicable](x: T) -> T:
    var y = x
    return y^


# `DivModable` refines `ImplicitlyCopyable`, so its bound copies `x`.
def quotient[T: DivModable & Deinitable](x: T, d: T) -> T:
    var y = x
    var parts = divmod(y, d)
    return parts[0]


def keep[T: TrivialRegisterPassable](x: T) -> T:
    var y = x
    return y


def fresh[T: Defaultable & Movable]() -> T:
    return T()


def show[T: AnyType](name: String):
    var d = False
    var r = False
    var t = False
    comptime if conforms_to(T, Defaultable):
        d = True
    comptime if conforms_to(T, RegisterPassable):
        r = True
    comptime if conforms_to(T, TrivialRegisterPassable):
        t = True
    print(name, d, r, t)


def main():
    var q = P(1)
    print(q.copy().a, P(copy=q).a)
    print(by_method(P(2)).a, by_constructor(P(3)).a)
    print(Box(P(4)).copy().p.a)
    print(twice(D(5)).n, keep(P(6)).a)
    print(fresh[Zero]().v, fresh[Int](), quotient(9, 4))
    show[Int]("Int")
    show[Bool]("Bool")
    show[NoneType]("NoneType")
    show[DType]("DType")
    show[SIMD[DType.float32, 4]]("SIMD")
    show[Tuple[Int, Bool]]("Tuple[Int, Bool]")
    show[Tuple[Int, String]]("Tuple[Int, String]")
    show[String]("String")
    show[P]("P")
    show[Zero]("Zero")
