# Two initializers that differ only in their refined `out self` types
# overload on the return type.
# expect: redefinition of function '__init__', cannot overload on return type
struct W[n: Int](Movable):
    var v: Int

    def __init__(out self: W[2], *, two: Int):
        self.v = two

    def __init__(out self: W[3], *, two: Int):
        self.v = two + 1000

def main():
    print(W[2](two=7).v)
