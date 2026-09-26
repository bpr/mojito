# `Self(...)` inside a method constructs the enclosing struct, positionally or
# by keyword; the pin prints "False False", Mojito reports "Undefined
# variable 'Self'".
@fieldwise_init
struct P(Copyable, Movable):
    var a: Int
    var b: Bool

    def flipped(self) -> Self:
        return Self(self.a, not self.b)

    def flipped_kw(self) -> Self:
        return Self(b=not self.b, a=self.a)

def main():
    var p = P(1, True)
    print(p.flipped().b, p.flipped_kw().b)
