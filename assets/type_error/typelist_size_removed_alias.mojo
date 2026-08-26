# expect: compile-time 'TypeList' value has no field 'size'
# Upstream removed the deprecated `TypeList.size` alias (2026-08 window: the
# head reports `'TypeList[Int, Bool]' value has no attribute 'size'`).
# `length` is the only spelling.
def main():
    comptime tl = TypeList.of[Trait=AnyType, Int, Bool]()
    comptime if tl.size == 2:
        print(2)
    else:
        print(0)
