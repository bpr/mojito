# expect: type mismatch for variable 'x': expected Int, found StringLiteral
# A generic struct's method reading `reflect[Self]` is validated with `Self`
# at the struct's own parameters, so the untaken arm's type error is reported
# with no `Box` ever constructed.
struct Box[T: Copyable & Deinitable](Copyable):
    var v: Self.T

    def __init__(out self, v: Self.T):
        self.v = v.copy()

    def count(self) -> Int:
        comptime if reflect[Self].field_count() == 1:
            return 1
        else:
            var x: Int = "oops"
            return 0


def main():
    print("never instantiated")
