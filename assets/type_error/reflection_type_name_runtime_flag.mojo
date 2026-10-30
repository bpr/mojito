# `name[qualified_builtins=…]()` takes a compile-time `Bool`: a runtime
# value is no parameter.
struct Point:
    var x: Int


def main():
    var qualified = True
    print(reflect[Point].name[qualified_builtins=qualified]())
