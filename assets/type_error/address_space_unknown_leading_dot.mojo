# A leading-dot parameter argument names a member of the parameter's type, so
# an undeclared `.FOO` is `AddressSpace.__getattr_param__["FOO"]()`, which
# fails on upstream's constraint.
# expect: constraint failed: unknown address space: 'FOO'
from std.memory import AddressSpace


def f[a: AddressSpace]() -> Int:
    return Int(a)


def main():
    print(f[.FOO]())
