# `Pointer(to=)` is upstream's declared `__init__(out self, *, ref[...] to:
# Self.T)`, so any `ref` argument constructs one: an rvalue materializes
# into a temporary that lives as long as the pointer, a literal expression
# and a runtime one alike. The pin prints 3, 3, and abc.
def main():
    var p = Pointer(to=1 + 2)
    print(p[])
    var a = 1
    var q = Pointer(to=a + 2)
    print(q[])
    var s = String("ab")
    var r = Pointer(to=s + "c")
    print(r[])
