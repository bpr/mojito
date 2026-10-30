# A struct's `comptime` member built by `Self(...)` and read through an
# application of the struct, or an alias of one, is a value of that
# application: `S[2].Z` is an `S[2]`.


@fieldwise_init
struct S[n: Int](ImplicitlyCopyable):
    var x: Int
    comptime Z = Self(Self.n * 10)

    def scaled(self) -> Int:
        return self.x * Self.n


comptime S3 = S[3]


def main():
    var two = S[2].Z
    print(two.x, two.scaled())
    var three: S[3] = S3.Z
    print(three.x, three.scaled())
