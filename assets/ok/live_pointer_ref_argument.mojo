# A `ref` argument naming a place while a `Pointer(to=place)` to it is still
# live. `Pointer` is provenance, not an exclusive borrow, so the second reader
# coexists with it and both print `3` (upstream agrees). A write to the owner
# still conflicts with either — that is
# `assets/ownership_error/pointer_to_owner_twice_then_mutation.mojo`.
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
