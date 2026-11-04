# A `conforms_to` pack subject naming no declaration is rejected at the
# declaration, not left to fail at a call.
# expect: unknown type 'Us'
def f[*Ts: AnyType]() where conforms_to(Us.values, Copyable):
    pass


def main():
    pass
