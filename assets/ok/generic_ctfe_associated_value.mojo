# An associated compile-time member reached through a trait-bounded type
# parameter folds at compile time: `capacity[Buffer[8]]()` reads `T.size` during
# CTFE for the module-level `comptime C`, and a second alias folds the
# comparison, so the printed `True` pins the folded value. (The assertion is an
# alias rather than a module-level `comptime if`, which upstream requires inside
# a function.)
trait Fixed:
    comptime size: Int

struct Buffer[n: Int](Fixed):
    comptime size = Self.n
    var tag: Int

def capacity[T: Fixed]() -> Int:
    return T.size

comptime C = capacity[Buffer[8]]()
comptime C_IS_8 = C == 8

def main():
    print(C)
    print(C_IS_8)
