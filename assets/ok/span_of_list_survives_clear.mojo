# A `Span(list)` view used after the list is cleared. Its loan names the
# list itself, which the clear mutates in place, so the view's length is
# still read, as at the pin: `3`.
def main():
    var xs: List[Int] = [1, 2, 3]
    var ys = Span(xs)
    xs.clear()
    print(len(ys))
