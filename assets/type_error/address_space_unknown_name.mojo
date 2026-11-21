# A name `AddressSpace` does not declare is its `__getattr_param__` call,
# whose default plugin hook answers `None`, so the instantiation fails on
# upstream's constraint.
# expect: constraint failed: unknown address space: 'FOO'
from std.memory import AddressSpace


def main():
    print(AddressSpace.FOO)
