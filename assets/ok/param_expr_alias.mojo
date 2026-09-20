# A non-parameterized associated alias binds the instance's value parameters
# beside its type parameters: `S[Int, 3].Alias` is `S[Int, 3]`.
struct S[T: Copyable & Deinitable, n: Int](Copyable, Movable):
    var value: Self.T
    comptime Alias = S[Self.T, Self.n]

    def __init__(out self, var value: Self.T):
        self.value = value^


def main():
    var a: S[Int, 3].Alias = S[Int, 3](7)
    print(a.value)
