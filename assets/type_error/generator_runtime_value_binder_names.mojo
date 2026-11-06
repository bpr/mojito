# A generator value converts to a declared generator type only when the
# binders' names agree, though a generator argument converts by signature.
# expect: type mismatch for variable 'y': expected __generator_type[c: Int] Int, found __generator_type[b: Int] Int
comptime Add[a: Int, b: Int]: Int = a + b


def main():
    var x = Add[1, _]
    var y: __generator_type[c: Int] Int = x
    _ = y
