# expect: type 'T' has no method 'copy'
def g[Tr: type_of(AnyType), //, T: Tr](x: T) -> Int:
    var y = x.copy()
    return 1


def main():
    print(g[Tr=Copyable](3))
