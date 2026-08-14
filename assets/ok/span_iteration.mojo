# Span borrowed iteration: `for x in span` reads element values, a strict
# sub-slice view iterates its own window of the same storage, and `for ref
# x` writes through to the underlying List. (The source read comes after
# the last view use: reading the source between a mutable span loop and a
# later use of the span over-rejects — the order-sensitive shared-view
# residue recorded with the §5 views work.)
def main():
    var xs = List[Int]()
    xs.append(1)
    xs.append(2)
    xs.append(3)
    var sp = Span(xs)
    var total = 0
    for x in sp:
        total += x
    print(total)
    var sub = sp[1:3]
    var subtotal = 0
    for x in sub:
        subtotal += x
    print(subtotal)
    for ref x in sp:
        x += 10
    print(xs[0] + xs[1] + xs[2])
