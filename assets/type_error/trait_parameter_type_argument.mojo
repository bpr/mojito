# expect: 'g' parameter 'Tr' has 'AnyTrait[AnyType]' type, but value has type 'AnyStruct[Int]'
def g[Tr: type_of(AnyType)]():
    print("g")


def main():
    g[Int]()
