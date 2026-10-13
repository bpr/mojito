# expect: 'l' abandoned without being explicitly destroyed: L must be closed
# A transfer moves its place when the call consumes it, after every operand:
# `l` is still owned when `boom` raises, so the raise abandons it.
# The pinned Mojo rejects it too.
@explicit_destroy("L must be closed")
struct L(Movable, Deinitable where False):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def close(deinit self):
        print("close", self.v)

def take(var a: L, n: Int):
    a^.close()

def boom(n: Int) raises -> Int:
    if n > 5:
        raise Error("boom")
    return n

def main() raises:
    var l = L(1)
    take(l^, boom(1))
