# expect: aliasing values passed mutably to 'a' argument and passed immutably to 'b' argument in 'f' call
# A reference result read by a `read` parameter is the reference the call
# returned, so it may not alias a `mut` argument of the same call, as at the
# pin: `o.value()` reaches `o`'s payload while `a` may overwrite it.
def f(mut a: Optional[String], b: String):
    a = None
    print(b)


def main():
    var o = Optional[String]("hi")
    f(o, o.value())
