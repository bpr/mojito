# A generic capture-list closure declared in a generic body, reading the
# body's own parameter `m`, passed to a closure-trait binder and called with
# an explicit parameter (`callback[2](40)`). The pin prints 53.
def invoke_captured[F: def[n: Int](Int) -> Int](callback: F) -> Int:
    return callback[2](40)


def outer[m: Int]() -> Int:
    var offset = 1

    def captured_add[n: Int](value: Int) {imm offset} -> Int:
        return value + n + offset + m

    return invoke_captured(captured_add)


def main():
    print(outer[10]())
