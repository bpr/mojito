# A callable parameter bound to a generic closure that captures. The instance
# folds the bound body, takes the captured places as trailing reference
# parameters, and calls the body's specialization directly with them: a write
# after the closure's declaration, or through another specialization, is seen
# by the next call, and the environment forwards through another binding.
struct Box(Copyable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x


def invoke_captured[
    origins: OriginSet,
    //,
    callback: def[n: Int](Int) capturing[origins] -> Int,
]() -> Int:
    return callback[2](40)


def forward[
    origins: OriginSet,
    //,
    callback: def[n: Int](Int) capturing[origins] -> Int,
]() -> Int:
    return invoke_captured[callback]() + callback[1](0)


def both[
    o1: OriginSet,
    o2: OriginSet,
    //,
    plain: def(Int) capturing[o1] -> Int,
    callback: def[n: Int](Int) capturing[o2] -> Int,
]() -> Int:
    return plain(1) + callback[3](0)


def bump[
    origins: OriginSet,
    //,
    callback: def[n: Int]() capturing[origins] -> None,
]():
    callback[7]()


def main():
    var offset = 0
    var box = Box(100)

    @parameter
    def captured_add[n: Int](value: Int) -> Int:
        return value + n + offset + box.x

    @parameter
    def plain(value: Int) -> Int:
        return value + offset

    @parameter
    def add_to[n: Int]():
        offset += n

    offset = 5
    print(invoke_captured[captured_add]())
    print(forward[captured_add]())
    print(both[plain, captured_add]())
    bump[add_to]()
    print(offset)
