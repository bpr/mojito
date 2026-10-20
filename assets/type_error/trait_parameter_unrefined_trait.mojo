# expect: 'g' parameter 'Tr' has 'AnyTrait[Copyable]' type, but value has type 'AnyTrait[Movable]'
def g[Tr: type_of(Copyable)]():
    print("g")


def main():
    g[Movable]()
