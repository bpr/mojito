# expect: type mismatch for variable 'x': expected Int, found StringLiteral
# A `comptime for` over a symbolic type's field count is checked once, its
# body under a symbolic index, so a type error in the body is reported from
# the template with nothing calling `each_field`.
def each_field[T: AnyType]():
    comptime for i in range(reflect[T].field_count()):
        var x: Int = "oops"


def main():
    print("never instantiated")
