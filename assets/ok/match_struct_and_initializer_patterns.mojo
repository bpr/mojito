# Struct patterns `Point(x=0, y=yv)`, contextual `{x=1}` over the subject's
# nominal type, nested struct and field patterns, omitted fields matching
# anything, `ref` and `var` field bindings, and `as` over a struct pattern.
@fieldwise_init
struct Point(ImplicitlyCopyable):
    var x: Int
    var y: Int


@fieldwise_init
struct Line(ImplicitlyCopyable):
    var a: Point
    var b: Point


def main():
    var p = Point(1, 2)
    match p:
        case Point(x=0, y=yv):
            print("y axis", yv)
        case Point(x=xv, y=2):
            print("y=2", xv)
        case _:
            print("other")
    match p:
        case {x=1}:
            print("x one")
        case _:
            print("other")
    var l = Line(Point(0, 0), Point(3, 4))
    match l:
        case Line(a={x=0, y=0}, b=Point(x=bx, y=ref by)):
            print("from origin", bx, by)
        case _:
            print("other")
    match l:
        case {b={x=3, y=4}} as whole:
            print("whole", whole.a.x)
    match (p, 5):
        case (Point(x=1, y=var yy), 5):
            yy += 1
            print(yy, p.y)
