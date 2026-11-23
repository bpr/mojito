# `__pow__` takes two or three operands (the third a modulus), its
# receiver included; a fourth is rejected at the declaration.
# expect: '__pow__' requires at most 3 operands
@fieldwise_init
struct S(Copyable):
    var v: Int

    def __pow__(self, exp: Int, modulus: Int = 0, scale: Int = 1) -> Int:
        return self.v


def main():
    print(S(2) ** 3)
