# expect: 'same(...)' abandoned without being explicitly destroyed: unhandled explicitly destroyed type 'AnyType'
# Writing through a reference-returning call destroys the referent's old value,
# which a `T` not proven `Deinitable` cannot be implicitly.
def same[T: Copyable](ref a: T) -> ref[origin_of(a)] T:
    return a


def through_call[T: Copyable](mut x: T, v: T):
    same(x) = v.copy()


def main():
    var k = 1
    through_call(k, 6)
    print(k)
