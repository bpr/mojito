# `conforms_to` and `reflect[...].name()` over a struct whose parameter is a
# struct value built by construction (`W[K(4)]`) are decided at compile time,
# as at the pin: the construction argument is closed, not a free parameter.
from std.reflection import reflect


@fieldwise_init
struct K:
    var v: Int


@fieldwise_init
struct P:
    var a: Int
    var b: Bool


struct W[k: K](Intable):
    def __int__(self) -> Int:
        return 7


struct V[p: P]:
    pass


struct L[s: SIMDLength]:
    pass


def main():
    comptime c = conforms_to(W[K(4)], Intable)
    print(c)
    comptime if conforms_to(W[K(4)], Sized):
        print("sized")
    else:
        print("not sized")
    print(conforms_to(V[P(3, True)], Intable))
    print(reflect[L[4]].name())
    print(reflect[V[P(3, True)]].name())
