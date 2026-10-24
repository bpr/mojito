# `String(x)` builds an owned nominal `String`, so it converts into an
# `Optional[String]` at an assignment and at a parameter like any other
# `String` value.
def take(x: Optional[String]) -> String:
    return x.value()


def main():
    var o = Optional[String](None)
    print(Bool(o))
    o = String(3)
    print(o.value())
    print(take(String(5)))
    var xs = [String(1), String(2.5)]
    print(xs[0] + xs[1])
