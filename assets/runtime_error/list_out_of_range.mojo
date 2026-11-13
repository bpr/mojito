# A List element index outside `0..size` aborts (strict bounds, as Span has).
# The pinned Mojo asserts here at every optimization level; unchecked, the
# native backend had no arena to catch the raw read and returned a value.
# expect: abort: index 10 is out of bounds, valid range is 0 to 2
def main():
    var xs: List[Int] = [1, 2, 3]
    var y: Int = xs[10]
    print(y)
