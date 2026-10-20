# expect: cannot infer type parameter 'Tr' of 'g'
def g[Tr: type_of(AnyType)]() -> Int:
    return 1


def main():
    print(g())
