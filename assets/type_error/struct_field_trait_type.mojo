# expect: struct fields do not support trait types; 'AnyType' is a trait, use a concrete type or compile-time generic
# A field's type is concrete or a compile-time generic, never a trait.


struct S:
    var x: AnyType


def main():
    pass
