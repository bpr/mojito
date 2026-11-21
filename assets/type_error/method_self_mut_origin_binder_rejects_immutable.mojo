# expect: invalid call to 'g': value passed to 't' cannot be converted from 'ImmOrigin' to 'MutOrigin'
# A method's origin binder typed `Origin[mut=Self.mut]` takes the receiver's
# `mut`: on a mutable `S[origin_of(y)]` it is a `MutOrigin`, so an
# `ImmOrigin(...)` argument does not convert (pinned Mojo: the same words).
struct S[mut: Bool, //, o: Origin[mut=mut]]:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def g[t: Origin[mut=Self.mut]](self):
        print(self.x)

def main():
    var y = 1
    var s = S[origin_of(y)](3)
    s.g[ImmOrigin(origin_of(y))]()
