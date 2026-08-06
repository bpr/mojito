# expect: escapes storage
# A closure that captured a for-ref loop binding cannot leave the function as
# a value.
def sneak() -> Int:
    var values: List[Int] = [1, 2, 3]
    for ref x in values:
        def peek() {x} -> Int:
            return x
        return peek
    return 0


def main():
    print(sneak())
