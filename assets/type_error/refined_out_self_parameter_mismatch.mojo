# A construction whose bracket disagrees with the parameter an initializer's
# refined `out self` defines is no candidate.
# expect: type mismatch for parameter 'n' of the return type 'W[2]'
struct W[n: Int](Movable):
    var v: Int

    def __init__(out self: W[2], *, two: Int):
        self.v = two

def main():
    print(W[3](two=7).v)
