# A field store below a `List` subscript writes the element in place: the
# checker-selected reference-returning `__getitem__` is evaluated once into a
# hidden `ref` handle and the field store goes through that handle.
@fieldwise_init
struct Point(Copyable, Movable):
    var x: Int
    var y: Int

def main():
    var points: List[Point] = [Point(1, 2), Point(3, 4)]
    var i = 1
    points[i].x = 9
    points[0].y += 5
    print(points[0].x, points[0].y, points[1].x, points[1].y)
