# A string literal's construction is its declared struct's `__init__` and
# its copy the declared struct's, at a spelled application, a brace
# initializer, a field initializer, and `type_of(x)()`.
comptime V = "abc".value


struct H:
    var x: type_of("hi")

    def __init__(out self):
        self.x = StringLiteral["hi".value]()


def main():
    var a: StringLiteral[V] = StringLiteral[V]()
    var b: StringLiteral[V] = StringLiteral[V](copy=a)
    var c: StringLiteral[V] = b.copy()
    print(a, b, c, c.byte_length())
    print(H().x)
    var d: type_of("xy") = {}
    print(d, type_of(d)())
    comptime e = StringLiteral["ct".value]()
    print(e, "{}-{}".format(1, "two"))
