# `Pointer`'s `IntLiteral` initializer rejects a null address at compile time.
# expect: constraint failed: Pointer is non-nullable
def main():
    print(Int(Pointer[Int, MutAnyOrigin](unsafe_from_address=0)))
