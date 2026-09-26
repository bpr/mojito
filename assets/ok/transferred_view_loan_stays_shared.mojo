# A view moved into a container carries the shared loan its construction took,
# and the container keeps it shared: the viewed list stays readable while the
# container holds the view, whether the view went in as a temporary or from a
# local, and whether the list is a read parameter or a `var`.
def from_read_parameter(xs: List[Int]) -> Int:
    var views = List[Span[Int, origin_of(xs)]]()
    views.append(Span(xs))
    var view = Span(xs)
    views.append(view)
    return len(xs) * 10 + len(views)


def main():
    var xs = List[Int]()
    xs.append(1)
    xs.append(2)
    print(from_read_parameter(xs))
    var views = List[Span[Int, origin_of(xs)]]()
    views.append(Span(xs))
    print(len(xs) + views[0][1])
