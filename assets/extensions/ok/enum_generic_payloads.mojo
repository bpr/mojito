# A generic `enum` with conditional `ImplicitlyCopyable`, matched over its
# own parameters with their bounds; a register-passable enum built by its
# own `@implicit` initializer; and a compile-time enum value under
# `comptime match`, as the pin's integration test runs them.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).


enum Result[T: Copyable & Deinitable, E: Copyable & Deinitable](
    Copyable,
    ImplicitlyCopyable where conforms_to(T, ImplicitlyCopyable) and conforms_to(
        E, ImplicitlyCopyable
    ),
):
    case success(Self.T)
    case failure(Self.E)

    def value_or(self, default: Self.T) -> Self.T:
        match self:
            case .success(value):
                return value.copy()
            case .failure(_):
                return default.copy()


def implicit_copy[T: ImplicitlyCopyable](value: T) -> T:
    return value


enum Shape(ImplicitlyCopyable, RegisterPassable):
    case empty
    case square(Int)
    case rect(Int, Int)

    @implicit
    def __init__(out self, side: Int):
        self = .square(side)

    def area(self) -> Int:
        match self:
            case .empty:
                return 0
            case .square(side):
                return side * side
            case .rect((width, height)):
                return width * height


def main():
    var ok: Result[Int, String] = .success(1)
    var err: Result[Int, String] = .failure("bad")
    var ok_copy = implicit_copy(ok)
    print(ok_copy.value_or(7), err.value_or(7))
    var list: Result[List[Int], String] = .success([1, 2])
    print(len(list.copy().value_or([])))
    print(
        Result[Int, String].success(3).value_or(7),
        implicit_copy(Result[Int, String].failure("bad")).value_or(8),
    )

    var shapes: List[Shape] = [Shape.empty, Shape.rect(2, 3), 4]
    print(shapes[0].area(), shapes[1].area(), shapes[2].area())

    comptime shape: Shape = .rect(2, 5)
    comptime match shape:
        case .rect((width, height)):
            print("rect", width, height)
        case _:
            print("other")
