# expect: immutable reference returned by method call 'pick()'
# A method's reference result through a read parameter is immutable, even
# when the receiver is a mutable binding.
struct W(Copyable):
    var t: Int
    var s: String

    def __init__(out self, t: Int):
        self.t = t
        self.s = String("w")


struct H:
    def __init__(out self):
        pass

    def pick(self, w: W) -> ref[origin_of(w.t)] Int:
        return w.t


def main():
    var w = W(7)
    var h = H()
    h.pick(w) = 5
    print(w.t)
