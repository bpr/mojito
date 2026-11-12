# A trait constant's annotation is its metatype, which its value must
# convert to: `Movable` does not imply `Copyable`.
# expect: cannot implicitly convert 'AnyTrait[Movable]' value to 'AnyTrait[Copyable]'
comptime Q: type_of(Copyable) = Movable


def main():
    print(1)
