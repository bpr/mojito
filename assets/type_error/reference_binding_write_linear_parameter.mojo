# expect: 'r' abandoned without being explicitly destroyed: unhandled explicitly destroyed type 'AnyType'
# Writing through a `ref` binding destroys the referent's old value, which a
# `T` not proven `Deinitable` cannot be implicitly.
def through_binding[T: Copyable](mut x: T, v: T):
    ref r = x
    r = v.copy()


def main():
    var k = 1
    through_binding(k, 5)
    print(k)
