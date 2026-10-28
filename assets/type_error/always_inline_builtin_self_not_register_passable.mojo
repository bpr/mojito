# expect: '@always_inline("builtin")' does not support this argument convention
# A builtin method with `self` needs a trivially register-passable struct.
struct L(Copyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    @always_inline("builtin")
    def doubled(self) -> Int:
        return self.v * 2


def main():
    print(L(3).doubled())
