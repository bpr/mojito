# `Pointer`'s `IntLiteral` initializer rejects a negative address at compile
# time.
# expect: constraint failed: Pointer's address cannot be negative
def main():
    print(Int(Pointer[Int, MutAnyOrigin](unsafe_from_address=-8)))
