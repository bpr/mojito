# expect: '@always_inline("builtin")' does not support this argument convention
# A builtin method takes `self` by `imm` alone.
struct L(TrivialRegisterPassable):
    var v: Int

    @always_inline("builtin")
    def __init__(out self, v: Int):
        self.v = v

    @always_inline("builtin")
    def bump(mut self):
        self.v = self.v + 1


def main():
    var l = L(3)
    l.bump()
    print(l.v)
