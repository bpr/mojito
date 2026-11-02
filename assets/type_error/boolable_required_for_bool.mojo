# expect: invalid call to 'Bool': no constructor overload matches
# `Bool(x)` on an opaque type parameter binds `Bool.__init__[T: Boolable]`. A
# plain `T: AnyType` promises no boolean conversion, so this is a type error.
def truthy[T: AnyType](x: T) -> Bool:
    return Bool(x)

def main():
    print(truthy(5))
