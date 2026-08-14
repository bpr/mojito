# `Span(list)` is a borrowed contiguous view: length and element reads see
# the List's storage without copying, strict contiguous sub-slices are
# sub-views of the same storage, and writing through a span element writes
# the List. Mutating the List again is legal once every view is dead.
def main():
    var xs: List[Int] = [10, 20, 30, 40, 50]
    var sp = Span(xs)
    print(len(sp), sp[0], sp[4])
    var sub = sp[1:4]
    print(len(sub), sub[0], sub[2])
    var tail = sub[1:]
    print(len(tail), tail[0])
    sp[1] = 21
    print(xs[1])
    xs.append(60)
    print(len(xs))
