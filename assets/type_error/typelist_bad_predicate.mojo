# expect: requires an IsTrivially* predicate or a one-parameter Bool-bodied comptime alias
# A TypeList any/all predicate is a per-element Bool proposition; a
# type-bodied alias is not one.
comptime Boxed[T: Copyable & Movable]: AnyType = Tuple[T, T]

def gated(x: Int) -> Int where TypeList.of[Int]().all[Boxed]():
    return x

def main():
    print(gated(1))
