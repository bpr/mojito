# A `mut` parameter must hold a value when the function leaves by raising
# too.
# expect: use of uninitialized value 'x'
# requires: stdlib
def take(mut x: String) raises -> String:
    var r = x^
    print(r)
    raise Error("boom")


def main() raises:
    var s = String("a")
    print(take(s))
