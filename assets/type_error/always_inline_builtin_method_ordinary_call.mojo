# expect: '@always_inline("builtin")' only supports calls to other '@always_inline("builtin")' functions
# A builtin method may call only builtin methods of `self`.
struct L(TrivialRegisterPassable):
    var v: Int

    @always_inline("builtin")
    def __init__(out self, v: Int):
        self.v = v

    def ord(self) -> Int:
        return self.v

    @always_inline("builtin")
    def doubled(self) -> Int:
        return self.ord() * 2


def main():
    print(L(3).doubled())
