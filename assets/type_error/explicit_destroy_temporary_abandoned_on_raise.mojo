# expect: '(expression temporary)' abandoned without being explicitly destroyed: L must be closed
# A call consumes its owned operands only once every operand ran: `L(1)` is
# still owned in flight when `boom` raises, so the raise abandons it.
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
    take(L(1), boom(1))
