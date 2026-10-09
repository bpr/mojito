# A generic closure declared in a generic body, reading the body's own
# parameter `m`, bound to a callable parameter. The pin prints 53; Mojito
# fails with "operator Add is not defined for Int and None", whether or not
# the closure captures (roadmap R532).
#
# Observed 2026-10-09 against `Mojo 1.2.0.dev2026092105`.
def invoke_captured[
    origins: OriginSet,
    //,
    callback: def[n: Int](Int) capturing[origins] -> Int,
]() -> Int:
    return callback[2](40)


def outer[m: Int]() -> Int:
    var offset = 1

    @parameter
    def captured_add[n: Int](value: Int) -> Int:
        return value + n + offset + m

    return invoke_captured[captured_add]()


def main():
    print(outer[10]())
