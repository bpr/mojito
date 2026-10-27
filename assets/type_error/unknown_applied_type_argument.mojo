# An application headed by an undeclared name in a bracket operand.
# expect: unknown type 'Foo'


def main():
    var xs = List[Foo[Int]]()
    print(len(xs))
