# expect: cannot use a dynamic value in type parameter
def first[n: Int]() -> Int:
    return n


def main():
    var x = 3

    def mk() {imm x} -> Int:
        return x

    print(first[mk()]())
