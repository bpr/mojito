# A struct's `comptime` member annotated with a value type cannot bind one of
# the struct's type parameters: its metatype is the parameter's bound.
# expect: cannot implicitly convert 'AnyType' value to 'Int'
struct S[T: AnyType]:
    comptime x: Int = Self.T


def main():
    pass
