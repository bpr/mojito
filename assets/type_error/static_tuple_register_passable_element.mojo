# A `String` element is not `RegisterPassable`, so it does not bind the
# element parameter.
# expect: type 'String' for parameter 'element_type' does not conform to trait 'RegisterPassable'
from std.utils import StaticTuple


def main():
    var t = StaticTuple[String, 2](fill="a")
    print(t[0])
