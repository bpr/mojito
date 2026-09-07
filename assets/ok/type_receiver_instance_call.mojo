# An instance method called through its type takes the receiver as the first
# argument (`List[Int].__len__(xs)` is `xs.__len__()`), on parameterized and
# plain structs, for reading and mutating receivers.
@fieldwise_init
struct Point(Copyable, Movable):
    var x: Int
    var y: Int
    def norm1(self) -> Int:
        return self.x + self.y
    def shift(mut self, dx: Int):
        self.x += dx

def main():
    var xs: List[Int] = [1, 2, 3]
    var span = Span(xs)
    print(Span[Int, origin_of(xs)].__len__(span))
    print(List[Int].__len__(xs))
    var p = Point(3, 4)
    print(Point.norm1(p))
    Point.shift(p, 10)
    print(p.x, List[Int].__getitem__(xs, 1))
    print(Optional[Int].value(Optional[Int](9)))
    List[Int].append(xs, 4)
    print(xs)
