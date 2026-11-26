# expect: operator '==' is not defined for Int if 0 < n else Tag
# A conditional type's methods are the requirements of the traits both its
# arms conform to: `Tag` is not `Equatable`, so `x == x` is undefined.
struct Tag(Copyable, Writable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def write_to(self, mut writer: Some[Writer]):
        writer.write("Tag(", self.v, ")")


def same[n: Int](x: Int if n > 0 else Tag) -> Bool:
    return x == x


def main():
    print(same[1](4))
