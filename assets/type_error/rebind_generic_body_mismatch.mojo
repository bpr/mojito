# expect: rebind input type 'String' does not match result type 'Int'
# A `rebind` in a parametric body is asserted at every instantiation, not on
# the declaration: the elaborator judges it per instance, and this one holds
# for `Int` and fails for `String`, which the pin reports as the
# instantiation failing.
def bump[T: Copyable](mut x: T):
    rebind[Int](x) += 1

def main():
    var v = 3
    bump(v)
    var s = String("a")
    bump(s)
    print(v, s)
