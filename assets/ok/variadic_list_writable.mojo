# A homogeneous collector over a `Writable` element is `Writable` itself,
# through upstream's `VariadicList.write_to` and `write_repr_to`: `print`
# writes its elements in parentheses and `repr` names the list's type.
def show(*xs: Int):
    print(xs)
    print(repr(xs))


def names(var *words: String):
    var text = String()
    words.write_to(text)
    print(text)


def main():
    show(1, 2, 3)
    names("a", "b")
