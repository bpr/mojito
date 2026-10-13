# expect: 'l' abandoned without being explicitly destroyed: L must be closed
# `l` is consumed after the raising call in the `try` body, so the `except`
# arm starts with it still owned and must consume it: it does not, and the
# raise abandons it. The pinned Mojo rejects it too.
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

def main():
    var l = L(1)
    try:
        _ = boom(1)
        l^.close()
    except e:
        print(e)
