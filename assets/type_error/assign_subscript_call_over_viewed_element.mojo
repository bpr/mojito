# expect: aliasing values passed immutably to 'v' argument and constructed as a result in 'keep' call
# A nominal `__setitem__` store whose value is a call borrowing an owned
# interior of the same list (a view of an element's bytes) is rejected, as
# upstream does for a whole assignment.
def keep(v: StringSpan) -> String:
    return String(v)

def main():
    var xs: List[String] = [String("ab  "), String("c ")]
    xs[0] = keep(xs[0].rstrip())
    print(xs[0])
