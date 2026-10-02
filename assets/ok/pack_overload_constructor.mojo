# Two type-pack constructors of one struct are an overload set: the call's
# arguments select the one with more fixed parameters, as they do for free
# functions and methods, and the selected constructor runs as its own
# per-call clone.
# requires: discovery


struct H:
    var k: Int

    def __init__[*Ts: Writable](out self, a: Int, *rest: *Ts):
        self.k = 2

    def __init__[*Ts: Writable](out self, a: Int, b: Int, *rest: *Ts):
        self.k = 3


def main():
    var x = 1
    print(H(x, x, x).k)
