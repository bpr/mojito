# expect: invalid call to 'g': value passed to 't' cannot be converted from 'MutOrigin' to 'Origin[mut=mut]'
# Through a receiver `S[_]` the binder `Origin[mut=Self.mut]` stays
# symbolic, so no origin of a known mutability converts to it (pinned Mojo:
# the same words).
struct S[mut: Bool, //, o: Origin[mut=mut]]:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def g[t: Origin[mut=Self.mut]](self):
        print(self.x)

def k(s: S[_]):
    var y = 1
    s.g[origin_of(y)]()

def main():
    var y = 1
    var s = S[origin_of(y)](3)
    k(s)
