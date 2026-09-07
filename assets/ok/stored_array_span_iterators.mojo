# The bundled Array and Span iterators iterate themselves: a stored
# `arr.__iter__()` / `span.__iter__()` drives a loop, resumes after an
# explicit `__next__`, and keeps its source view
# alive (the iterator lends the Span place itself, not only the List it
# borrows). A mutable Span iterator writes through `for ref`.
def main():
    var a = [1, 2, 3]
    var ai = a.__iter__()
    for x in ai:
        print(x)
    var xs: List[Int] = [10, 20, 30]
    var sp = Span(xs)
    var it = sp.__iter__()
    try:
        print(it.__next__())
    except StopIteration:
        print("stop")
    for y in it:
        print(y)
    var ms = Span(xs)
    var mit = ms.__iter__()
    for ref z in mit:
        z += 1
    print(xs[0], xs[1], xs[2])
