# `Slice(...)` reads `Optional[Int]` bound arguments (the descriptor's own
# field type) alongside `Int` and `None`.
def main():
    var start = Optional[Int](1)
    var stop = Optional[Int](4)
    var slice = Slice(start, stop, None)
    print(slice)
    print(repr(slice))
    var open = Slice(Optional[Int](), stop, Optional[Int](2))
    print(open, start, stop)
    var xs: List[Int] = [10, 20, 30, 40, 50]
    print(xs[slice], xs[open])
