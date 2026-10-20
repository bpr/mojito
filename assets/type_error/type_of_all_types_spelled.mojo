# expect: unknown trait '__TypeOfAllTypes'
# `__TypeOfAllTypes` is only the name the pin prints for `!kgen.type`; no
# declaration spells it.
def k[T: __TypeOfAllTypes]() -> Int:
    return 2


def main():
    print(k[Int]())
