# `for ref x in span` yields element references: writes go through to the
# underlying List storage.
def main():
    var xs = List[Int]()
    xs.append(1)
    xs.append(2)
    var sp = Span(xs)
    for ref x in sp:
        x += 10
    print(xs[0] + xs[1])
