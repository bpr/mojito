# The `break` out of the `try` carries `p.a` moved to the loop's exit, where
# it is written back before `p` is used whole.
struct P(Movable):
    var a: String
    var b: String

    def __init__(out self):
        self.a = "a"
        self.b = "b"


def take(var s: String) raises:
    print(s)


def main() raises:
    var p = P()
    for _ in range(2):
        try:
            take(p.a^)
            break
        except:
            p.a = "d"
    p.a = "e"
    print(p.a, p.b)
