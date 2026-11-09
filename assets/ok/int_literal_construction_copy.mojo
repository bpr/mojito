# A numeric literal's construction is its declared struct's `__init__` and
# its copy the declared struct's, at a spelled application, a brace
# initializer, and a field initializer.
comptime V = (7).value


struct H:
    var x: type_of(3)

    def __init__(out self):
        self.x = IntLiteral[(3).value]()


def main():
    var a: IntLiteral[V] = IntLiteral[V]()
    var b: IntLiteral[V] = IntLiteral[V](copy=a)
    var c: IntLiteral[V] = b.copy()
    print(a, b, c, Int(c) + 1)
    var f: FloatLiteral[(2.5).value] = FloatLiteral[(2.5).value]()
    var g: type_of(f) = FloatLiteral[(2.5).value](copy=f)
    print(g, g.copy())
    print(H().x)
    var d: type_of(4) = {}
    print(d, type_of(d)())
