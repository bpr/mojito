# A `conforms_to` subject naming no declaration is rejected at the
# declaration, not left to fail at a call.
# expect: not a compile-time Int constant: Undeclared
def f[T: AnyType]() where conforms_to(Undeclared, Copyable):
    pass


def main():
    pass
