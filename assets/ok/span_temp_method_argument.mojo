# A borrowing view built inline as a method argument stays anchored across
# the call it feeds: the hidden argument slot's loan keeps `a` alive until
# `extend` has run, even though `a` has no later use.
def main():
    var a: List[Int] = [1, 2, 3]
    var b = List[Int]()
    b.extend(Span(a))
    print(len(b))
    print(b[0] + b[2])
