# expect: invalid call to 'g': 'g' parameter 'T' has 'Copyable' type, but value has type 'AnyStruct[NC]'
struct NC:
    def __init__(out self):
        pass


def g[Tr: type_of(AnyType), //, T: Tr]() -> Int:
    return 1


def main():
    print(g[Tr=Copyable, NC]())
