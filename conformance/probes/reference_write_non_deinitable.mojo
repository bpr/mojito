# PROBE (divergence): writing over a non-`Deinitable` value through a
# reference.
#
# The pinned Mojo rejects both stores: the overwritten `T` "abandoned without
# being explicitly destroyed ... consider adding trait conformance to
# Deinitable". Mojito rejects the plain `x = v.copy()` alike, but runs both
# writes through a reference and prints `5` and `6`.
#
# When fixed: move to `assets/type_error`.
def same[T: Copyable](ref a: T) -> ref[origin_of(a)] T:
    return a


def through_binding[T: Copyable](mut x: T, v: T):
    ref r = x
    r = v.copy()


def through_call[T: Copyable](mut x: T, v: T):
    same(x) = v.copy()


def main():
    var k = 1
    through_binding(k, 5)
    print(k)
    through_call(k, 6)
    print(k)
