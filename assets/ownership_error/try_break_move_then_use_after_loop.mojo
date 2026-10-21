# expect: use of uninitialized value 'x'
# The `break` out of the `try` carries `x` moved to the loop's exit.
struct S(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


def take(var s: S) raises:
    print(s.v)


def main() raises:
    var x = S(1)
    var i = 0
    while i < 3:
        i += 1
        try:
            take(x^)
            break
        except:
            x = S(2)
    print(x.v)
