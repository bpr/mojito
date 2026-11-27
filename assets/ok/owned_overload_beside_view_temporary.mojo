# A method overload set where an owned `var` overload beats a view overload
# generic over its origin: the temporary argument moves into the selected
# overload, and the view candidate's solve leaves no borrow of it behind.
struct L[T: Copyable & Deinitable]:
    var xs: List[Self.T]

    def __init__(out self):
        self.xs = List[Self.T]()

    def ext(mut self, var other: List[Self.T]):
        print("owned", len(other))

    def ext(mut self, elements: Span[Self.T, _]):
        print("span", len(elements))


def main():
    var l = L[Int]()
    var q: List[Int] = [2, 3]
    l.ext(q.copy())
    l.ext(Span(q))
    print(len(q), q[0])
    var p: List[Int] = [1]
    var joined = p + q.copy()
    print(len(joined), len(q))
