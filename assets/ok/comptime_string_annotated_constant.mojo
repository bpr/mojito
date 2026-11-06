# A `String`-annotated `comptime` name over a string literal is the
# `String` its annotation converts the literal to, at module scope, as a
# struct member, and as a local: an owned binding or argument takes it, and
# a `String` method reads it, as at the pin.
comptime G: String = "glob"


def take(var s: String):
    print("take", s)


def borrow(s: String):
    print("borrow", s)


def show[s: String]():
    print("show", s)


struct A:
    comptime S: String = "own"


def main():
    var u = G
    u += "!"
    print(u)
    take(G)
    borrow(G)
    show[G]()
    print(G.byte_length())
    take(A.S)
    comptime L: String = "loc"
    take(L)
    print(L.byte_length())
