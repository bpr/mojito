# expect: expects 2 type argument
# A parameterized associated-type application must supply one argument per
# explicit (post-`//`) parameter. `Pair` has two explicit parameters, so applying
# it with a single argument is rejected.
trait Bad:
    comptime Pair[a: Int, b: Int]: AnyType

    def get(self) -> Self.Pair[1]:
        ...

def main():
    print(42)
