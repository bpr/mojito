# expect: '@always_inline("builtin")' only supports calls to other '@always_inline("builtin")' functions
# A construction in a builtin body calls its struct's `__init__`, which the
# synthesized fieldwise one is not builtin.
@fieldwise_init
struct L(TrivialRegisterPassable):
    var n: Int

    @staticmethod
    @always_inline("builtin")
    def make(n: Int) -> Self:
        return Self(n)


def main():
    print(L.make(5).n)
