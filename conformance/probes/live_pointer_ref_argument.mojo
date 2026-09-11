# Question: a `ref` argument naming a place while a `Pointer(to=place)` to
# it is still live. Upstream (`1.1.0.dev2026082605`) accepts it and prints
# `3` twice: `Pointer` is not an exclusive borrow.
#
# Mojito today: rejects it with `access to 'x' conflicts with live
# reference 'p'`.
#
# On the fix: let the call's `ref` access coexist with the live pointer
# loan and move this program to `assets/ok`.
struct W[mut: Bool, //, o: Origin[mut=mut]]:
    var p: Pointer[Int, Self.o]

    def __init__(out self, ref [Self.o] x: Int):
        self.p = Pointer(to=x)

    def get(self) -> Int:
        return self.p[]


def main():
    var x = 3
    var p = Pointer(to=x)
    var d = W(x)
    print(d.get())
    print(p[])
