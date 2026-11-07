# A generic function applied at two string literal types is two instances,
# each spelled with its literal's text, and each literal's `Writable`
# witnesses are its own text's members, as upstream's `StringLiteral[value]`
# hangs them off the instance its text applies.
@no_inline
def written[T: Writable](v: T) -> String:
    var s = String()
    v.write_repr_to(s)
    v.write_to(s)
    return s


def main():
    print(written[StringLiteral["hi".value]]("hi"))
    print(written[StringLiteral["a(b".value]]("a(b"))
    print(written[StringLiteral["a)b".value]]("a)b"))
    print(written("c"))
    var s = String()
    "hi".write_repr_to(s)
    StringLiteral._from["a\nb"]().write_repr_to(s)
    print(s)
