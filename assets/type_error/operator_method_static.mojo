# An operator special method must take its receiver: a static one is
# rejected at the declaration.
# expect: special method must not be a static method
struct S:
    @staticmethod
    def __pow__(a: Int, b: Int) -> Int:
        return a


def main():
    print(S.__pow__(1, 2))
