# A generic body that calls another generic: the instance of `outer` takes the
# request for `helper[Int]` from the template's retained application, without
# `outer$Int` being inferred, and follows the elaborator's retarget to the
# clone in the next round (class FixedCalls). The generic struct's method
# forwards to the same helper; method bodies still take the clone check.
def helper[T: Copyable](x: T) -> Int:
    return 5


def outer[T: Copyable](x: T) -> Int:
    return helper(x)


@fieldwise_init
struct Box[T: Copyable & Movable & Deinitable]:
    var value: Self.T

    def forward(self) -> Int:
        return helper(self.value)


def main():
    print(outer(3))
    print(outer(True))
    print(Box(4).forward())
    print(Box(False).forward())
