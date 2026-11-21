# expect: violated constraint
# A struct's `where Self.mut` reads the receiver's `mut=` argument, so a
# receiver over an immutable origin violates it.


struct Q[mut: Bool, //, o: Origin[mut=mut]]:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def get(self) -> Int where Self.mut:
        return self.n


def main():
    print(Q[ImmutAnyOrigin](4).get())
