# A closure-trait binder bound to a generic capture-list closure, called
# with explicit parameters (`callback[2](40)`): a write after the closure's
# declaration, or through another closure, is seen by the next call, and the
# closure forwards through another binding.
struct Box(Copyable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x


def invoke_captured[F: def[n: Int](Int) -> Int](callback: F) -> Int:
    return callback[2](40)


def forward[F: def[n: Int](Int) -> Int](callback: F) -> Int:
    return invoke_captured(callback) + callback[1](0)


def both[
    P: def(Int) -> Int, F: def[n: Int](Int) -> Int
](plain: P, callback: F) -> Int:
    return plain(1) + callback[3](0)


def bump[F: def[n: Int]() -> None](callback: F):
    callback[7]()


def main():
    var offset = 0
    var box = Box(100)

    def captured_add[n: Int](value: Int) {imm offset, imm box} -> Int:
        return value + n + offset + box.x

    def plain(value: Int) {imm offset} -> Int:
        return value + offset

    def add_to[n: Int]() {mut offset}:
        offset += n

    offset = 5
    print(invoke_captured(captured_add))
    print(forward(captured_add))
    print(both(plain, captured_add))
    bump(add_to)
    print(offset)
