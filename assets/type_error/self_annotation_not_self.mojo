# A receiver other than an initializer's `out self` may be annotated only
# with `Self`: there are no conditional methods.
# expect: 'self' argument must have type 'Self'; use a 'where' clause
struct W[n: Int](Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def two_only(self: W[2]) -> Int:
        return self.v

def main():
    print(W[2](5).two_only())
