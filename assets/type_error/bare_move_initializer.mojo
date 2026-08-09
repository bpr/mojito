# Current Mojo rejects a bare `move:` initializer parameter with a migration
# diagnostic; the consuming spelling is `__init__(out self, *, deinit move: Self)`.
# expect: deinit move
struct Buf:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __init__(out self, *, move: Self):
        self.n = move.n

def main():
    var b = Buf(1)
    var c: Buf = b^
    print(c.n)
