# expect: dynamic type values not permitted yet
def g[Tr: type_of(AnyType)]():
    var x = Tr


def main():
    g[Copyable]()
