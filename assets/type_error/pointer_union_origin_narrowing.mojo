# expect: type mismatch for variable 'r'
# A pointer whose origin is the union `origin_of(x, y)` does not convert to
# a pointer of one member's origin, as at the pin.
def main():
    var x = 5
    var y = 6
    var p = Pointer[Int, origin_of(x, y)](to=x)
    var r: Pointer[Int, origin_of(x)] = p
    print(r[])
