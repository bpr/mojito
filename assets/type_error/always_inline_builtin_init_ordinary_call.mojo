# expect: '@always_inline("builtin")' only supports calls to other '@always_inline("builtin")' functions
# A builtin `__init__`'s body is judged as any builtin body: it calls no
# ordinary function.
def h(n: Int) -> Int:
    return n


struct L(TrivialRegisterPassable):
    var v: Int

    @always_inline("builtin")
    def __init__(out self, v: Int):
        self.v = h(v)


def main():
    print(L(3).v)
