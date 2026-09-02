# Static-method dispatch on parameterized nominal types: the explicit
# `Type[Args].static(...)` receiver (TypeApply and its single-argument
# subscript parse) and the bare `Type.static(...)` receiver with the struct's
# parameters inferred from the argument types. Overloads resolve through the
# checker-selected symbols, including same-arity pairs the VM's arity
# fallback cannot separate.


struct Box[T: Copyable & Movable]:
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    @staticmethod
    def filled(var item: Self.T) -> Self:
        return Box(item^)

    @staticmethod
    def tag() -> Int:
        return 2

    @staticmethod
    def pick(var item: Self.T) -> Self:
        return Box(item^)

    @staticmethod
    def pick(count: Int) -> Int:
        return count * 10


def main() raises:
    # Explicit receivers: a builtin scalar argument and a struct argument
    # (the latter parses as a subscript, not a TypeApply).
    var a = Box[Int].filled(7)
    print(a.item)
    var b = Box[String].filled(String("hi"))
    print(b.item)
    print(Box[Int].tag())
    # Bare receiver: T inferred from the argument.
    var c = Box.filled(String("inferred"))
    print(c.item)
    # Same-arity overloads ride the checker's selection.
    var d = Box[String].pick(String("z"))
    print(d.item)
    print(Box[String].pick(4))
    # Dict.fromkeys — the stdlib's first parametric static — both spellings.
    var keys = List[String]()
    keys.append(String("a"))
    keys.append(String("b"))
    var counts = Dict.fromkeys(keys, 0)
    print(len(counts))
    print(counts[String("b")])
    var nums = List[Int]()
    nums.append(4)
    nums.append(9)
    var names = Dict[Int, String].fromkeys(nums, String("x"))
    print(len(names))
    print(names[9])
