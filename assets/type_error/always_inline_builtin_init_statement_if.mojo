# expect: '@always_inline("builtin")' can only handle single-result if
# A builtin `__init__`'s body is judged as any builtin body: it takes no
# statement `if`.
struct L(TrivialRegisterPassable):
    var v: Int

    @always_inline("builtin")
    def __init__(out self, v: Int):
        if v > 0:
            self.v = v
        else:
            self.v = 0


def main():
    print(L(3).v)
