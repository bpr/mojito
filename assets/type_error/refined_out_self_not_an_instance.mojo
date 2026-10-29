# An initializer's refined `out self` type must be an instance of the
# enclosing struct.
# expect: 'self' argument must have type 'W[n]', but actually has type 'Int'
struct W[n: Int](Movable):
    var v: Int

    def __init__(out self: Int, *, w: Int):
        self = w

def main():
    print(W[3](w=1))
