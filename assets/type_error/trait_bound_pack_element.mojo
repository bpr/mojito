# expect: invalid call to 'g': 'g' parameter 'Ts' has 'Copyable' type, but value has type 'AnyStruct[NC]'
struct NC:
    def __init__(out self):
        pass


def g[Tr: type_of(AnyType), //, *Ts: Tr]() -> Int:
    return Ts.length


def main():
    print(g[Tr=Copyable, Int, NC]())
